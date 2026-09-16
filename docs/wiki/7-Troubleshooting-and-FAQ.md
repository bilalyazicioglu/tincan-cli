# 7. Troubleshooting and FAQ

Common issues and resolution steps for Tincan CLI.

---

## 7.1 Frequently Asked Questions

### Q: Does my microphone have to run at 48000 Hz?
A: No. Opus works at 48000 Hz and that is what travels over the wire, but `src/audio/resample.rs` converts to and from whatever rate your device reports, using cubic Hermite interpolation on the capture and playback threads. A 44.1 kHz USB microphone and a 16 kHz Bluetooth headset are both fine. What tincan cannot open is a device that reports no format at all; that one is refused, with a line saying so, rather than guessed at.

### Q: Is my voice traffic routed through a third-party server?
A: Usually not, but sometimes yes. Voice is sent peer to peer as QUIC UDP datagrams whenever a direct link can be established, and that is the normal case. When hole punching fails, the datagrams fall back to one of n0's relay servers, which forwards them without being able to read them — the QUIC session belongs to the two peers. The interface tells you which of the two is happening: the string down the middle runs taut on a direct link and dashed through a relay.

### Q: Can the people I talk to see my IP address?
A: Yes, and so can anyone holding the invite code. A public key is only enough to find you because the addresses behind it are published under that key, and those records are public; peers that connect directly then see each other's addresses anyway. This is what having no server in the middle costs. If your address is the thing you need to keep private, put tincan behind a VPN — the room does not care which address it reaches you on. See [Security](../../README.md#security).

### Q: Does Tincan work without an internet connection?
A: No, not even between two machines on the same network. Peers find each other through n0's pkarr relay and DNS, and tincan's iroh preset carries no local discovery, so a room cannot form offline. See [What tincan depends on](../../README.md#what-tincan-depends-on).

---

## 7.2 Common Issues & Fixes

### Issue 1: `... is not reporting a format, so it cannot be opened`
- **Cause**: The device gave `cpal` no default configuration to work from. The sample rate itself is never the problem — every rate is resampled.
- **Fix**: Pick another device on the settings screen, or check that the one you want is not held open exclusively by another application.

### Issue 2: Terminal Function Keys F1-F5 do not trigger action
- **Fix**: Some terminal emulators trap F-keys. Use alternative shortcuts: `Ctrl+G` for voice join (F2), `Ctrl+T` for mute (F3).

