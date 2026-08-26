# 7. Troubleshooting and FAQ

Common issues and resolution steps for Tincan CLI.

---

## 7.1 Frequently Asked Questions

### Q: Why does Tincan require a 48000 Hz microphone/speaker sample rate?
A: Opus operates natively at 48000 Hz. If a device operates at 44.1 kHz or 96 kHz without a hardware resampler, Tincan falls back to text-only mode to prevent distorted audio playback.

### Q: Is my voice traffic routed through a third-party server?
A: No. Voice traffic is sent directly P2P between peers using QUIC UDP datagrams.

---

## 7.2 Common Issues & Fixes

### Issue 1: `audio could not start, text chat only: sample rate is not 48000 Hz`
- **Fix**: Open OS sound settings and change default input/output sample rate to 48.0 kHz 16-bit / 24-bit.

### Issue 2: Terminal Function Keys F1-F5 do not trigger action
- **Fix**: Some terminal emulators trap F-keys. Use alternative shortcuts: `Ctrl+G` for voice join (F2), `Ctrl+T` for mute (F3).

