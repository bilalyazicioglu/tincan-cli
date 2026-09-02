//! The state the interface sees, and how events are applied to it.
//!
//! Kept independent of the terminal and the network: because this is a pure state
//! machine, questions like "who shows up where, which message lands in which pane" can
//! be tested.

use std::collections::{HashMap, HashSet};

use crate::audio::MicTest;
use crate::audio::device::AudioDeviceInfo;
use crate::net::Event;
use crate::net::voice::LinkStatus;
use crate::proto::{ChannelId, ChatLine, PeerId, PeerInfo};

/// The most lines kept in the chat pane.
const VISIBLE_HISTORY: usize = 500;

/// How long a dropout keeps being reported after the audio recovers.
const DROPOUT_MEMORY: std::time::Duration = std::time::Duration::from_secs(6);

/// How long we listen to the room before deciding where its floor is. Long enough to
/// catch a fan coming round, short enough that nobody wanders off.
const CALIBRATION: std::time::Duration = std::time::Duration::from_millis(1500);
/// How far above the loudest thing we heard the gate is placed. About 2.6 dB on the
/// meter's scale: enough that the room stays under it, little enough that a quiet
/// voice still gets over.
const CALIBRATION_MARGIN: f32 = 0.06;
/// Above this the gate would start eating the voice it is supposed to protect.
const GATE_CEILING: f32 = 0.9;

/// A level this high is either a shout or a feedback loop.
const RUNAWAY_LEVEL: f32 = 0.9;
/// How long it has to stay there before we call it feedback. Speech has gaps between
/// syllables; a room howling into its own microphone does not, and that is the one
/// signature that separates them.
const RUNAWAY_FOR: std::time::Duration = std::time::Duration::from_millis(700);
/// How many cells the level meter is drawn in. Lives here rather than in the view
/// because the gate steps by exactly one cell — what you press and what you see have
/// to be the same distance.
pub const METER_CELLS: usize = 28;
/// One cell of the meter.
pub const GATE_STEP: f32 = 1.0 / METER_CELLS as f32;

/// A measurement of the room in progress.
#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    until: std::time::Instant,
    /// The loudest the room got while we listened.
    peak: f32,
}

/// A line in the chat pane: either someone's message or a system notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Chat(ChatLine),
    Notice { text: String, at: u64 },
}

/// The active top-level screen mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Chat,
    Settings,
}

/// Focusable section within the Settings screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsSection {
    #[default]
    InputDevice,
    OutputDevice,
    MicTest,
}

pub struct App {
    pub me: PeerId,
    pub room_name: String,
    pub invite_code: String,
    pub channels: Vec<String>,
    pub peers: Vec<PeerInfo>,
    /// The name of every identity we have ever seen. This does not shrink when the
    /// roster does, so old messages in the history keep their author's name.
    names: HashMap<PeerId, String>,
    pub lines: Vec<Line>,
    /// The channel on screen — a typed message goes here.
    pub viewing: ChannelId,
    /// The channel we are connected to by voice. Independent of the one being viewed:
    /// you can be talking in "gaming" while reading the chat in "general".
    pub voice: Option<ChannelId>,
    pub muted: bool,
    /// Deafened: we hear nobody. Comes from the roster, so everyone can see it.
    pub deafened: bool,
    /// Whether push-to-talk mode is on (`--ptt`).
    pub ptt_mode: bool,
    /// Whether the push-to-talk key is currently active.
    pub ptt_active: bool,
    pub input: String,
    /// Who is currently speaking — comes from the audio engine, shown in the people
    /// list.
    pub speaking: HashSet<PeerId>,
    /// How loud each of the others is, 0-4, for the meters beside their names.
    pub peer_levels: HashMap<PeerId, u8>,
    /// Where the microphone's noise floor sits, as a position on the level meter.
    /// Anything quieter than this never leaves the machine.
    pub input_gate: f32,
    /// A measurement of the room, if one is running.
    pub calibrating: Option<Calibration>,
    /// Whether the string may animate. Off under reduced motion.
    pub motion: bool,
    /// When the interface opened. The string's pulse runs on this clock, so it keeps
    /// travelling at the same speed no matter how often the screen is redrawn.
    pub started: std::time::Instant,
    /// Whether the audio hardware came up. If it did not, the interface must say so.
    pub voice_available: bool,
    /// The quality of the voice connections; refreshed periodically.
    pub link: LinkStatus,
    /// Audio dropout counter — above zero means the user heard a crackle. It only
    /// ever climbs, so on its own it cannot say whether the trouble is now or was an
    /// hour ago; `dropped_at` is what answers that.
    pub audio_dropouts: u64,
    dropped_at: Option<std::time::Instant>,
    pub status: Option<String>,
    /// Filled with a reason when the session ends; the interface closes once it is set.
    pub ended: Option<String>,

    // ── Settings & Audio Device State ───────────────────────────────────────
    pub view_mode: ViewMode,
    pub settings_section: SettingsSection,
    pub input_devices: Vec<AudioDeviceInfo>,
    pub output_devices: Vec<AudioDeviceInfo>,
    pub selected_input_idx: usize,
    pub selected_output_idx: usize,
    pub active_input_name: Option<String>,
    pub active_output_name: Option<String>,
    pub mic_level: f32,
    /// What the microphone test is doing.
    pub mic_test: MicTest,
    /// When the current stage of the test runs out, for the stages that are timed.
    pub mic_test_until: Option<std::time::Instant>,
    /// Set when live monitoring was cut off because it started feeding back.
    pub fed_back: bool,
    /// Since when the input has been pinned at the top, if it has.
    loud_since: Option<std::time::Instant>,
    pub settings_error: Option<String>,
    pub mode_transition_at: Option<std::time::Instant>,
}

impl App {
    pub fn new(me: PeerId, invite_code: String) -> Self {
        Self {
            me,
            room_name: String::new(),
            invite_code,
            channels: Vec::new(),
            peers: Vec::new(),
            names: HashMap::new(),
            lines: Vec::new(),
            viewing: ChannelId(0),
            voice: None,
            muted: false,
            deafened: false,
            ptt_mode: false,
            ptt_active: false,
            input: String::new(),
            speaking: HashSet::new(),
            peer_levels: HashMap::new(),
            input_gate: crate::config::DEFAULT_GATE,
            calibrating: None,
            motion: true,
            started: std::time::Instant::now(),
            voice_available: false,
            link: LinkStatus::default(),
            audio_dropouts: 0,
            dropped_at: None,
            status: None,
            ended: None,

            view_mode: ViewMode::Chat,
            settings_section: SettingsSection::InputDevice,
            input_devices: Vec::new(),
            output_devices: Vec::new(),
            selected_input_idx: 0,
            selected_output_idx: 0,
            active_input_name: None,
            active_output_name: None,
            mic_level: 0.0,
            mic_test: MicTest::Off,
            mic_test_until: None,
            fed_back: false,
            loud_since: None,
            settings_error: None,
            mode_transition_at: None,
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Welcome { me, room } => {
                self.me = me;
                self.room_name = room.room_name;
                self.channels = room.channels;
                self.peers = room.peers;
                self.lines = room.recent_chat.into_iter().map(Line::Chat).collect();
                self.remember_names();
                // Our own state comes from the server's list, so it stays right
                // across a reconnect too.
                self.sync_self_from_roster();
            }
            Event::Roster(peers) => {
                self.peers = peers;
                self.remember_names();
                self.sync_self_from_roster();
            }
            Event::Chat(line) => self.push(Line::Chat(line)),
            Event::Notice(text) => {
                let at = crate::net::now();
                self.push(Line::Notice { text, at });
            }
            Event::Disconnected(reason) => self.ended = Some(reason),
        }
    }

    /// Puts the full invite code back on screen.
    pub fn show_invite_code(&mut self, copied: bool) {
        let text = if copied {
            format!("invite code (copied to clipboard): {}", self.invite_code)
        } else {
            format!("invite code: {}", self.invite_code)
        };
        let at = crate::net::now();
        self.push(Line::Notice { text, at });
    }

    fn push(&mut self, line: Line) {
        self.lines.push(line);
        if self.lines.len() > VISIBLE_HISTORY {
            self.lines.drain(..self.lines.len() - VISIBLE_HISTORY);
        }
    }

    fn remember_names(&mut self) {
        for peer in &self.peers {
            self.names.insert(peer.id, peer.name.clone());
        }
    }

    /// Aligns the voice/mute state with what the coordinator reports.
    fn sync_self_from_roster(&mut self) {
        if let Some(me) = self.peers.iter().find(|p| p.id == self.me) {
            self.voice = me.channel;
            self.muted = me.muted;
            self.deafened = me.deafened;
        }
    }

    /// The lines belonging to the channel on screen. Notices show in every channel.
    pub fn visible_lines(&self) -> Vec<&Line> {
        self.lines
            .iter()
            .filter(|line| match line {
                Line::Chat(chat) => chat.channel == self.viewing,
                Line::Notice { .. } => true,
            })
            .collect()
    }

    pub fn peers_in(&self, channel: ChannelId) -> Vec<&PeerInfo> {
        self.peers
            .iter()
            .filter(|p| p.channel == Some(channel))
            .collect()
    }

    pub fn name_of(&self, id: PeerId) -> String {
        self.names.get(&id).cloned().unwrap_or_else(|| id.short())
    }

    pub fn channel_name(&self, channel: ChannelId) -> &str {
        self.channels
            .get(channel.0 as usize)
            .map(String::as_str)
            .unwrap_or("?")
    }

    /// Moves to the next channel (viewing only).
    pub fn view_next(&mut self, forward: bool) {
        if self.channels.is_empty() {
            return;
        }
        let count = self.channels.len() as u8;
        self.viewing = ChannelId(if forward {
            (self.viewing.0 + 1) % count
        } else {
            (self.viewing.0 + count - 1) % count
        });
    }

    /// Toggles between Chat and Settings views.
    pub fn toggle_settings(&mut self) {
        self.mode_transition_at = Some(std::time::Instant::now());
        match self.view_mode {
            ViewMode::Chat => {
                self.view_mode = ViewMode::Settings;
                self.settings_error = None;
                self.refresh_devices();
            }
            ViewMode::Settings => {
                self.view_mode = ViewMode::Chat;
                self.stop_mic_test();
                self.settings_error = None;
            }
        }
    }

    /// Takes the engine's running dropout count and notes when it last moved.
    pub fn note_dropouts(&mut self, total: u64) {
        if total > self.audio_dropouts {
            self.dropped_at = Some(std::time::Instant::now());
        }
        self.audio_dropouts = total;
    }

    /// Whether audio has broken up recently enough to still be worth reporting. A
    /// string that frays once and stays frayed for the rest of the call reports the
    /// past, not the present.
    pub fn recently_dropped(&self) -> bool {
        self.dropped_at
            .is_some_and(|at| at.elapsed() < DROPOUT_MEMORY)
    }

    /// The meter step, 0-4, to draw beside someone's name.
    ///
    /// Our own comes from the microphone rather than the network — we never hear
    /// ourselves come back — and reads zero whenever the microphone is shut, because
    /// a meter that moves while you are muted is a lie.
    pub fn level_of(&self, peer: PeerId) -> u8 {
        if peer == self.me {
            if !self.mic_open() {
                return 0;
            }
            return crate::audio::bar(self.mic_level);
        }
        self.peer_levels.get(&peer).copied().unwrap_or(0)
    }

    /// Whether anything on screen is still moving. The event loop asks before waking
    /// itself, so an idle room costs nothing.
    ///
    /// A running measurement counts even under reduced motion: the microphone level is
    /// only published when it changes, so in the silent room a measurement is *for*
    /// there may be no event left to finish it on.
    pub fn needs_animation(&self) -> bool {
        self.is_transitioning()
            || self.calibrating.is_some()
            || self.mic_test_until.is_some()
            || (self.motion && !self.speaking.is_empty())
    }

    /// Records for a few seconds and then plays it back, or stops a test already
    /// running.
    ///
    /// Recording and playing are separate stages on purpose: while the microphone is
    /// open the speaker stays silent, so there is no loop for a laptop to close.
    pub fn toggle_recorded_test(&mut self) {
        if self.mic_test == MicTest::Off {
            self.fed_back = false;
            self.mic_test = MicTest::Recording;
            self.mic_test_until =
                Some(std::time::Instant::now() + crate::audio::TEST_LENGTH);
        } else {
            self.stop_mic_test();
        }
    }

    /// Turns live monitoring on or off. Safe on headphones; on a laptop the microphone
    /// hears the speaker and the two feed each other.
    pub fn toggle_monitor(&mut self) {
        if self.mic_test == MicTest::Monitoring {
            self.stop_mic_test();
        } else {
            self.fed_back = false;
            self.loud_since = None;
            self.mic_test = MicTest::Monitoring;
            self.mic_test_until = None;
        }
    }

    pub fn stop_mic_test(&mut self) {
        self.mic_test = MicTest::Off;
        self.mic_test_until = None;
        self.loud_since = None;
    }

    /// Moves a timed test on to its next stage. Returns `true` when the stage changed,
    /// so the caller knows to tell the audio engine.
    pub fn advance_mic_test(&mut self) -> bool {
        let Some(until) = self.mic_test_until else {
            return false;
        };
        if std::time::Instant::now() < until {
            return false;
        }
        match self.mic_test {
            MicTest::Recording => {
                self.mic_test = MicTest::Playing;
                self.mic_test_until = Some(until + crate::audio::TEST_LENGTH);
                true
            }
            _ => {
                self.stop_mic_test();
                true
            }
        }
    }

    /// How long the current stage of the test has left.
    pub fn mic_test_left(&self) -> Option<std::time::Duration> {
        self.mic_test_until
            .map(|until| until.saturating_duration_since(std::time::Instant::now()))
    }

    /// Cuts live monitoring off when it starts feeding back, and returns `true` when it
    /// just did.
    ///
    /// Only monitoring can feed back: it is the one mode that opens the microphone and
    /// the speaker at the same time.
    pub fn watch_for_feedback(&mut self, level: f32) -> bool {
        if self.mic_test != MicTest::Monitoring {
            self.loud_since = None;
            return false;
        }
        if level < RUNAWAY_LEVEL {
            self.loud_since = None;
            return false;
        }
        let since = *self.loud_since.get_or_insert_with(std::time::Instant::now);
        if since.elapsed() < RUNAWAY_FOR {
            return false;
        }
        self.stop_mic_test();
        self.fed_back = true;
        true
    }

    /// Whether the microphone is currently loud enough to be sent.
    pub fn gate_open(&self) -> bool {
        self.mic_level > self.input_gate
    }

    /// Moves the noise floor by a step of the meter.
    pub fn nudge_gate(&mut self, delta: f32) {
        self.calibrating = None;
        self.input_gate = (self.input_gate + delta).clamp(0.0, GATE_CEILING);
    }

    /// Starts listening to the room to find its floor.
    pub fn start_calibration(&mut self) {
        self.calibrating = Some(Calibration {
            until: std::time::Instant::now() + CALIBRATION,
            peak: 0.0,
        });
    }

    /// Feeds the running measurement, if there is one.
    pub fn observe_level(&mut self, level: f32) {
        if let Some(calibration) = self.calibrating.as_mut()
            && std::time::Instant::now() < calibration.until
        {
            calibration.peak = calibration.peak.max(level);
        }
    }

    /// Ends the measurement once its time is up, and returns the gate it decided on.
    /// `None` while it is still listening, or when nothing is running.
    pub fn finish_calibration(&mut self) -> Option<f32> {
        let calibration = self.calibrating?;
        if std::time::Instant::now() < calibration.until {
            return None;
        }
        self.calibrating = None;
        self.input_gate = (calibration.peak + CALIBRATION_MARGIN).clamp(0.0, GATE_CEILING);
        Some(self.input_gate)
    }

    /// Returns true if a mode switch happened recently (< 180 ms).
    pub fn is_transitioning(&self) -> bool {
        self.mode_transition_at
            .map(|at| at.elapsed().as_millis() < 180)
            .unwrap_or(false)
    }

    /// Refreshes the cached list of audio devices from the host system.
    pub fn refresh_devices(&mut self) {
        if let Ok(inputs) = crate::audio::device::list_input_devices() {
            self.input_devices = inputs;
            if !self.input_devices.is_empty() {
                if let Some(active) = &self.active_input_name {
                    if let Some(idx) = self.input_devices.iter().position(|d| d.name == *active) {
                        self.selected_input_idx = idx;
                    }
                } else if let Some(default_idx) = self.input_devices.iter().position(|d| d.is_default) {
                    self.selected_input_idx = default_idx;
                }
                if self.selected_input_idx >= self.input_devices.len() {
                    self.selected_input_idx = 0;
                }
            }
        }

        if let Ok(outputs) = crate::audio::device::list_output_devices() {
            self.output_devices = outputs;
            if !self.output_devices.is_empty() {
                if let Some(active) = &self.active_output_name {
                    if let Some(idx) = self.output_devices.iter().position(|d| d.name == *active) {
                        self.selected_output_idx = idx;
                    }
                } else if let Some(default_idx) = self.output_devices.iter().position(|d| d.is_default) {
                    self.selected_output_idx = default_idx;
                }
                if self.selected_output_idx >= self.output_devices.len() {
                    self.selected_output_idx = 0;
                }
            }
        }
    }

    /// Cycles through sections in the Settings view.
    pub fn settings_next_section(&mut self, forward: bool) {
        self.settings_section = match (self.settings_section, forward) {
            (SettingsSection::InputDevice, true) => SettingsSection::OutputDevice,
            (SettingsSection::OutputDevice, true) => SettingsSection::MicTest,
            (SettingsSection::MicTest, true) => SettingsSection::InputDevice,

            (SettingsSection::InputDevice, false) => SettingsSection::MicTest,
            (SettingsSection::OutputDevice, false) => SettingsSection::InputDevice,
            (SettingsSection::MicTest, false) => SettingsSection::OutputDevice,
        };
        self.settings_error = None;
    }

    /// Navigates items within the active settings section.
    pub fn settings_navigate_item(&mut self, forward: bool) {
        match self.settings_section {
            SettingsSection::InputDevice => {
                if !self.input_devices.is_empty() {
                    let len = self.input_devices.len();
                    self.selected_input_idx = if forward {
                        (self.selected_input_idx + 1) % len
                    } else {
                        (self.selected_input_idx + len - 1) % len
                    };
                }
            }
            SettingsSection::OutputDevice => {
                if !self.output_devices.is_empty() {
                    let len = self.output_devices.len();
                    self.selected_output_idx = if forward {
                        (self.selected_output_idx + 1) % len
                    } else {
                        (self.selected_output_idx + len - 1) % len
                    };
                }
            }
            SettingsSection::MicTest => {
                self.settings_next_section(forward);
            }
        }
        self.settings_error = None;
    }

    /// Returns the currently highlighted input device.
    pub fn selected_input_device(&self) -> Option<&AudioDeviceInfo> {
        self.input_devices.get(self.selected_input_idx)
    }

    /// Returns the currently highlighted output device.
    pub fn selected_output_device(&self) -> Option<&AudioDeviceInfo> {
        self.output_devices.get(self.selected_output_idx)
    }

    /// Whether the microphone is open right now.
    pub fn mic_open(&self) -> bool {
        if self.muted {
            return false;
        }
        if self.ptt_mode {
            return self.ptt_active;
        }
        true
    }

    /// Takes the typed message and clears the input field.
    pub fn take_input(&mut self) -> Option<String> {
        let text = self.input.trim().to_string();
        self.input.clear();
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::RoomSnapshot;

    fn peer(seed: u8, channel: Option<ChannelId>) -> PeerInfo {
        PeerInfo {
            id: PeerId([seed; 32]),
            name: format!("user{seed}"),
            channel,
            muted: false,
            deafened: false,
        }
    }

    fn welcomed() -> App {
        let mut app = App::new(PeerId([1; 32]), "kod".into());
        app.apply(Event::Welcome {
            me: PeerId([1; 32]),
            room: RoomSnapshot {
                room_name: "oda".into(),
                channels: vec!["general".into(), "gaming".into(), "music".into()],
                peers: vec![peer(1, None), peer(2, Some(ChannelId(1)))],
                recent_chat: vec![],
            },
        });
        app
    }

    #[test]
    fn welcome_populates_the_room() {
        let app = welcomed();
        assert_eq!(app.room_name, "oda");
        assert_eq!(app.channels.len(), 3);
        assert_eq!(app.peers.len(), 2);
        assert_eq!(app.viewing, ChannelId(0));
        assert_eq!(app.voice, None, "sese otomatik girilmemeli");
    }

    #[test]
    fn settings_toggle_and_navigation() {
        let mut app = welcomed();
        assert_eq!(app.view_mode, ViewMode::Chat);

        app.toggle_settings();
        assert_eq!(app.view_mode, ViewMode::Settings);
        assert_eq!(app.settings_section, SettingsSection::InputDevice);

        app.settings_next_section(true);
        assert_eq!(app.settings_section, SettingsSection::OutputDevice);

        app.settings_next_section(true);
        assert_eq!(app.settings_section, SettingsSection::MicTest);

        app.settings_next_section(true);
        assert_eq!(app.settings_section, SettingsSection::InputDevice);

        app.settings_next_section(false);
        assert_eq!(app.settings_section, SettingsSection::MicTest);

        app.toggle_settings();
        assert_eq!(app.view_mode, ViewMode::Chat);
        assert_eq!(app.mic_test, MicTest::Off);
    }

    #[test]
    fn settings_item_navigation() {
        let mut app = welcomed();
        app.input_devices = vec![
            AudioDeviceInfo {
                name: "Mic 1".into(),
                sample_rate: 48000,
                channels: 1,
                is_default: true,
                is_supported: true,
            },
            AudioDeviceInfo {
                name: "Mic 2".into(),
                sample_rate: 48000,
                channels: 2,
                is_default: false,
                is_supported: true,
            },
        ];
        app.settings_section = SettingsSection::InputDevice;
        assert_eq!(app.selected_input_idx, 0);

        app.settings_navigate_item(true);
        assert_eq!(app.selected_input_idx, 1);

        app.settings_navigate_item(true);
        assert_eq!(app.selected_input_idx, 0);

        app.settings_navigate_item(false);
        assert_eq!(app.selected_input_idx, 1);
    }

    #[test]
    fn viewing_and_voice_channels_are_independent() {
        let mut app = welcomed();
        app.apply(Event::Roster(vec![
            peer(1, Some(ChannelId(2))),
            peer(2, Some(ChannelId(1))),
        ]));

        assert_eq!(app.voice, Some(ChannelId(2)), "the voice channel comes from the roster");
        assert_eq!(app.viewing, ChannelId(0), "the viewed channel must not change");

        app.view_next(true);
        assert_eq!(app.viewing, ChannelId(1));
        assert_eq!(app.voice, Some(ChannelId(2)), "browsing must not move the voice channel");
    }

    #[test]
    fn channel_view_wraps_in_both_directions() {
        let mut app = welcomed();
        app.view_next(false);
        assert_eq!(app.viewing, ChannelId(2), "must wrap backwards");
        app.view_next(true);
        assert_eq!(app.viewing, ChannelId(0), "must wrap forwards");
    }

    #[test]
    fn chat_is_filtered_by_channel_but_notices_are_not() {
        let mut app = welcomed();
        app.apply(Event::Chat(ChatLine {
            channel: ChannelId(0),
            from: PeerId([2; 32]),
            text: "general message".into(),
            at: 1,
        }));
        app.apply(Event::Chat(ChatLine {
            channel: ChannelId(1),
            from: PeerId([2; 32]),
            text: "gaming message".into(),
            at: 2,
        }));
        app.apply(Event::Notice("user2 joined the room".into()));

        let visible: Vec<String> = app
            .visible_lines()
            .iter()
            .map(|line| match line {
                Line::Chat(c) => c.text.clone(),
                Line::Notice { text, .. } => text.clone(),
            })
            .collect();
        assert_eq!(visible, vec!["general message", "user2 joined the room"]);

        app.view_next(true);
        let visible: Vec<String> = app
            .visible_lines()
            .iter()
            .map(|line| match line {
                Line::Chat(c) => c.text.clone(),
                Line::Notice { text, .. } => text.clone(),
            })
            .collect();
        assert_eq!(visible, vec!["gaming message", "user2 joined the room"]);
    }

    #[test]
    fn roster_drives_channel_membership_display() {
        let mut app = welcomed();
        assert_eq!(app.peers_in(ChannelId(1)).len(), 1);
        assert_eq!(app.peers_in(ChannelId(0)).len(), 0);

        app.apply(Event::Roster(vec![peer(1, Some(ChannelId(0))), peer(2, None)]));
        assert_eq!(app.peers_in(ChannelId(0)).len(), 1);
        assert_eq!(app.peers_in(ChannelId(1)).len(), 0);
    }

    #[test]
    fn input_is_trimmed_and_blank_input_sends_nothing() {
        let mut app = welcomed();
        app.input = "   ".into();
        assert_eq!(app.take_input(), None);
        assert!(app.input.is_empty(), "invalid input must be cleared too");

        app.input = "  selam  ".into();
        assert_eq!(app.take_input().as_deref(), Some("selam"));
    }

    #[test]
    fn history_is_bounded() {
        let mut app = welcomed();
        for i in 0..VISIBLE_HISTORY + 100 {
            app.apply(Event::Notice(format!("bildirim {i}")));
        }
        assert_eq!(app.lines.len(), VISIBLE_HISTORY);
    }

    #[test]
    fn a_dropout_is_reported_now_and_not_forever() {
        let mut app = welcomed();
        assert!(!app.recently_dropped());

        app.note_dropouts(3);
        assert!(app.recently_dropped(), "a fresh dropout must show");
        assert_eq!(app.audio_dropouts, 3);

        app.note_dropouts(3);
        assert!(
            app.recently_dropped(),
            "an unchanged total is the same dropout, still inside its window"
        );

        app.dropped_at = Some(std::time::Instant::now() - DROPOUT_MEMORY * 2);
        assert!(!app.recently_dropped(), "old trouble must stop being reported");
    }

    #[test]
    fn the_recorded_test_keeps_the_speaker_shut_while_the_microphone_is_open() {
        let mut app = welcomed();
        app.toggle_recorded_test();
        assert_eq!(app.mic_test, MicTest::Recording, "recording comes first, on its own");

        app.mic_test_until = Some(std::time::Instant::now());
        assert!(app.advance_mic_test());
        assert_eq!(app.mic_test, MicTest::Playing, "and only then does the speaker open");

        app.mic_test_until = Some(std::time::Instant::now());
        assert!(app.advance_mic_test());
        assert_eq!(app.mic_test, MicTest::Off);
        assert!(!app.advance_mic_test(), "and it stays over");
    }

    #[test]
    fn space_stops_a_test_that_is_already_running() {
        let mut app = welcomed();
        app.toggle_recorded_test();
        app.toggle_recorded_test();
        assert_eq!(app.mic_test, MicTest::Off);
        assert_eq!(app.mic_test_until, None);
    }

    #[test]
    fn monitoring_that_starts_howling_is_cut_off() {
        let mut app = welcomed();
        app.toggle_monitor();
        assert_eq!(app.mic_test, MicTest::Monitoring);

        assert!(!app.watch_for_feedback(1.0), "one loud frame is not yet a verdict");
        app.loud_since = Some(std::time::Instant::now() - RUNAWAY_FOR * 2);

        assert!(app.watch_for_feedback(1.0), "but a level that never comes down is");
        assert_eq!(app.mic_test, MicTest::Off);
        assert!(app.fed_back, "and the interface has to be able to say why");
    }

    #[test]
    fn a_loud_voice_with_gaps_in_it_is_not_feedback() {
        let mut app = welcomed();
        app.toggle_monitor();
        app.loud_since = Some(std::time::Instant::now() - RUNAWAY_FOR * 2);

        // A syllable ends. Speech does that; a howling room does not.
        assert!(!app.watch_for_feedback(0.2));
        assert!(!app.watch_for_feedback(1.0), "and the clock starts over");
        assert_eq!(app.mic_test, MicTest::Monitoring);
    }

    #[test]
    fn the_recorded_test_cannot_feed_back_by_definition() {
        let mut app = welcomed();
        app.toggle_recorded_test();
        app.loud_since = Some(std::time::Instant::now() - RUNAWAY_FOR * 2);
        assert!(
            !app.watch_for_feedback(1.0),
            "there is no loop to cut: the speaker is shut while the microphone is open"
        );
        assert_eq!(app.mic_test, MicTest::Recording);
    }

    #[test]
    fn the_gate_cannot_be_pushed_off_either_end_of_the_meter() {
        let mut app = welcomed();
        for _ in 0..100 {
            app.nudge_gate(-0.05);
        }
        assert_eq!(app.input_gate, 0.0, "the bottom of the meter means never gate");

        for _ in 0..100 {
            app.nudge_gate(0.05);
        }
        assert_eq!(app.input_gate, GATE_CEILING, "and it must never eat the voice entirely");
    }

    #[test]
    fn the_gate_says_whether_anything_is_getting_through() {
        let mut app = welcomed();
        app.input_gate = 0.3;

        app.mic_level = 0.1;
        assert!(!app.gate_open(), "room noise stays home");
        app.mic_level = 0.5;
        assert!(app.gate_open());
    }

    #[test]
    fn measuring_the_room_settles_above_the_loudest_thing_it_heard() {
        let mut app = welcomed();
        app.start_calibration();
        assert!(app.needs_animation(), "a measurement has to keep the loop awake");
        assert_eq!(app.finish_calibration(), None, "it is still listening");

        for level in [0.05, 0.22, 0.11] {
            app.observe_level(level);
        }
        // Run the clock out rather than sleeping.
        app.calibrating.as_mut().unwrap().until = std::time::Instant::now();

        let gate = app.finish_calibration().expect("its time is up");
        assert!((gate - (0.22 + CALIBRATION_MARGIN)).abs() < 1e-6, "settled at {gate}");
        assert_eq!(app.input_gate, gate);
        assert!(app.calibrating.is_none(), "and it is over");
        assert_eq!(app.finish_calibration(), None, "it does not fire twice");
    }

    #[test]
    fn a_level_arriving_after_the_measurement_is_not_counted() {
        let mut app = welcomed();
        app.start_calibration();
        app.observe_level(0.1);
        app.calibrating.as_mut().unwrap().until = std::time::Instant::now();

        app.observe_level(0.9);
        assert!(
            app.finish_calibration().unwrap() < 0.9,
            "someone talking after the window must not set the floor"
        );
    }

    #[test]
    fn touching_the_gate_by_hand_calls_off_a_measurement() {
        let mut app = welcomed();
        app.start_calibration();
        app.nudge_gate(0.05);
        assert!(app.calibrating.is_none(), "the user overruled it");
    }

    #[test]
    fn our_own_meter_reads_the_microphone_and_falls_silent_when_muted() {
        let mut app = welcomed();
        app.mic_level = 1.0;
        assert_eq!(app.level_of(app.me), 4);

        app.muted = true;
        assert_eq!(app.level_of(app.me), 0, "a muted meter must not move");
    }

    #[test]
    fn everyone_elses_meter_comes_from_the_audio_engine() {
        let mut app = welcomed();
        let bob = PeerId([2; 32]);
        assert_eq!(app.level_of(bob), 0, "someone we have not heard is quiet");

        app.peer_levels.insert(bob, 3);
        assert_eq!(app.level_of(bob), 3);
    }

    #[test]
    fn a_still_room_asks_for_no_redraws() {
        let mut app = welcomed();
        assert!(!app.needs_animation(), "nothing is moving, so nothing should wake the loop");

        app.speaking.insert(PeerId([2; 32]));
        assert!(app.needs_animation(), "a travelling pulse needs frames");

        app.motion = false;
        assert!(!app.needs_animation(), "reduced motion must stop the frames, not just the pulse");
    }

    #[test]
    fn microphone_is_open_by_default() {
        let app = welcomed();
        assert!(app.mic_open(), "in normal mode the microphone is open");
    }

    #[test]
    fn muting_closes_the_microphone_in_every_mode() {
        let mut app = welcomed();
        app.muted = true;
        assert!(!app.mic_open());

        app.ptt_mode = true;
        app.ptt_active = true;
        assert!(!app.mic_open(), "mute must override push-to-talk");
    }

    #[test]
    fn push_to_talk_keeps_the_microphone_shut_until_pressed() {
        let mut app = welcomed();
        app.ptt_mode = true;
        assert!(!app.mic_open(), "nothing may be transmitted before the key is pressed");

        app.ptt_active = true;
        assert!(app.mic_open());

        app.ptt_active = false;
        assert!(!app.mic_open(), "must close when the key is released");
    }

    #[test]
    fn deafened_state_comes_from_the_roster() {
        let mut app = welcomed();
        assert!(!app.deafened);

        let mut me = peer(1, None);
        me.deafened = true;
        app.apply(Event::Roster(vec![me]));
        assert!(app.deafened, "the deafened flag comes from the coordinator");
    }

    #[test]
    fn disconnect_ends_the_session() {
        let mut app = welcomed();
        assert!(app.ended.is_none());
        app.apply(Event::Disconnected("the room closed".into()));
        assert_eq!(app.ended.as_deref(), Some("the room closed"));
    }

    #[test]
    fn unknown_sender_falls_back_to_short_id() {
        let app = welcomed();
        assert_eq!(app.name_of(PeerId([2; 32])), "user2");
        assert_eq!(app.name_of(PeerId([9; 32])), PeerId([9; 32]).short());
    }

    #[test]
    fn invite_code_can_be_brought_back_in_full() {
        let mut app = App::new(PeerId([1; 32]), "n73w-kuqc-uog2-abcd".into());
        app.channels = vec!["general".into()];

        app.show_invite_code(false);
        let shown = last_notice(&app);
        assert!(shown.contains("n73w-kuqc-uog2-abcd"), "{shown}");
        assert!(!shown.contains("clipboard"), "{shown}");

        app.show_invite_code(true);
        assert!(last_notice(&app).contains("clipboard"));
    }

    fn last_notice(app: &App) -> String {
        match app.visible_lines().last().unwrap() {
            Line::Notice { text, .. } => text.clone(),
            other => panic!("expected a notice, got {other:?}"),
        }
    }

    #[test]
    fn names_survive_after_a_peer_leaves() {
        let mut app = welcomed();
        app.apply(Event::Chat(ChatLine {
            channel: ChannelId(0),
            from: PeerId([2; 32]),
            text: "ben gidiyorum".into(),
            at: 1,
        }));
        assert_eq!(app.name_of(PeerId([2; 32])), "user2");

        app.apply(Event::Roster(vec![peer(1, None)]));

        assert_eq!(
            app.name_of(PeerId([2; 32])),
            "user2",
            "a departed peer's old message must not turn into a raw id"
        );
        assert_eq!(app.peers.len(), 1, "the people list must still be updated");
    }
}
