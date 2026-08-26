# Tincan CLI Wiki & Technical Documentation Portal

Welcome to the official **Tincan CLI** Technical Wiki. Tincan is a serverless, peer-to-peer (P2P) voice and text chat client engineered for the terminal. It delivers multi-channel group communication without requiring central servers, user accounts, or third-party infrastructure.

---

## 📚 Wiki Contents

1. **[Architecture Overview](wiki/1-Architecture-Overview.md)**
   - Executive Architecture & Philosophy
   - Dual-Plane Design: Control Plane (Star) vs Voice Plane (Mesh)
   - Coordinator State Machine & Roster Synchronization
   - Bandwidth Scaling & Performance Profile

2. **[Audio Engine Deep Dive](wiki/2-Audio-Engine-Deep-Dive.md)**
   - Audio Pipeline & Signal Flow Diagram
   - `cpal` Audio Host Driver Bridge
   - Lock-Free Ring Buffers (`rtrb`)
   - Opus Codec & Packet Loss Concealment (PLC)
   - Voice Activity Detection (VAD) & DTX Silence Suppression
   - Per-Peer Adaptive Jitter Buffer
   - Multi-Source PCM Audio Mixer & Soft Limiter

3. **[Security & Cryptography Model](wiki/3-Security-and-Cryptography.md)**
   - Two-Tier Security Architecture
   - Zero-Knowledge Challenge-Response Authentication (Argon2id + Nonce)
   - Transport Layer Encryption: QUIC TLS 1.3 + Ed25519 Public Keys (Iroh)
   - Comprehensive Threat Model & Mitigations

4. **[Wire Protocol Specification](wiki/4-Protocol-Specification.md)**
   - Binary Wire Formats (`src/proto.rs`)
   - Control Plane Messages (`ClientMessage`, `ServerMessage`)
   - Voice Datagram Packet Header & Payload Layout
   - Postcard Binary Serialization Standard

5. **[Developer & Contribution Guide](wiki/5-Developer-and-Contribution-Guide.md)**
   - Development Setup & Dependencies (Opus, ALSA/CoreAudio)
   - Codebase Directory & Module Map
   - Running the Non-Networked Integration Test Suite (94 Tests)
   - Debug Logging & Inspection

6. **[Platform & OS Compatibility](wiki/6-Platform-and-OS-Compatibility.md)**
   - macOS (Apple Silicon & Intel)
   - Linux (Debian, Ubuntu, Fedora, Arch)
   - Windows & WASAPI Driver Strategy
   - Mobile & Embedded (Termux Android, Raspberry Pi)

7. **[Troubleshooting & FAQ](wiki/7-Troubleshooting-and-FAQ.md)**
   - Audio Hardware & 48 kHz Sample Rate Setup
   - NAT Traversal & DERP Relays
   - Terminal Hotkey Configuration & Troubleshooting

---

## ⚡ Quick Navigation

- **Main Repository**: [github.com/bilalyazicioglu/tincan-cli](https://github.com/bilalyazicioglu/tincan-cli)
- **Contributing Guidelines**: [CONTRIBUTING.md](../CONTRIBUTING.md)
- **Issue Tracker**: [GitHub Issues](https://github.com/bilalyazicioglu/tincan-cli/issues)
