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

- **P2P & Serverless**: Runs directly over QUIC and encrypted WebRTC/Iroh mesh connections. No central servers, no user accounts.
- **Low-Latency Voice**: 48 kHz Opus audio with per-peer jitter buffering and DTX silence suppression.
- **Terminal UI**: Metallic rust theme, live VU meters, and travelling pulse latency visualizations.
- **Zero Config**: NAT traversal and relay fallbacks work globally out-of-the-box.

## Links

- **GitHub Repository**: [github.com/bilalyazicioglu/tincan-cli](https://github.com/bilalyazicioglu/tincan-cli)
- **Documentation & Wiki**: [Technical Wiki](https://github.com/bilalyazicioglu/tincan-cli/blob/main/docs/WIKI.md)
- **License**: MIT
