//! The terminal interface.

pub mod state;
pub mod theme;
mod view;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use anyhow::Result;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::{mpsc, watch};

use crate::audio::MicTest;
use crate::audio::blip::Blip;
use crate::audio::device::{AudioDevices, AudioHealth, Recovered};
use crate::config::Config;
use crate::net::voice::VoiceMesh;
use crate::net::{Command, Event, Session};
use crate::proto::{ChannelId, PeerId};
use state::{App, GATE_STEP, SettingsSection, ViewMode};
use theme::Theme;

/// Where the interface holds on to the audio side. Never built if audio failed to
/// start.
pub struct VoiceControl {
    pub mesh: VoiceMesh,
    /// Who is currently speaking.
    pub speaking: watch::Receiver<HashSet<PeerId>>,
    /// Whether the microphone is open — the interface computes it, the engine reads it.
    pub mic_open: Arc<AtomicBool>,
    /// Whether we can hear the others.
    pub hearing: Arc<AtomicBool>,
    /// Live microphone volume level (0.0 to 1.0) for VU meter visualization.
    pub mic_level: watch::Receiver<f32>,
    /// How loud each of the others is, 0-4, for the meters in the roster.
    pub peer_levels: watch::Receiver<HashMap<PeerId, u8>>,
    /// What the microphone test is doing, as `MicTest::bits`.
    pub mic_test: Arc<AtomicU8>,
    /// The microphone's noise floor, as the capture loop reads it.
    pub gate: Arc<AtomicU32>,
    pub health: Arc<AudioHealth>,
    pub blip_tx: mpsc::Sender<Blip>,
    /// The audio hardware stays open for as long as this is kept alive.
    pub devices: AudioDevices,
}

impl VoiceControl {
    pub fn play(&self, blip: Blip) {
        let _ = self.blip_tx.try_send(blip);
    }

    pub fn switch_input(&self, wanted: Option<&str>) -> Result<String> {
        self.devices.switch_input(wanted)
    }

    pub fn switch_output(&self, wanted: Option<&str>) -> Result<String> {
        self.devices.switch_output(wanted)
    }

    pub fn set_mic_test(&self, test: MicTest) {
        self.mic_test.store(test.bits(), Ordering::Relaxed);
    }

    /// Moves the microphone's noise floor. `level` is a position on the same meter the
    /// settings screen draws, so what the user drags and what the detector compares
    /// against cannot drift apart.
    pub fn set_gate(&self, level: f32) {
        self.gate
            .store(crate::audio::rms_for(level).to_bits(), Ordering::Relaxed);
    }

    pub fn active_input(&self) -> Option<String> {
        self.devices.active_input()
    }

    pub fn active_output(&self) -> Option<String> {
        self.devices.active_output()
    }

    /// Remembered devices that were not plugged in when we started.
    pub fn missing(&self) -> Vec<String> {
        self.devices.missing()
    }

    /// Reopens any stream the driver took away.
    pub fn recover(&self) -> Vec<Recovered> {
        self.devices.recover()
    }
}

/// How often the interface redraws itself while something on screen is moving.
/// Roughly 14 frames a second: enough for a pulse to read as travel, cheap enough to
/// leave a laptop alone.
const FRAME_INTERVAL: std::time::Duration = std::time::Duration::from_millis(70);

/// Decides what the interface says about your own microphone and ears.
///
/// The keys only send a command — the state that matters comes back from the
/// coordinator — so the sound follows what actually happened rather than what was
/// asked for. The first roster is a sync rather than a change, and makes no sound.
#[derive(Default)]
struct SelfChime {
    known: Option<(bool, bool)>,
}

impl SelfChime {
    fn on_roster(&mut self, muted: bool, deafened: bool) -> Option<Blip> {
        let (was_muted, was_deafened) = self.known.replace((muted, deafened))?;

        // Shutting your ears closes the microphone with them. That is one action, so
        // it makes one sound; the mute that came along is not news.
        if deafened != was_deafened {
            return Some(if deafened { Blip::EarsOff } else { Blip::EarsOn });
        }
        if muted != was_muted {
            return Some(if muted { Blip::MicOff } else { Blip::MicOn });
        }
        None
    }
}

/// Decides when the welcome chime should sound.
#[derive(Default)]
struct JoinChime {
    known: HashSet<PeerId>,
    seeded: bool,
}

impl JoinChime {
    fn on_roster(&mut self, app: &App, prev_voice: Option<ChannelId>) -> bool {
        let newcomer = app
            .peers
            .iter()
            .any(|p| p.id != app.me && !self.known.contains(&p.id));
        self.known = app.peers.iter().map(|p| p.id).collect();

        let announce = newcomer && self.seeded;
        self.seeded = true;

        let joined_channel = app.voice.is_some() && app.voice != prev_voice;
        announce || joined_channel
    }
}

/// Wires the session to the screen and runs until the user quits.
pub async fn run(
    mut session: Session,
    voice: Option<VoiceControl>,
    ptt_mode: bool,
) -> Result<()> {
    let theme = Theme::from_env();
    let mut app = App::new(session.me, session.invite_code.clone());
    app.voice_available = voice.is_some();
    app.ptt_mode = ptt_mode && app.voice_available;
    app.motion = theme.motion;
    if let Some(voice) = voice.as_ref() {
        // The rail names the microphone and speaker in use from the first frame, so
        // it has to ask the engine what it actually opened rather than wait for the
        // user to visit the settings screen.
        app.active_input_name = voice.active_input();
        app.active_output_name = voice.active_output();
        app.input_gate = Config::load().gate_for(app.active_input_name.as_deref());
        voice.set_gate(app.input_gate);
    }
    let mut terminal = ratatui::init();
    let mut keys = spawn_key_reader();
    let mut chime = JoinChime::default();
    let mut own_state = SelfChime::default();
    let mut quality_tick = tokio::time::interval(std::time::Duration::from_secs(1));

    let mut startup_notes: Vec<String> =
        voice.as_ref().map(|v| v.missing()).unwrap_or_default();
    // A device that stays gone would otherwise say so once a second, forever.
    let mut last_audio_note: Option<String> = None;

    let mut speaking_rx = voice.as_ref().map(|v| v.speaking.clone());
    let mut mic_level_rx = voice.as_ref().map(|v| v.mic_level.clone());
    let mut peer_levels_rx = voice.as_ref().map(|v| v.peer_levels.clone());

    let result = async {
        loop {
            terminal.draw(|frame| view::draw(frame, &app, &theme))?;

            tokio::select! {
                event = session.events.recv() => match event {
                    Some(event) => {
                        let membership_changed =
                            matches!(event, Event::Roster(_) | Event::Welcome { .. });
                        let prev_voice = app.voice;
                        app.apply(event);
                        if membership_changed {
                            sync_voice(&app, voice.as_ref()).await;
                            if let Some(v) = voice.as_ref() {
                                if chime.on_roster(&app, prev_voice) {
                                    v.play(Blip::Chime);
                                }
                                if let Some(blip) = own_state.on_roster(app.muted, app.deafened) {
                                    v.play(blip);
                                }
                            }
                        }
                    }
                    None => break,
                },

                // The speaking indicator comes from the audio engine.
                changed = next_speakers(speaking_rx.as_mut()) => {
                    app.speaking = changed;
                }

                // Live microphone level: our own meter and the settings screen.
                level = next_mic_level(mic_level_rx.as_mut()) => {
                    app.mic_level = level;
                    app.observe_level(level);
                    settle_calibration(&mut app, voice.as_ref());
                    if app.watch_for_feedback(level)
                        && let Some(v) = voice.as_ref()
                    {
                        v.set_mic_test(app.mic_test);
                    }
                }

                // How loud everyone else is, for the meters in the roster.
                levels = next_peer_levels(peer_levels_rx.as_mut()) => {
                    app.peer_levels = levels;
                }

                _ = quality_tick.tick() => {
                    if let Some(voice) = voice.as_ref() {
                        app.link = voice.mesh.link_status().await;
                        app.note_dropouts(voice.health.underruns());

                        // The first tick is the earliest point the roster has landed,
                        // and `Welcome` replaces the transcript wholesale — a notice
                        // pushed before it would be thrown away.
                        for note in startup_notes.drain(..) {
                            app.notice(note);
                        }
                        for news in voice.recover() {
                            let note = describe(&news);
                            if last_audio_note.as_deref() != Some(note.as_str()) {
                                app.notice(note.clone());
                                last_audio_note = Some(note);
                            }
                        }
                    }
                }

                // The only self-driven redraw: while the string is carrying a pulse
                // or a mode change is settling. With nothing moving the interface
                // sleeps until the network, the audio or a key wakes it.
                _ = tokio::time::sleep(FRAME_INTERVAL), if app.needs_animation() => {
                    settle_calibration(&mut app, voice.as_ref());
                    if app.advance_mic_test()
                        && let Some(v) = voice.as_ref()
                    {
                        v.set_mic_test(app.mic_test);
                    }
                }

                key = keys.recv() => match key {
                    Some(key) => {
                        if handle_key(&mut app, key, &session.commands, voice.as_ref()).await? {
                            break;
                        }
                        apply_local_audio_state(&app, voice.as_ref());
                    }
                    None => break,
                },
            }

            if let Some(reason) = app.ended.clone() {
                app.status = Some(reason);
                terminal.draw(|frame| view::draw(frame, &app, &theme))?;
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                break;
            }
        }
        anyhow::Ok(())
    }
    .await;

    ratatui::restore();
    drop(voice);
    if let Some(reason) = app.ended {
        println!("{reason}");
    }
    result
}

/// Reading keys blocks, so it runs on its own thread and is piped into a channel.
fn spawn_key_reader() -> mpsc::Receiver<KeyEvent> {
    let (tx, rx) = mpsc::channel(64);
    std::thread::spawn(move || {
        loop {
            match crossterm::event::read() {
                Ok(TermEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                    if tx.blocking_send(key).is_err() {
                        return;
                    }
                }
                Ok(_) => continue,
                Err(_) => return,
            }
        }
    });
    rx
}

/// Handles a key. Returns `true` if we should quit.
async fn handle_key(
    app: &mut App,
    key: KeyEvent,
    commands: &mpsc::Sender<Command>,
    voice: Option<&VoiceControl>,
) -> Result<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Global Quit
    if ctrl && key.code == KeyCode::Char('c') {
        let _ = commands.send(Command::Quit).await;
        return Ok(true);
    }

    // Toggle Settings view (F6 or Ctrl+,)
    let toggle_settings_key = key.code == KeyCode::F(6) || (ctrl && key.code == KeyCode::Char(','));
    if toggle_settings_key {
        if app.view_mode == ViewMode::Settings {
            remember_gate(app, voice);
        }
        app.toggle_settings();
        if let Some(v) = voice {
            v.play(Blip::Chime);
            v.set_mic_test(app.mic_test);
        }
        return Ok(false);
    }

    // ── Settings Mode Keyboard Handling ─────────────────────────────────────
    if app.view_mode == ViewMode::Settings {
        match key.code {
            KeyCode::Esc => {
                remember_gate(app, voice);
                app.toggle_settings();
                if let Some(v) = voice {
                    v.play(Blip::Chime);
                    v.set_mic_test(MicTest::Off);
                }
                return Ok(false);
            }
            // The gate steps by one cell of the meter it is drawn on, so a press moves
            // it exactly as far as it looks like it should.
            KeyCode::Left | KeyCode::Char('h')
                if app.settings_section == SettingsSection::MicTest =>
            {
                app.nudge_gate(-GATE_STEP);
                if let Some(v) = voice {
                    v.set_gate(app.input_gate);
                }
                return Ok(false);
            }
            KeyCode::Right | KeyCode::Char('l')
                if app.settings_section == SettingsSection::MicTest =>
            {
                app.nudge_gate(GATE_STEP);
                if let Some(v) = voice {
                    v.set_gate(app.input_gate);
                }
                return Ok(false);
            }
            KeyCode::Char('a') | KeyCode::Char('A')
                if app.settings_section == SettingsSection::MicTest =>
            {
                app.start_calibration();
                return Ok(false);
            }
            KeyCode::Tab => {
                app.settings_next_section(true);
                return Ok(false);
            }
            KeyCode::BackTab => {
                app.settings_next_section(false);
                return Ok(false);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.settings_navigate_item(false);
                return Ok(false);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.settings_navigate_item(true);
                return Ok(false);
            }
            KeyCode::Char(' ') => {
                app.toggle_recorded_test();
                if let Some(v) = voice {
                    v.set_mic_test(app.mic_test);
                }
                return Ok(false);
            }
            // Live monitoring is behind its own key because it is only safe on
            // headphones: on a laptop it closes a loop between speaker and microphone.
            KeyCode::Char('m') | KeyCode::Char('M') => {
                app.toggle_monitor();
                if let Some(v) = voice {
                    v.set_mic_test(app.mic_test);
                }
                return Ok(false);
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                app.refresh_devices();
                return Ok(false);
            }
            KeyCode::Enter => {
                match app.settings_section {
                    SettingsSection::InputDevice => {
                        if let Some(dev) = app.selected_input_device() {
                            let name = dev.name.clone();
                            if !dev.is_supported {
                                // Any rate is resampled; a device only fails here
                                // when it will not report a rate at all.
                                app.settings_error = Some(format!(
                                    "{name} is not reporting a format, so it cannot be opened"
                                ));
                            } else if let Some(v) = voice {
                                match v.switch_input(Some(&name)) {
                                    Ok(activated) => {
                                        app.active_input_name = Some(activated.clone());
                                        app.settings_error = None;
                                        let mut cfg = Config::load();
                                        // The gate belongs to the microphone, not to
                                        // the session: this one has its own floor.
                                        app.input_gate = cfg.gate_for(Some(&activated));
                                        v.set_gate(app.input_gate);
                                        cfg.input_device = Some(activated);
                                        let _ = cfg.save();
                                    }
                                    Err(err) => {
                                        app.settings_error = Some(format!("could not switch the microphone: {err:#}"));
                                    }
                                }
                            }
                        }
                    }
                    SettingsSection::OutputDevice => {
                        if let Some(dev) = app.selected_output_device() {
                            let name = dev.name.clone();
                            if !dev.is_supported {
                                // Any rate is resampled; a device only fails here
                                // when it will not report a rate at all.
                                app.settings_error = Some(format!(
                                    "{name} is not reporting a format, so it cannot be opened"
                                ));
                            } else if let Some(v) = voice {
                                match v.switch_output(Some(&name)) {
                                    Ok(activated) => {
                                        app.active_output_name = Some(activated.clone());
                                        app.settings_error = None;
                                        let mut cfg = Config::load();
                                        cfg.output_device = Some(activated);
                                        let _ = cfg.save();
                                    }
                                    Err(err) => {
                                        app.settings_error = Some(format!("could not switch the speaker: {err:#}"));
                                    }
                                }
                            }
                        }
                    }
                    SettingsSection::MicTest => {
                        app.toggle_recorded_test();
                        if let Some(v) = voice {
                            v.set_mic_test(app.mic_test);
                        }
                    }
                }
                return Ok(false);
            }
            _ => return Ok(false),
        }
    }

    // ── Chat Mode Keyboard Handling ─────────────────────────────────────────
    let toggle_voice = key.code == KeyCode::F(2) || (ctrl && key.code == KeyCode::Char('g'));
    let toggle_mute = key.code == KeyCode::F(3) || (ctrl && key.code == KeyCode::Char('t'));
    let push_to_talk = key.code == KeyCode::F(4);
    let show_code = key.code == KeyCode::F(1);
    let toggle_deafen = key.code == KeyCode::F(5);

    if show_code {
        let copied = crate::clipboard::copy(&app.invite_code);
        app.show_invite_code(copied);
        return Ok(false);
    }

    if push_to_talk && app.ptt_mode {
        app.ptt_active = !app.ptt_active;
        return Ok(false);
    }
    if toggle_deafen {
        let deafened = !app.deafened;
        let _ = commands.send(Command::SetDeafened(deafened)).await;
        if deafened && !app.muted {
            let _ = commands.send(Command::SetMuted(true)).await;
        }
        return Ok(false);
    }

    if toggle_voice {
        let target = if app.voice == Some(app.viewing) {
            None
        } else {
            Some(app.viewing)
        };
        let _ = commands.send(Command::SwitchChannel(target)).await;
        return Ok(false);
    }
    if toggle_mute {
        let _ = commands.send(Command::SetMuted(!app.muted)).await;
        return Ok(false);
    }

    match key.code {
        KeyCode::Tab => app.view_next(true),
        KeyCode::BackTab => app.view_next(false),

        KeyCode::Enter => {
            if let Some(text) = app.take_input() {
                let channel = app.viewing;
                let _ = commands.send(Command::Chat { channel, text }).await;
            }
        }

        KeyCode::Backspace => {
            app.input.pop();
        }

        KeyCode::Char(c) if !ctrl => app.input.push(c),

        _ => {}
    }
    Ok(false)
}

/// What to tell the room about a stream that was taken away and put back.
fn describe(news: &Recovered) -> String {
    match &news.device {
        Some(device) => format!("the {} changed — now on {device}", news.side.name()),
        None => format!("the {} was lost and will not reopen", news.side.name()),
    }
}

/// Ends a room measurement once its time is up, and keeps what it decided.
fn settle_calibration(app: &mut App, voice: Option<&VoiceControl>) {
    if app.finish_calibration().is_some() {
        if let Some(voice) = voice {
            voice.set_gate(app.input_gate);
        }
        remember_gate(app, voice);
    }
}

/// Writes the gate to the config, under the name of the microphone it was set for.
///
/// Called when leaving the settings screen rather than on every keypress: dragging the
/// gate across the meter is twenty-odd presses, and none of them is worth a file write.
fn remember_gate(app: &App, voice: Option<&VoiceControl>) {
    if voice.is_none() {
        return;
    }
    let Some(device) = app.active_input_name.as_deref() else {
        return;
    };
    let mut config = Config::load();
    if (config.gate_for(Some(device)) - app.input_gate).abs() < f32::EPSILON {
        return;
    }
    config.set_gate(device, app.input_gate);
    let _ = config.save();
}

/// When the roster changes, updates the voice mesh to the new membership and aligns
/// the mute state with what the coordinator reports.
async fn sync_voice(app: &App, voice: Option<&VoiceControl>) {
    let Some(voice) = voice else {
        return;
    };
    apply_local_audio_state(app, Some(voice));

    let members = match app.voice {
        Some(channel) => app.peers_in(channel).iter().map(|p| p.id).collect(),
        None => Vec::new(),
    };
    voice.mesh.set_membership(app.voice, members).await;
}

/// Passes the interface's microphone/headphone decision down to the audio engine.
fn apply_local_audio_state(app: &App, voice: Option<&VoiceControl>) {
    let Some(voice) = voice else {
        return;
    };
    voice.mic_open.store(app.mic_open(), Ordering::Relaxed);
    voice.hearing.store(!app.deafened, Ordering::Relaxed);
}

/// Waits for the next change in the speaker list.
async fn next_speakers(
    speaking: Option<&mut watch::Receiver<HashSet<PeerId>>>,
) -> HashSet<PeerId> {
    match speaking {
        Some(speaking) => {
            if speaking.changed().await.is_ok() {
                speaking.borrow_and_update().clone()
            } else {
                std::future::pending().await
            }
        }
        None => std::future::pending().await,
    }
}

/// Waits for the next change in anyone else's level.
async fn next_peer_levels(
    levels: Option<&mut watch::Receiver<HashMap<PeerId, u8>>>,
) -> HashMap<PeerId, u8> {
    match levels {
        Some(levels) => {
            if levels.changed().await.is_ok() {
                levels.borrow_and_update().clone()
            } else {
                std::future::pending().await
            }
        }
        None => std::future::pending().await,
    }
}

/// Waits for the next microphone volume level update.
async fn next_mic_level(
    mic_level: Option<&mut watch::Receiver<f32>>,
) -> f32 {
    match mic_level {
        Some(rx) => {
            if rx.changed().await.is_ok() {
                *rx.borrow_and_update()
            } else {
                std::future::pending().await
            }
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::PeerInfo;
    use std::time::Duration;

    fn speaker(seed: u8) -> HashSet<PeerId> {
        HashSet::from([PeerId([seed; 32])])
    }

    #[tokio::test]
    async fn wakes_up_when_someone_starts_speaking() {
        let (tx, mut rx) = watch::channel(HashSet::new());
        tx.send(speaker(1)).unwrap();

        let speakers = next_speakers(Some(&mut rx)).await;
        assert_eq!(speakers, speaker(1));
    }

    #[tokio::test]
    async fn does_not_spin_when_nothing_changes() {
        let (tx, mut rx) = watch::channel(HashSet::new());
        tx.send(speaker(1)).unwrap();
        next_speakers(Some(&mut rx)).await;

        let again = tokio::time::timeout(Duration::from_millis(150), next_speakers(Some(&mut rx)));
        assert!(
            again.await.is_err(),
            "must not return immediately with no change — that is a spin loop"
        );
    }

    #[tokio::test]
    async fn never_returns_when_voice_is_off() {
        let result = tokio::time::timeout(Duration::from_millis(100), next_speakers(None));
        assert!(result.await.is_err());
    }

    #[tokio::test]
    async fn peer_levels_reach_the_roster() {
        let (tx, mut rx) = watch::channel(HashMap::new());
        tx.send(HashMap::from([(PeerId([2; 32]), 3u8)])).unwrap();

        let levels = next_peer_levels(Some(&mut rx)).await;
        assert_eq!(levels.get(&PeerId([2; 32])), Some(&3));
    }

    #[tokio::test]
    async fn peer_levels_do_not_spin_when_nobody_moves() {
        let (tx, mut rx) = watch::channel(HashMap::new());
        tx.send(HashMap::from([(PeerId([2; 32]), 3u8)])).unwrap();
        next_peer_levels(Some(&mut rx)).await;

        let again = tokio::time::timeout(Duration::from_millis(150), next_peer_levels(Some(&mut rx)));
        assert!(again.await.is_err(), "an unchanged meter must not redraw the screen");
    }

    #[tokio::test]
    async fn mic_level_updates_correctly() {
        let (tx, mut rx) = watch::channel(0.0f32);
        tx.send(0.75f32).unwrap();

        let level = next_mic_level(Some(&mut rx)).await;
        assert!((level - 0.75).abs() < f32::EPSILON);
    }

    // ── Hardware that changed under us ──────────────────────────────────────

    #[test]
    fn a_reopened_stream_says_what_it_landed_on() {
        use crate::audio::device::Side;

        let back = describe(&Recovered {
            side: Side::Speaker,
            device: Some("MacBook Pro Speakers".into()),
        });
        assert!(back.contains("speaker"), "{back}");
        assert!(back.contains("MacBook Pro Speakers"), "{back}");

        let gone = describe(&Recovered { side: Side::Microphone, device: None });
        assert!(gone.contains("microphone"), "{gone}");
        assert!(gone.contains("not reopen"), "silence about a dead microphone helps nobody: {gone}");
    }

    // ── Your own microphone and ears ────────────────────────────────────────

    #[test]
    fn the_first_roster_is_a_sync_and_says_nothing() {
        let mut chime = SelfChime::default();
        assert_eq!(chime.on_roster(false, false), None);
        assert_eq!(chime.on_roster(true, false), Some(Blip::MicOff), "but the next change speaks");
    }

    #[test]
    fn arriving_already_muted_is_not_an_event() {
        let mut chime = SelfChime::default();
        assert_eq!(chime.on_roster(true, true), None);
    }

    #[test]
    fn the_microphone_says_which_way_it_went() {
        let mut chime = SelfChime::default();
        chime.on_roster(false, false);

        assert_eq!(chime.on_roster(true, false), Some(Blip::MicOff));
        assert_eq!(chime.on_roster(false, false), Some(Blip::MicOn));
    }

    #[test]
    fn shutting_your_ears_makes_one_sound_not_two() {
        let mut chime = SelfChime::default();
        chime.on_roster(false, false);

        // F5 deafens and mutes in the same breath, and the roster reports both at
        // once.
        assert_eq!(chime.on_roster(true, true), Some(Blip::EarsOff), "one action, one sound");
        assert_eq!(chime.on_roster(false, false), Some(Blip::EarsOn));
    }

    #[test]
    fn a_roster_that_changes_nothing_about_you_is_silent() {
        let mut chime = SelfChime::default();
        chime.on_roster(true, false);
        assert_eq!(chime.on_roster(true, false), None, "someone else moving is not your business");
    }

    // ── The welcome chime ───────────────────────────────────────────────────

    fn peer(seed: u8, channel: Option<u8>) -> PeerInfo {
        PeerInfo {
            id: PeerId([seed; 32]),
            name: format!("peer{seed}"),
            channel: channel.map(ChannelId),
            muted: false,
            deafened: false,
        }
    }

    fn room(peers: Vec<PeerInfo>) -> App {
        let mut app = App::new(PeerId([1; 32]), "code".into());
        app.voice = peers
            .iter()
            .find(|p| p.id == PeerId([1; 32]))
            .and_then(|p| p.channel);
        app.peers = peers;
        app
    }

    #[test]
    fn first_roster_is_silent() {
        let mut chime = JoinChime::default();
        let app = room(vec![peer(1, None), peer(2, None), peer(3, None)]);
        assert!(!chime.on_roster(&app, None));
    }

    #[test]
    fn every_newcomer_chimes_for_everyone_present() {
        let mut chime = JoinChime::default();
        let app = room(vec![peer(1, None)]);
        chime.on_roster(&app, None);

        let app = room(vec![peer(1, None), peer(2, None)]);
        assert!(chime.on_roster(&app, None), "second peer arriving must chime");

        let app = room(vec![peer(1, None), peer(2, None), peer(3, None)]);
        assert!(chime.on_roster(&app, None), "third peer arriving must chime too");
    }

    #[test]
    fn unchanged_membership_is_silent() {
        let mut chime = JoinChime::default();
        let app = room(vec![peer(1, None), peer(2, None)]);
        chime.on_roster(&app, None);

        let app = room(vec![peer(1, None), peer(2, Some(3))]);
        assert!(!chime.on_roster(&app, None), "a peer switching channel is not an arrival");
    }

    #[test]
    fn departure_is_silent() {
        let mut chime = JoinChime::default();
        let app = room(vec![peer(1, None), peer(2, None), peer(3, None)]);
        chime.on_roster(&app, None);

        let app = room(vec![peer(1, None), peer(2, None)]);
        assert!(!chime.on_roster(&app, None));
    }

    #[test]
    fn rejoining_peer_chimes_again() {
        let mut chime = JoinChime::default();
        let app = room(vec![peer(1, None), peer(2, None)]);
        chime.on_roster(&app, None);
        let app = room(vec![peer(1, None)]);
        chime.on_roster(&app, None);

        let app = room(vec![peer(1, None), peer(2, None)]);
        assert!(chime.on_roster(&app, None));
    }

    #[test]
    fn own_channel_entry_chimes() {
        let mut chime = JoinChime::default();
        let app = room(vec![peer(1, None)]);
        chime.on_roster(&app, None);

        let app = room(vec![peer(1, Some(0))]);
        assert!(chime.on_roster(&app, None), "entering a channel must chime");
    }

    #[test]
    fn own_channel_exit_is_silent() {
        let mut chime = JoinChime::default();
        let app = room(vec![peer(1, Some(0))]);
        chime.on_roster(&app, None);

        let app = room(vec![peer(1, None)]);
        assert!(!chime.on_roster(&app, Some(ChannelId(0))));
    }
}
