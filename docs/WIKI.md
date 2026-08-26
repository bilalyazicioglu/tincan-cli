# Tincan CLI Wiki & Technical Architecture Guide

Welcome to the **Tincan CLI** official technical documentation and architecture wiki. Tincan is a serverless, peer-to-peer (P2P) voice and text chat client designed for the terminal. It provides Discord-like multi-channel functionality without relying on centralized servers, user accounts, or third-party infrastructure.

---

## 1. Vision & Core Philosophy

Traditional group communication tools (Discord, Teams, Slack) depend entirely on centralized server topologies. Tincan fundamentally redesigns terminal communication by combining **Iroh P2P networking**, **QUIC transport**, **Opus audio coding**, and a **Ratatui terminal user interface (TUI)** into a single, self-contained Rust binary.

Key tenets:
- **Zero Server Infrastructure**: The room creator (host) acts as a lightweight control-plane coordinator. Voice payload flows strictly P2P between peers.
- **Zero Configuration NAT Traversal**: Relies on QUIC hole-punching and DERP relays via Iroh. No port forwarding or VPNs required.
- **Privacy & Admission Control**: Challenge-response authentication (Argon2id + Nonce) prevents unauthorized entry while preserving end-to-end QUIC encryption.
- **Resource Efficiency**: Written in modern Rust, consuming minimal CPU and memory footprints.

---

## 2. System Architecture

Tincan separates network traffic into two decoupled planes:

```
        ┌─────────────────────────────────────────┐
        │        alice (Room Coordinator)         │
        └───────┬─────────────────────────┬───────┘
                │                         │
      Control Stream (TCP-like QUIC)      │ Control Stream
                │                         │
        ┌───────▼───────┐         ┌───────▼───────┐
        │      bob      │◄───────►│     carol     │
        └───────────────┘         └───────────────┘
                 Direct P2P Voice Datagram Mesh
```

### 2.1 Control Plane (Star Topology)
- **Role**: Room authorization, peer discovery, text chat broadcast, user channel presence.
- **Host / Coordinator**: The first peer who initializes `tincan host` hosts the room state.
- **Protocol**: Reliable QUIC streams using `postcard` binary serialization (`src/proto.rs`).
- **Bandwidth**: Minimal (~300 B/s per client).

### 2.2 Voice Plane (Full Mesh Topology)
- **Role**: Low-latency, real-time voice delivery.
- **Protocol**: QUIC unreliable datagrams (`src/net/voice.rs`).
- **Flow**: Each peer in a voice channel sends Opus packets directly to all other peers in that same channel.
- **Scalability**: Designed for 2–8 participants per channel (~160 kbps upload/download per peer for a 6-person room).

---

## 3. Audio Engine & Signal Flow

Tincan's audio pipeline (`src/audio/`) handles low-latency capture, compression, loss concealment, and playback.

```
 [ Microphone ] ──► [ cpal Host ] ──► [ rtrb Lock-free Buffer ] ──► [ Opus Encoder ]
                                                                          │
                                                                 (QUIC Datagram Mesh)
                                                                          │
 [ Speakers ] ◄─── [ Audio Mixer ] ◄─── [ Jitter Buffer ] ◄─── [ Opus Decoder ]
```

### 3.1 Components
1. **Audio Driver Bridge (`src/audio/device.rs`)**: Connects system audio backends (CoreAudio, ALSA, WASAPI) via `cpal` to lock-free ring buffers (`rtrb`).
2. **Codec & Packet Loss Concealment (`src/audio/codec.rs`)**: Encodes PCM audio to 48 kHz Opus frames (20ms length). Features built-in Packet Loss Concealment (PLC) for dropped frames.
3. **Voice Activity Detection (`src/audio/vad.rs`)**: Calculates frame energy levels to suppress transmission during silence (DTX), saving bandwidth.
4. **Adaptive Jitter Buffer (`src/audio/jitter.rs`)**: Smooths out network timing jitter and reorders out-of-order packets.
5. **Multi-Source Audio Mixer (`src/audio/mixer.rs`)**: Combines decoded audio from multiple peers with a soft limiter to prevent audio clipping.

---

## 4. Security & Cryptography

Security in Tincan operates on two layers:

### 4.1 Admission Control (Argon2id Nonce Challenge)
When joining a password-protected room:
1. Host sends a cryptographically random 32-byte nonce over QUIC TLS.
2. Joining client derives key: `Argon2id(password, nonce)`.
3. Client returns key proof to host for verification (`src/auth.rs`).
4. Prevents replay attacks and zero-knowledge password leak.

### 4.2 Transport Layer Security
All data (Control Streams and Voice Datagrams) is encrypted end-to-end via **QUIC TLS 1.3** backed by Iroh's public key identity (`Ed25519`).

---

## 5. Development & Project Roadmap

### Build Requirements
- Rust 1.91+
- System Opus library (`libopus-dev` on Linux, `opus` via brew on macOS)

### Running Tests
```bash
cargo test
cargo clippy --all-targets
```

### Global Open-Source Roadmap
- **v0.2.0**: Leader failover protocol, Windows WASAPI support, Termux audio backend.
- **v0.3.0**: Acoustic Echo Cancellation (AEC), RNNoise background noise reduction, TUI volume sliders.
- **v0.4.0**: SFU/Relay fallback mode for large rooms (> 10 peers), end-to-end encrypted text messages.
- **v1.0.0**: Stable API, plugin system, WebAssembly client, desktop system tray support.

---

## 6. Community & Contributing

We welcome contributions from developers worldwide! Check out our open issues, read `CONTRIBUTING.md`, and join the discussion.

