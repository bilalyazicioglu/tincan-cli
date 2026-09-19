# tincan-cli

> **Serverless peer-to-peer voice and text chat for your terminal.**

This is the official NPM distribution package for [`tincan`](https://github.com/bilalyazicioglu/tincan-cli). It downloads and runs the prebuilt native binary for your platform.

## Quick Start

Run directly without installing:

```bash
npx tincan-cli host
```

Or install globally:

```bash
npm install -g tincan-cli
tincan host --name alice
```

## Join a Room

```bash
tincan join <INVITE-CODE> --name bob
```

## Features

- **P2P & Serverless**: Voice and chat run peer to peer over QUIC through [iroh](https://iroh.computer). No accounts, and no server of ours holds your room — though finding peers and getting through NATs does rely on iroh's public infrastructure, run by Number Zero ([details](https://github.com/bilalyazicioglu/tincan-cli#what-tincan-depends-on)).
- **Low-Latency Voice**: 48 kHz Opus audio with per-peer jitter buffering and DTX silence suppression.
- **Terminal UI**: Metallic rust theme, live VU meters, and travelling pulse latency visualizations.
- **Zero Config**: NAT traversal and relay fallbacks work globally out-of-the-box.

## Platforms

Prebuilt binaries are downloaded for macOS (`arm64`, `x64`), Linux (`x64`, `arm64`) and
Windows (`x64`). The Windows binary is built and tested in CI but has not been exercised
on a real Windows desktop; anything else falls back to `cargo install --git https://github.com/bilalyazicioglu/tincan-cli`.

## Links

- **GitHub Repository**: [github.com/bilalyazicioglu/tincan-cli](https://github.com/bilalyazicioglu/tincan-cli)
- **Documentation & Wiki**: [Technical Wiki](https://github.com/bilalyazicioglu/tincan-cli/blob/main/docs/WIKI.md)
- **License**: MIT
