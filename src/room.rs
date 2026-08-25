//! The room's authoritative state — the coordinator's single source of truth.
//!
//! This module is deliberately pure: no network, no clock, no audio. All of the
//! coordinator's logic is tested here; `net/control.rs` only wires this type to
//! messages.

use std::collections::{BTreeMap, VecDeque};

use anyhow::{Result, bail, ensure};

use crate::proto::{ChannelId, ChatLine, MAX_CHAT_CHARS, MAX_NAME_CHARS, PeerId, PeerInfo, RoomSnapshot};

/// How many chat lines are kept in memory and handed to a newcomer.
const CHAT_HISTORY: usize = 200;

pub struct Room {
    name: String,
    channels: Vec<String>,
    /// BTreeMap so the roster order is identical on every client (deterministic by id).
    peers: BTreeMap<PeerId, PeerInfo>,
    chat: VecDeque<ChatLine>,
}

impl Room {
    pub fn new(name: impl Into<String>, channels: Vec<String>) -> Result<Self> {
        let channels: Vec<String> = channels.into_iter().map(|c| c.trim().to_string()).collect();
        ensure!(!channels.is_empty(), "a room needs at least one channel");
        ensure!(channels.len() <= u8::MAX as usize, "too many channels");
        ensure!(channels.iter().all(|c| !c.is_empty()), "a channel name cannot be empty");
        Ok(Self {
            name: name.into(),
            channels,
            peers: BTreeMap::new(),
            chat: VecDeque::new(),
        })
    }

    pub fn channels(&self) -> &[String] {
        &self.channels
    }

    pub fn roster(&self) -> Vec<PeerInfo> {
        self.peers.values().cloned().collect()
    }

    pub fn snapshot(&self) -> RoomSnapshot {
        RoomSnapshot {
            room_name: self.name.clone(),
            channels: self.channels.clone(),
            peers: self.roster(),
            recent_chat: self.chat.iter().cloned().collect(),
        }
    }

    /// Peers in the same voice channel — this is what decides who the mesh connects to.
    pub fn peers_in_channel(&self, channel: ChannelId) -> Vec<PeerId> {
        self.peers
            .values()
            .filter(|p| p.channel == Some(channel))
            .map(|p| p.id)
            .collect()
    }

    pub fn get(&self, id: &PeerId) -> Option<&PeerInfo> {
        self.peers.get(id)
    }

    /// Joins the room. Re-joining with the same identity refreshes the record
    /// (a reconnect).
    ///
    /// Returns the final display name, which may have been altered to break a clash.
    pub fn join(&mut self, id: PeerId, requested_name: &str) -> Result<String> {
        let name = self.sanitize_name(&id, requested_name)?;
        self.peers.insert(
            id,
            PeerInfo {
                id,
                name: name.clone(),
                channel: None,
                muted: false,
                deafened: false,
            },
        );
        Ok(name)
    }

    pub fn leave(&mut self, id: &PeerId) -> Option<PeerInfo> {
        self.peers.remove(id)
    }

    pub fn switch_channel(&mut self, id: &PeerId, channel: Option<ChannelId>) -> Result<()> {
        if let Some(ChannelId(index)) = channel {
            ensure!(
                (index as usize) < self.channels.len(),
                "no such channel: {index}"
            );
        }
        let peer = self.peers.get_mut(id).ok_or_else(|| anyhow::anyhow!("you are not in the room"))?;
        peer.channel = channel;
        Ok(())
    }

    pub fn set_muted(&mut self, id: &PeerId, muted: bool) -> Result<()> {
        let peer = self.peers.get_mut(id).ok_or_else(|| anyhow::anyhow!("you are not in the room"))?;
        peer.muted = muted;
        Ok(())
    }

    pub fn set_deafened(&mut self, id: &PeerId, deafened: bool) -> Result<()> {
        let peer = self.peers.get_mut(id).ok_or_else(|| anyhow::anyhow!("you are not in the room"))?;
        peer.deafened = deafened;
        Ok(())
    }

    /// Validates a chat message, appends it to the history and returns the line to
    /// broadcast.
    ///
    /// The timestamp is supplied from outside: ordering must come from the
    /// coordinator's clock, because client clocks cannot be trusted.
    pub fn post_chat(&mut self, id: &PeerId, channel: ChannelId, text: &str, at: u64) -> Result<ChatLine> {
        ensure!(self.peers.contains_key(id), "you are not in the room");
        ensure!(
            (channel.0 as usize) < self.channels.len(),
            "no such channel: {}",
            channel.0
        );

        let text = text.trim();
        ensure!(!text.is_empty(), "an empty message cannot be sent");
        ensure!(
            text.chars().count() <= MAX_CHAT_CHARS,
            "message too long ({} characters, limit {})",
            text.chars().count(),
            MAX_CHAT_CHARS
        );

        let line = ChatLine {
            channel,
            from: *id,
            text: text.to_string(),
            at,
        };
        self.chat.push_back(line.clone());
        while self.chat.len() > CHAT_HISTORY {
            self.chat.pop_front();
        }
        Ok(line)
    }

    /// Cleans up a nickname and makes it unique within the room.
    fn sanitize_name(&self, id: &PeerId, requested: &str) -> Result<String> {
        let trimmed: String = requested
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_NAME_CHARS)
            .collect();
        let trimmed = trimmed.trim().to_string();
        if trimmed.is_empty() {
            bail!("a nickname cannot be empty");
        }

        // If someone else already uses the name, disambiguate with the short id —
        // nobody gets turned away.
        let taken = self
            .peers
            .values()
            .any(|p| p.id != *id && p.name.eq_ignore_ascii_case(&trimmed));
        Ok(if taken {
            format!("{trimmed}#{}", &id.short()[..4])
        } else {
            trimmed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> Room {
        Room::new("test room", vec!["general".into(), "gaming".into()]).unwrap()
    }

    fn id(seed: u8) -> PeerId {
        PeerId([seed; 32])
    }

    #[test]
    fn rejects_room_without_channels() {
        assert!(Room::new("x", vec![]).is_err());
        assert!(Room::new("x", vec!["  ".into()]).is_err());
    }

    #[test]
    fn joining_peer_starts_outside_voice_channels() {
        let mut room = room();
        let name = room.join(id(1), "alice").unwrap();
        assert_eq!(name, "alice");

        let roster = room.roster();
        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].channel, None, "a newcomer must not auto-join voice");
        assert!(!roster[0].muted);
    }

    #[test]
    fn duplicate_names_are_disambiguated_not_rejected() {
        let mut room = room();
        room.join(id(1), "alice").unwrap();
        let second = room.join(id(2), "Alice").unwrap();

        assert_ne!(second, "alice", "the second alice must be distinguishable");
        assert!(second.starts_with("Alice#"));
        assert_eq!(room.roster().len(), 2, "nobody gets excluded");
    }

    #[test]
    fn rejoining_same_identity_refreshes_instead_of_duplicating() {
        let mut room = room();
        room.join(id(1), "alice").unwrap();
        room.switch_channel(&id(1), Some(ChannelId(1))).unwrap();

        // The link dropped and the same identity came back under a new name.
        let name = room.join(id(1), "alice2").unwrap();
        assert_eq!(name, "alice2");
        assert_eq!(room.roster().len(), 1, "one identity must not be listed twice");
        assert_eq!(room.get(&id(1)).unwrap().channel, None, "state must be reset");
    }

    #[test]
    fn name_is_trimmed_and_capped() {
        let mut room = room();
        let long = "a".repeat(MAX_NAME_CHARS + 20);
        let name = room.join(id(1), &format!("  {long}  ")).unwrap();
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);

        assert!(room.join(id(2), "   ").is_err(), "an empty name must be rejected");
        assert!(
            room.join(id(3), "\u{7}\u{7}").is_err(),
            "control characters do not make a name"
        );
    }

    #[test]
    fn switching_to_unknown_channel_fails_without_changing_state() {
        let mut room = room();
        room.join(id(1), "alice").unwrap();
        room.switch_channel(&id(1), Some(ChannelId(0))).unwrap();

        assert!(room.switch_channel(&id(1), Some(ChannelId(9))).is_err());
        assert_eq!(
            room.get(&id(1)).unwrap().channel,
            Some(ChannelId(0)),
            "a failed switch must not disturb the current channel"
        );
    }

    #[test]
    fn channel_membership_drives_the_voice_mesh() {
        let mut room = room();
        room.join(id(1), "a").unwrap();
        room.join(id(2), "b").unwrap();
        room.join(id(3), "c").unwrap();
        room.switch_channel(&id(1), Some(ChannelId(0))).unwrap();
        room.switch_channel(&id(2), Some(ChannelId(0))).unwrap();
        room.switch_channel(&id(3), Some(ChannelId(1))).unwrap();

        assert_eq!(room.peers_in_channel(ChannelId(0)), vec![id(1), id(2)]);
        assert_eq!(room.peers_in_channel(ChannelId(1)), vec![id(3)]);

        // A peer leaving the channel must drop out of the mesh.
        room.switch_channel(&id(2), None).unwrap();
        assert_eq!(room.peers_in_channel(ChannelId(0)), vec![id(1)]);
    }

    #[test]
    fn leaving_removes_peer_from_roster() {
        let mut room = room();
        room.join(id(1), "alice").unwrap();
        assert!(room.leave(&id(1)).is_some());
        assert!(room.roster().is_empty());
        assert!(room.leave(&id(1)).is_none(), "a second leave is ignored quietly");
    }

    #[test]
    fn strangers_cannot_act() {
        let mut room = room();
        assert!(room.post_chat(&id(99), ChannelId(0), "hello", 1).is_err());
        assert!(room.switch_channel(&id(99), Some(ChannelId(0))).is_err());
        assert!(room.set_muted(&id(99), true).is_err());
    }

    #[test]
    fn chat_is_validated() {
        let mut room = room();
        room.join(id(1), "alice").unwrap();

        let line = room.post_chat(&id(1), ChannelId(0), "  hey  ", 100).unwrap();
        assert_eq!(line.text, "hey", "leading/trailing space must be trimmed");
        assert_eq!(line.at, 100, "the timestamp must come from the coordinator");

        assert!(room.post_chat(&id(1), ChannelId(0), "   ", 1).is_err(), "empty message");
        assert!(room.post_chat(&id(1), ChannelId(5), "x", 1).is_err(), "no such channel");
        // Deliberately a multi-byte character: the limit counts chars, not bytes, and
        // an ASCII string here would stop testing that.
        let long = "é".repeat(MAX_CHAT_CHARS + 1);
        assert!(room.post_chat(&id(1), ChannelId(0), &long, 1).is_err(), "long message");
    }

    #[test]
    fn chat_history_is_bounded_and_ordered() {
        let mut room = room();
        room.join(id(1), "alice").unwrap();
        for i in 0..CHAT_HISTORY + 50 {
            room.post_chat(&id(1), ChannelId(0), &format!("message {i}"), i as u64).unwrap();
        }

        let history = room.snapshot().recent_chat;
        assert_eq!(history.len(), CHAT_HISTORY, "history must not grow without bound");
        assert_eq!(history.first().unwrap().text, "message 50", "the oldest drop off");
        assert_eq!(history.last().unwrap().text, format!("message {}", CHAT_HISTORY + 49));
    }

    #[test]
    fn snapshot_carries_everything_a_newcomer_needs() {
        let mut room = room();
        room.join(id(1), "alice").unwrap();
        room.switch_channel(&id(1), Some(ChannelId(1))).unwrap();
        room.post_chat(&id(1), ChannelId(0), "hello", 5).unwrap();

        let snapshot = room.snapshot();
        assert_eq!(snapshot.room_name, "test room");
        assert_eq!(snapshot.channels, vec!["general", "gaming"]);
        assert_eq!(snapshot.peers.len(), 1);
        assert_eq!(snapshot.peers[0].channel, Some(ChannelId(1)));
        assert_eq!(snapshot.recent_chat.len(), 1);
    }
}
