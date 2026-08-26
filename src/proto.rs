//! The control plane's on-the-wire types.
//!
//! This module is deliberately independent of iroh: identities travel as raw
//! `[u8; 32]`, so the protocol can be tested without the network layer. Conversion to
//! iroh's types lives in `net`.
//!
//! Framing: every message is a `u32` little-endian length prefix + a postcard body.

use serde::{Deserialize, Serialize};

/// The ALPN used on the control stream.
pub const ALPN: &[u8] = b"tincan/control/0";

/// The ALPN used in the voice mesh. Separate from the control plane: voice links are
/// established directly between peers and never pass through the coordinator.
pub const VOICE_ALPN: &[u8] = b"tincan/voice/0";

/// Voice packet header — the first bytes of the datagram.
///
/// The sender's identity is not written into the packet: a voice datagram already
/// arrives over a QUIC connection established with that peer and verified by public
/// key. Putting the identity in the packet would waste space and create a field that
/// could lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceHeader {
    /// Frame sequence number; the jitter buffer uses it to order frames and detect
    /// loss. At 20 ms per frame a `u32` lasts about 2.7 years, so wrap-around is a
    /// non-problem.
    pub seq: u32,
    /// Which channel is being spoken into. The receiver will not play audio from a
    /// channel other than its own — not even in the brief gap between a channel switch
    /// and the roster update that announces it.
    pub channel: ChannelId,
}

impl VoiceHeader {
    pub const SIZE: usize = 5;

    pub fn write_into(&self, buffer: &mut [u8]) {
        buffer[..4].copy_from_slice(&self.seq.to_le_bytes());
        buffer[4] = self.channel.0;
    }

    /// Splits a datagram into its header and Opus payload.
    pub fn parse(datagram: &[u8]) -> Option<(Self, &[u8])> {
        if datagram.len() <= Self::SIZE {
            return None;
        }
        let seq = u32::from_le_bytes(datagram[..4].try_into().ok()?);
        let header = Self {
            seq,
            channel: ChannelId(datagram[4]),
        };
        Some((header, &datagram[Self::SIZE..]))
    }
}

/// The largest single control message that will be accepted.
/// Chat lines are small; this limit guards against corrupt or malicious length
/// prefixes.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Maximum number of characters in a chat message.
pub const MAX_CHAT_CHARS: usize = 2000;

/// Maximum number of characters in a nickname.
pub const MAX_NAME_CHARS: usize = 24;

/// Peer identity — the raw form of an iroh public key.
///
/// Because identity and cryptographic verification are the same thing here, there are
/// no user accounts: the QUIC handshake already proves who a peer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    /// The short form shown in the interface.
    pub fn short(&self) -> String {
        self.0[..5].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Channel identity — the index into the list fixed when the room was created.
/// Channels are not added or removed at runtime in the MVP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChannelId(pub u8);

/// A peer's publicly visible state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: PeerId,
    pub name: String,
    /// `None` means the peer is in the room but in no voice channel (text only).
    pub channel: Option<ChannelId>,
    /// Microphone off — nobody can hear them.
    pub muted: bool,
    /// Headphones off — they cannot hear anyone. This is shared so that others do not
    /// talk into the void; kept purely local, nobody would ever notice.
    pub deafened: bool,
}

/// The complete state handed to a peer that has just joined the room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSnapshot {
    pub room_name: String,
    pub channels: Vec<String>,
    pub peers: Vec<PeerInfo>,
    /// Recent messages per channel (ordered, oldest → newest).
    pub recent_chat: Vec<ChatLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChannelId,
    pub from: PeerId,
    pub text: String,
    /// Unix epoch seconds, on the coordinator's clock, so ordering has one source.
    pub at: u64,
}

/// Client → coordinator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToCoordinator {
    /// Answer to the challenge: nickname + password proof.
    Hello { name: String, proof: [u8; 32] },
    /// Switch voice channel; `None` means leave voice entirely.
    SwitchChannel { channel: Option<ChannelId> },
    Chat { channel: ChannelId, text: String },
    SetMuted { muted: bool },
    SetDeafened { deafened: bool },
    /// A graceful goodbye. Without it, the coordinator finds out when the link drops.
    Leave,
}

/// Coordinator → client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToPeer {
    /// Sent as soon as the connection is up; the salt for the password proof.
    Challenge { nonce: [u8; 16] },
    Welcome { you: PeerId, room: RoomSnapshot },
    Rejected { reason: String },
    /// Any change to the roster — the full list is sent.
    ///
    /// A full list rather than a delta: in a six-person room the list is a few hundred
    /// bytes, and in exchange client state can never drift out of sync.
    Roster { peers: Vec<PeerInfo> },
    Chat(ChatLine),
    /// System lines such as "X joined the room".
    Notice { text: String },
}

/// Encodes a length-prefixed frame.
pub fn encode<T: Serialize>(message: &T) -> anyhow::Result<Vec<u8>> {
    let body = postcard::to_stdvec(message)?;
    anyhow::ensure!(
        body.len() <= MAX_MESSAGE_BYTES,
        "message too large: {} bytes",
        body.len()
    );
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

/// Decodes a body (the length prefix has already been read in the `net` layer).
pub fn decode<T: for<'de> Deserialize<'de>>(body: &[u8]) -> anyhow::Result<T> {
    Ok(postcard::from_bytes(body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_peer(seed: u8) -> PeerInfo {
        PeerInfo {
            id: PeerId([seed; 32]),
            name: format!("user{seed}"),
            channel: Some(ChannelId(1)),
            muted: false,
            deafened: false,
        }
    }

    /// Framing + postcard round-trip, in both directions of the protocol.
    #[test]
    fn frames_round_trip() {
        let messages = vec![
            ToPeer::Challenge { nonce: [7; 16] },
            ToPeer::Welcome {
                you: PeerId([1; 32]),
                room: RoomSnapshot {
                    room_name: "lobby".into(),
                    channels: vec!["general".into(), "gaming".into()],
                    peers: vec![sample_peer(1), sample_peer(2)],
                    recent_chat: vec![ChatLine {
                        channel: ChannelId(0),
                        from: PeerId([2; 32]),
                        text: "hello world".into(),
                        at: 1_700_000_000,
                    }],
                },
            },
            ToPeer::Roster { peers: vec![sample_peer(3)] },
            ToPeer::Rejected { reason: "wrong password".into() },
        ];

        for message in messages {
            let framed = encode(&message).unwrap();
            let len = u32::from_le_bytes(framed[..4].try_into().unwrap()) as usize;
            assert_eq!(len, framed.len() - 4, "length prefix must match the body");
            let decoded: ToPeer = decode(&framed[4..]).unwrap();
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn client_messages_round_trip() {
        let messages = vec![
            ToCoordinator::Hello { name: "alice".into(), proof: [9; 32] },
            ToCoordinator::SwitchChannel { channel: Some(ChannelId(2)) },
            ToCoordinator::SwitchChannel { channel: None },
            ToCoordinator::Chat { channel: ChannelId(0), text: "nice one".into() },
            ToCoordinator::SetMuted { muted: true },
            ToCoordinator::SetDeafened { deafened: true },
            ToCoordinator::Leave,
        ];

        for message in messages {
            let framed = encode(&message).unwrap();
            let decoded: ToCoordinator = decode(&framed[4..]).unwrap();
            assert_eq!(decoded, message);
        }
    }

    /// Multi-byte characters and emoji must survive the wire intact. The point of
    /// this test is the non-ASCII text — keep it that way.
    #[test]
    fn preserves_non_ascii_text() {
        let line = ChatLine {
            channel: ChannelId(0),
            from: PeerId([1; 32]),
            text: "größe 日本語 🎧 ok".into(),
            at: 42,
        };
        let framed = encode(&line).unwrap();
        let decoded: ChatLine = decode(&framed[4..]).unwrap();
        assert_eq!(decoded.text, "größe 日本語 🎧 ok");
    }

    #[test]
    fn rejects_oversized_message() {
        let huge = ToCoordinator::Chat {
            channel: ChannelId(0),
            text: "a".repeat(MAX_MESSAGE_BYTES + 1),
        };
        assert!(encode(&huge).is_err());
    }

    #[test]
    fn voice_header_round_trips() {
        let header = VoiceHeader {
            seq: 123_456,
            channel: ChannelId(2),
        };
        let payload = [9u8; 80];

        let mut datagram = vec![0u8; VoiceHeader::SIZE + payload.len()];
        header.write_into(&mut datagram);
        datagram[VoiceHeader::SIZE..].copy_from_slice(&payload);

        let (parsed, body) = VoiceHeader::parse(&datagram).unwrap();
        assert_eq!(parsed, header);
        assert_eq!(body, payload);
    }

    /// A corrupt or payload-free datagram must be ignored quietly — the audio path
    /// cannot panic.
    #[test]
    fn voice_header_rejects_undersized_datagrams() {
        assert!(VoiceHeader::parse(&[]).is_none());
        assert!(VoiceHeader::parse(&[1, 2, 3]).is_none());
        // A full header but no payload: nothing to play, so this is invalid too.
        assert!(VoiceHeader::parse(&[0; VoiceHeader::SIZE]).is_none());
    }

    #[test]
    fn short_id_is_stable_and_readable() {
        let id = PeerId([0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(id.short(), "abcdef0123");
        assert_eq!(id.to_string().len(), 64);
    }
}
