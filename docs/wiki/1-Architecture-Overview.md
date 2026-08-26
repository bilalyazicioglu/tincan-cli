# 1. Architecture Overview

Tincan CLI is built from the ground up to provide serverless multi-channel voice and text communication inside standard terminal emulators.

---

## 1.1 Dual-Plane Design Philosophy

Centralized communications systems (Discord, Teams, Slack) rely on server infrastructure to route all traffic. Tincan decouples communication into two independent network planes:

```
                  ┌─────────────────────────────────┐
                  │    Coordinator (Room Host)      │
                  └────────┬───────────────┬────────┘
                           │               │
            Control Stream │               │ Control Stream
            (Reliable QUIC)│               │ (Reliable QUIC)
                           │               │
                   ┌───────▼───────┐       ▼───────┐
                   │    Peer B     │◄─────►│Peer C │
                   └───────────────┘       └───────┘
                     Direct Voice Mesh (Datagrams)
```

### Control Plane (Star Topology)
- **Host / Coordinator**: The first user who creates a room via `tincan host` acts as the room coordinator.
- **Responsibility**: Authoritative room state, roster management, channel definition, password admission verification, and text chat message broadcast.
- **Protocol**: Reliable QUIC streams using Postcard binary serialization (`src/proto.rs`, `src/net/control.rs`).
- **Traffic**: Tiny footprint (~300 B/s per client).

### Voice Plane (Full Mesh Topology)
- **Peers**: Every client in a voice channel connects directly to all other peers in that same channel.
- **Responsibility**: Real-time Opus audio transmission.
- **Protocol**: QUIC unreliable datagrams (`src/net/voice.rs`).
- **Coordinator Isolation**: **Voice payloads never pass through the coordinator**. The host is not an audio relay or bottleneck.

---

## 1.2 Room State Machine & Roster Sync

The room state is managed by the `Room` struct (`src/room.rs`).

1. **State Initialization**: Host creates a room with custom or default channels (`general`, `gaming`, `music`).
2. **Peer Join Sequence**:
   - Joining client connects over Iroh QUIC endpoint.
   - Host sends password challenge nonce (`Argon2id`).
   - Client sends proof.
   - Host validates proof, assigns a unique `PeerId`, and returns a `Welcome` snapshot containing channels, roster, and chat history.
3. **Roster Convergence**: When a peer switches channels, mutes, or deafens, a `RosterUpdate` notice is broadcast to all participants.

---

## 1.3 Bandwidth & Network Scaling

Voice mesh bandwidth scales with active voice participants $N$:
$$\text{Total Peer Upload} = (N - 1) \times \text{Opus Bitrate}$$

For standard 48 kHz mono speech at 24 kbps with 20ms frames:
- **2 Peers**: 24 kbps upload / 24 kbps download
- **4 Peers**: 72 kbps upload / 72 kbps download
- **6 Peers**: 120 kbps upload / 120 kbps download

For 2–8 participants, full mesh P2P delivers sub-50ms latency without server hosting costs.
