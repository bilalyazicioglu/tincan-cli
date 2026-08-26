# 5. Developer and Contribution Guide

This guide covers building, testing, debugging, and contributing to Tincan CLI.

---

## 5.1 System Prerequisites

- **Rust**: Version 1.91+ (edition 2024).
- **Opus**: C Opus codec library & headers (`libopus-dev` on Ubuntu/Debian, `opus` via Homebrew on macOS).
- **Audio Host Libraries**: `libasound2-dev` on Linux (ALSA), CoreAudio framework on macOS.

### Installing Dependencies

```bash
# macOS
brew install opus pkg-config autoconf automake libtool

# Debian / Ubuntu
sudo apt install libopus-dev pkg-config libasound2-dev autoconf automake libtool
```

---

## 5.2 Building & Running Locally

```bash
# Clone the repository
git clone https://github.com/bilalyazicioglu/tincan-cli.git
cd tincan-cli

# Build debug binary
cargo build

# Run room host
cargo run -- host --name alice --room lobby

# Run room join
cargo run -- join <code-string> --name bob
```

---

## 5.3 Non-Networked Integration Test Suite

Tincan includes a 94-test suite validating room state, password auth, jitter buffer, and audio mesh without touching external internet connections.

```bash
# Run unit & integration tests
cargo test

# Run linter checks
cargo clippy --all-targets
```

---

## 5.4 Debug Logging

Tincan logs to `stderr` so debug messages never scramble the Ratatui terminal interface.

```bash
RUST_LOG=tincan=debug cargo run -- host 2>tincan.log
```

