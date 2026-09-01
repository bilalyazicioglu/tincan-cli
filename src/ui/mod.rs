//! The terminal interface.

pub mod state;
mod view;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::{mpsc, watch};

use crate::audio::device::{AudioDevices, AudioHealth};
use crate::config::Config;
use crate::net::voice::VoiceMesh;
use crate::net::{Command, Event, Session};
use crate::proto::{ChannelId, PeerId};
use state::{App, SettingsSection, ViewMode};

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
    /// Whether the microphone loopback test is active.
    pub mic_loopback: Arc<AtomicBool>,
    pub health: Arc<AudioHealth>,
    pub blip_tx: mpsc::Sender<()>,
    /// The audio hardware stays open for as long as this is kept alive.
    pub devices: AudioDevices,
}

impl VoiceControl {
    pub fn play_blip(&self) {
        let _ = self.blip_tx.try_send(());
    }

    pub fn switch_input(&self, wanted: Option<&str>) -> Result<String> {
        self.devices.switch_input(wanted)
    }

    pub fn switch_output(&self, wanted: Option<&str>) -> Result<String> {
        self.devices.switch_output(wanted)
    }

    pub fn set_loopback(&self, active: bool) {
        self.mic_loopback.store(active, Ordering::Relaxed);
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
    let mut app = App::new(session.me, session.invite_code.clone());
    app.voice_available = voice.is_some();
    app.ptt_mode = ptt_mode && app.voice_available;
    let mut terminal = ratatui::init();
    let mut keys = spawn_key_reader();
    let mut chime = JoinChime::default();
    let mut quality_tick = tokio::time::interval(std::time::Duration::from_secs(1));

    let mut speaking_rx = voice.as_ref().map(|v| v.speaking.clone());
    let mut mic_level_rx = voice.as_ref().map(|v| v.mic_level.clone());

    let result = async {
        loop {
            terminal.draw(|frame| view::draw(frame, &app))?;

            tokio::select! {
                event = session.events.recv() => match event {
                    Some(event) => {
                        let membership_changed =
                            matches!(event, Event::Roster(_) | Event::Welcome { .. });
                        let prev_voice = app.voice;
                        app.apply(event);
                        if membership_changed {
                            sync_voice(&app, voice.as_ref()).await;
                            if chime.on_roster(&app, prev_voice)
                                && let Some(v) = voice.as_ref()
                            {
                                v.play_blip();
                            }
                        }
                    }
                    None => break,
                },

                // The speaking indicator comes from the audio engine.
                changed = next_speakers(speaking_rx.as_mut()) => {
                    app.speaking = changed;
                }

                // Live microphone level for Settings VU meter.
                level = next_mic_level(mic_level_rx.as_mut()) => {
                    app.mic_level = level;
                }

                _ = quality_tick.tick() => {
                    if let Some(voice) = voice.as_ref() {
                        app.link = voice.mesh.link_status().await;
                        app.audio_dropouts = voice.health.underruns();
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
                terminal.draw(|frame| view::draw(frame, &app))?;
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
        app.toggle_settings();
        if let Some(v) = voice {
            v.set_loopback(app.mic_test_active);
        }
        return Ok(false);
    }

    // ── Settings Mode Keyboard Handling ─────────────────────────────────────
    if app.view_mode == ViewMode::Settings {
        match key.code {
            KeyCode::Esc => {
                app.toggle_settings();
                if let Some(v) = voice {
                    v.set_loopback(false);
                }
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
                let active = !app.mic_test_active;
                app.mic_test_active = active;
                if let Some(v) = voice {
                    v.set_loopback(active);
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
                                app.settings_error = Some(format!(
                                    "'{}' runs at {} Hz, but 48000 Hz is required",
                                    name, dev.sample_rate
                                ));
                            } else if let Some(v) = voice {
                                match v.switch_input(Some(&name)) {
                                    Ok(activated) => {
                                        app.active_input_name = Some(activated.clone());
                                        app.settings_error = None;
                                        let mut cfg = Config::load();
                                        cfg.input_device = Some(activated);
                                        let _ = cfg.save();
                                    }
                                    Err(err) => {
                                        app.settings_error = Some(format!("Microphone switch failed: {err:#}"));
                                    }
                                }
                            }
                        }
                    }
                    SettingsSection::OutputDevice => {
                        if let Some(dev) = app.selected_output_device() {
                            let name = dev.name.clone();
                            if !dev.is_supported {
                                app.settings_error = Some(format!(
                                    "'{}' runs at {} Hz, but 48000 Hz is required",
                                    name, dev.sample_rate
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
                                        app.settings_error = Some(format!("Speaker switch failed: {err:#}"));
                                    }
                                }
                            }
                        }
                    }
                    SettingsSection::MicTest => {
                        let active = !app.mic_test_active;
                        app.mic_test_active = active;
                        if let Some(v) = voice {
                            v.set_loopback(active);
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
    async fn mic_level_updates_correctly() {
        let (tx, mut rx) = watch::channel(0.0f32);
        tx.send(0.75f32).unwrap();

        let level = next_mic_level(Some(&mut rx)).await;
        assert!((level - 0.75).abs() < f32::EPSILON);
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
