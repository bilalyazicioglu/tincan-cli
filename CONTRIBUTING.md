# Contributing to Tincan CLI

Thank you for your interest in contributing to **Tincan CLI**! We welcome bug reports, feature proposals, documentation improvements, and code contributions from developers of all skill levels.

---

## Code of Conduct

Please treat everyone in the community with respect, empathy, and professional courtesy.

---

## How to Contribute

### 1. Finding an Issue
Browse the [GitHub Issues](https://github.com/bilalyazicioglu/tincan-cli/issues) for open tasks:
- `good first issue`: Ideal for new contributors.
- `help wanted`: Tasks where community help is actively requested.
- `bug`: Confirmed defects needing fixes.
- `enhancement`: New features and design proposals.

### 2. Setting Up Development Environment
1. Fork and clone the repository:
   ```bash
   git clone https://github.com/YOUR-USERNAME/tincan-cli.git
   cd tincan-cli
   ```
2. Install build dependencies (Opus, pkg-config, ALSA/CoreAudio).
3. Run tests:
   ```bash
   cargo test
   cargo clippy --all-targets
   ```

### 3. Submitting Pull Requests
- Keep PRs focused on a single logical change.
- Ensure all automated unit & integration tests pass (`cargo test`).
- Format code with `cargo fmt`.
- Write clear commit messages. Do NOT include AI co-author tags or automated bot signatures.

---

## Coding Guidelines

- **Rust Edition**: Rust 2024 edition (`rust-version = 1.91`).
- **Error Handling**: Use `anyhow` for top-level error propagation and specific error enums for library modules.
- **Async Runtime**: Use `tokio` multi-threaded runtime.
- **Audio Code Style**: Keep lock-free audio ring buffers isolated from async/blocking calls to guarantee real-time audio safety.

