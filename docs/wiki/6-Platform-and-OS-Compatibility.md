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
| **Windows 10/11** | x86_64 | WASAPI | Builds and tests in CI; untried on a desktop | Yes (`tincan-x86_64-pc-windows-msvc.zip`) |
| **Android** | Termux ARM64 | OpenSL ES / AAudio | Text chat only; voice is #43 | Source compile |

---

## 6.2 macOS Specifics
- **Microphone Permissions**: Granted to the terminal app executing tincan (Terminal, iTerm2, VS Code, Alacritty).
- **Code Signing**: Binaries are ad-hoc signed (`codesign -s -`).

---

## 6.3 Linux Specifics
- **Static Opus Linking**: Linux release assets link Opus statically to ensure zero `libopus.so` runtime dependency issues across distros.

---

## 6.4 Windows Specifics
- **Archive Format**: The Windows asset is a `.zip` rather than a `.tar.gz`, and contains `tincan.exe`. Install it with `npx tincan-cli`, or unpack it and put the binary on your `PATH`. `install.sh` is a POSIX script and does not cover Windows.
- **Opus**: `audiopus_sys` carries a prebuilt `libopus.lib` for MSVC, so no autotools, CMake or vcpkg is needed — the release build links it statically and the workflow fails if an Opus DLL appears next to the binary.
- **Settings File**: `%APPDATA%\tincan\config.toml`, rather than the `~/.config/tincan/config.toml` used on Unix.
- **Clipboard**: The invite code is copied through PowerShell's `Set-Clipboard`. `clip.exe` was avoided because it decodes stdin as the machine's local code page.
- **What is not verified**: CI proves the binary compiles, links and passes the test suite. It does not prove the terminal UI renders, that WASAPI enumerates devices, or that the microphone permission prompt behaves. None of that has been tried on a real Windows machine.
