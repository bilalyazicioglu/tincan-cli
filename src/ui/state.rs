//! The state the interface sees, and how events are applied to it.
//!
//! Kept independent of the terminal and the network: because this is a pure state
//! machine, questions like "who shows up where, which message lands in which pane" can
//! be tested.

use std::collections::{HashMap, HashSet};

use crate::audio::device::AudioDeviceInfo;
use crate::net::Event;
use crate::net::voice::LinkStatus;
use crate::proto::{ChannelId, ChatLine, PeerId, PeerInfo};

/// The most lines kept in the chat pane.
const VISIBLE_HISTORY: usize = 500;

/// How long a dropout keeps being reported after the audio recovers.
const DROPOUT_MEMORY: std::time::Duration = std::time::Duration::from_secs(6);

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
    pub mic_test_active: bool,
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
            mic_test_active: false,
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
                self.mic_test_active = false;
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
    pub fn needs_animation(&self) -> bool {
        self.is_transitioning() || (self.motion && !self.speaking.is_empty())
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
        assert!(!app.mic_test_active);
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
