# 2. Audio Engine Deep Dive

Tincan's audio engine (`src/audio/`) is built for sub-30ms real-time voice communications over lossy networks.

---

## 2.1 Audio Pipeline Architecture

```
 [ Microphone ]
       │ (cpal capture callback)
       ▼
 [ Lock-Free Ring Buffer (rtrb) ]
       │ (audio processing thread)
       ▼
 [ Voice Activity Detector (VAD) ]
       │ (speech detected)
       ▼
 [ Opus Encoder (audiopus) ]
       │ (encoded frames)
       ▼
 [ QUIC Datagram Network Mesh ]
       │ (incoming UDP packets)
       ▼
 [ Adaptive Jitter Buffer ]
       │ (reordered & PLC concealed)
       ▼
 [ Opus Decoder ]
       │ (PCM frames)
       ▼
 [ Multi-Source Mixer & Limiter ]
       │ (cpal playback callback)
       ▼
 [ Speakers ]
```

---

## 2.2 Core Modules

### 1. Audio Hardware Bridge (`src/audio/device.rs`)
- Uses `cpal` to interface with native system sound servers: CoreAudio (macOS), ALSA / PulseAudio / PipeWire (Linux), WASAPI (Windows).
- Enforces native 48000 Hz sample rate and 20ms frame lengths (960 samples per frame).

### 2. Lock-Free Thread Safety (`rtrb`)
- Audio hardware callbacks run in high-priority real-time threads.
- Real-time threads **must never block, lock mutexes, allocate memory, or perform I/O**.
- Tincan uses lock-free Single-Producer Single-Consumer (SPSC) ring buffers (`rtrb`) to bridge audio callbacks with network worker threads.

### 3. Opus Codec & Loss Concealment (`src/audio/codec.rs`)
- Encodes 16-bit PCM mono audio into 48 kHz Opus frames (typically 40–80 bytes per packet).
- Features built-in Packet Loss Concealment (PLC). When network packets drop, the decoder interpolates missing audio frames based on linear predictive coding.

### 4. Voice Activity Detection (`src/audio/vad.rs`)
- Calculates frame Root Mean Square (RMS) energy against background noise floors.
- Enables Discontinuous Transmission (DTX): suppresses packet generation during silence, cutting upload bandwidth to zero when not speaking.

### 5. Per-Peer Adaptive Jitter Buffer (`src/audio/jitter.rs`)
- Smooths out packet arrival jitter caused by Wi-Fi or internet routing variance.
- Reorders out-of-order sequence numbers and dynamically adjusts buffer depth to minimize latency while preventing dropouts.

### 6. Multi-Source Audio Mixer & Soft Limiter (`src/audio/mixer.rs`)
- Sums PCM samples from all active speakers in a channel.
- Employs a soft limiter to prevent digital clipping when multiple peers talk simultaneously.
