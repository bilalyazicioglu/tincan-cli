//! The state the interface sees, and how events are applied to it.
//!
//! Kept independent of the terminal and the network: because this is a pure state
//! machine, questions like "who shows up where, which message lands in which pane" can
//! be tested.

use std::collections::{HashMap, HashSet};

use crate::net::Event;
use crate::net::voice::LinkStatus;
use crate::proto::{ChannelId, ChatLine, PeerId, PeerInfo};

/// The most lines kept in the chat pane.
const VISIBLE_HISTORY: usize = 500;

/// A line in the chat pane: either someone's message or a system notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Chat(ChatLine),
    Notice { text: String, at: u64 },
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
    /// Whether the audio hardware came up. If it did not, the interface must say so.
    pub voice_available: bool,
    /// The quality of the voice connections; refreshed periodically.
    pub link: LinkStatus,
    /// Audio dropout counter — above zero means the user heard a crackle.
    pub audio_dropouts: u64,
    pub status: Option<String>,
    /// Filled with a reason when the session ends; the interface closes once it is set.
    pub ended: Option<String>,
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
            voice_available: false,
            link: LinkStatus::default(),
            audio_dropouts: 0,
            status: None,
            ended: None,
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

    /// The lines belonging to the channel on screen. Notices show in every channel —
    /// "X joined the room" is not tied to one.
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

    /// Whether the microphone is open right now — the resultant of the mute and
    /// push-to-talk decisions.
    ///
    /// The audio engine reads a single flag. Rather than combining two states down
    /// there, the decision is made here, so the logic lives in one place and can be
    /// tested.
    pub fn mic_open(&self) -> bool {
        if self.muted {
            return false;
        }
        if self.ptt_mode {
            return self.ptt_active;
        }
        true
    }

    /// Takes the typed message and clears the input field; `None` if there is
    /// nothing to send.
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

    /// A user must be able to read one channel's chat while talking in another.
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
    fn microphone_is_open_by_default() {
        let app = welcomed();
        assert!(app.mic_open(), "in normal mode the microphone is open");
    }

    #[test]
    fn muting_closes_the_microphone_in_every_mode() {
        let mut app = welcomed();
        app.muted = true;
        assert!(!app.mic_open());

        // Even with push-to-talk engaged, muting overrides everything.
        app.ptt_mode = true;
        app.ptt_active = true;
        assert!(!app.mic_open(), "mute must override push-to-talk");
    }

    /// Push-to-talk mode must default to silence; the key is the only way to open it.
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
        // For an identity we have never seen, we have no name.
        assert_eq!(app.name_of(PeerId([9; 32])), PeerId([9; 32]).short());
    }

    /// When someone leaves, their old messages on screen must still show their name —
    /// otherwise the chat history becomes unreadable as people come and go.
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

        // user2 left: no longer in the roster.
        app.apply(Event::Roster(vec![peer(1, None)]));

        assert_eq!(
            app.name_of(PeerId([2; 32])),
            "user2",
            "a departed peer's old message must not turn into a raw id"
        );
        assert_eq!(app.peers.len(), 1, "the people list must still be updated");
    }
}
