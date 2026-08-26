# 6. Platform and OS Compatibility

Tincan is designed to run across Unix and Windows environments.

---

## 6.1 Platform Matrix

| Operating System | Architecture | Audio Driver | Status | Binary Asset |
| :--- | :--- | :--- | :--- | :--- |
| **macOS 11+** | Apple Silicon (`aarch64`) | CoreAudio | Fully Supported | Yes (`tincan-aarch64-apple-darwin.tar.gz`) |
| **macOS 11+** | Intel (`x86_64`) | CoreAudio | Fully Supported | Yes (`tincan-x86_64-apple-darwin.tar.gz`) |
| **Linux** | x86_64 | ALSA / Pulse / PipeWire | Fully Supported | Yes (`tincan-x86_64-unknown-linux-gnu.tar.gz`) |
| **Linux** | ARM64 (Raspberry Pi 4) | ALSA | Fully Supported | Yes (`tincan-aarch64-unknown-linux-gnu.tar.gz`) |
| **Windows 10/11** | x86_64 | WASAPI | Roadmap (v0.2.0) | Source compile |
| **Android** | Termux ARM64 | OpenSL ES / AAudio | Roadmap (v0.2.0) | Source compile |

---

## 6.2 macOS Specifics
- **Microphone Permissions**: Granted to the terminal app executing tincan (Terminal, iTerm2, VS Code, Alacritty).
- **Code Signing**: Binaries are ad-hoc signed (`codesign -s -`).

---

## 6.3 Linux Specifics
- **Static Opus Linking**: Linux release assets link Opus statically to ensure zero `libopus.so` runtime dependency issues across distros.
