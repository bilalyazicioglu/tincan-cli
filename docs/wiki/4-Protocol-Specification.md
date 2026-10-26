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


---

## 4.4 Versions and What Older Peers Are Told

The control plane is versioned by its ALPN, `tincan/control/1` (`proto::ALPN`); voice uses
`tincan/voice/0`. Two versions that cannot talk must not meet, and the older one should be
told why rather than fail with a misleading error.

**Retiring a control version.** Give `ALPN` a new number and add the old one to
`RETIRED_CONTROL_ALPNS`. Endpoints keep answering the retired ALPN, but a peer arriving on
it is not let in: the room opens the control stream, sends `Rejected` with an update
notice (the version needed and the command that updates the copy the person installed),
and closes. The encoding of `Rejected` must therefore never change. Clients from 0.4.0 on
show a `Rejected` received in place of the `Challenge`; older ones report an unexpected
greeting.

**Invite forms.** What `tincan join` accepts:

| Form | Meaning |
|------|---------|
| 52 base32 characters (dashes, spaces, case ignored) | Invite code: the coordinator's public key |
| `name` | A room on n0's network, found from name and passphrase |
| `name@host`, `name@http(s)://host[:port]` | A named room on that relay |
| `host` with a dot, `host:port`, or an `http(s)://` URL | The relay's default room |
| `tincan:…` | Reserved for future forms; this version says it needs a newer tincan |

A bare host means HTTPS. A room on a relay derives its key from name and passphrase
exactly as a room on n0's network does (the default room's name is `tincan`), and every
endpoint in it resolves every other through the relay, so no address is ever published.
