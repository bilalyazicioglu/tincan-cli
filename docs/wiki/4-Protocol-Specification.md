# 4. Wire Protocol Specification

This document specifies the binary wire format for Tincan's Control Plane and Voice Plane protocols (`src/proto.rs`).

---

## 4.1 Serialization & Framing

All control plane network messages are serialized using **Postcard** (a compact binary format designed for embedded and real-time Rust systems).

- **Max Frame Size**: Control messages are capped at 64 KB (`MAX_FRAME_SIZE = 65536`).
- **Length-Prefixed Framing**: Control stream frames are prefixed with a 4-byte big-endian `u32` payload length header.

---

## 4.2 Control Plane Message Types

### Client to Host Messages (`ClientMessage`)
```rust
pub enum ClientMessage {
    AuthProof { proof: [u8; 32] },
    SwitchChannel { channel: ChannelId },
    Chat { channel: ChannelId, text: String },
    SetMuted { muted: bool },
    SetDeafened { deafened: bool },
    Leave,
}
```

### Host to Client Messages (`ServerMessage`)
```rust
pub enum ServerMessage {
    AuthChallenge { nonce: [u8; 32] },
    AuthResult { success: bool, reason: Option<String> },
    Welcome { snapshot: RoomSnapshot },
    RosterUpdate { peer: PeerState },
    PeerJoined { peer: PeerState },
    PeerLeft { id: PeerId },
    ChatBroadcast { channel: ChannelId, from: PeerId, text: String, at: u64 },
}
```

---

## 4.3 Voice Plane Datagram Format

Voice packets are sent as QUIC unreliable datagrams directly between peers.

### Datagram Packet Binary Structure

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          Sender PeerId                        |
|                            (32 bytes)                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|   ChannelId   |                Sequence Number                |
|   (1 byte)    |                   (u64)                       |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                      Opus Encoded Payload                     |
|                        (variable size)                        |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

### Packet Header Fields
- `sender`: 32-byte Ed25519 `PeerId` of the speaker.
- `channel`: 1-byte `ChannelId` (voice channel filtering).
- `sequence`: 8-byte `u64` sequence number for jitter buffer ordering.
- `payload`: Variable-length Opus mono 48 kHz frame (20ms length).

