//! The voice plane: capture, encoding, jitter buffering, mixing, playback.

pub mod blip;
pub mod codec;
pub mod device;
pub mod jitter;
pub mod mixer;
pub mod resample;
pub mod vad;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{mpsc, watch};

use crate::proto::PeerId;
use device::{AudioDevices, AudioHealth};
use jitter::{Frame, JitterBuffer};
use mixer::Mixer;
use vad::Vad;

/// Opus's native rate; the whole chain is built on it.
pub const SAMPLE_RATE: u32 = 48_000;
/// A 20 ms frame. The standard balance between latency and per-packet overhead.
pub const FRAME: usize = 960;
/// Target bitrate per person. In a six-person mesh that means ~160 kbps upload.
pub const BITRATE: i32 = 32_000;
/// Frame duration.
pub const FRAME_DURATION: Duration = Duration::from_millis(20);
/// The jitter buffer's target depth (3 frames ≈ 60 ms).
const JITTER_TARGET: usize = 3;
/// How many frames we try to keep queued in the speaker buffer.
const PLAYBACK_TARGET_FRAMES: usize = 3;
/// If a peer sends nothing for this long, its resources are released.
const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// A voice frame arriving from the network.
#[derive(Debug, Clone)]
pub struct Incoming {
    pub from: PeerId,
    pub seq: u32,
    pub payload: Vec<u8>,
}

/// The audio engine's connections to the outside world.
pub struct VoiceIo {
    /// Our own encoded voice — the network layer distributes these to the mesh.
    pub outgoing: mpsc::Receiver<Vec<u8>>,
    /// Frames arriving from the network are written here.
    pub incoming: mpsc::Sender<Incoming>,
    /// Who is currently speaking (ourselves included) — the interface indicator
    /// listens to this.
    pub speaking: watch::Receiver<HashSet<PeerId>>,
    /// Whether the microphone is open — the **result** of the mute and push-to-talk
    /// decisions. The interface computes this one flag and the engine only reads it.
    pub mic_open: Arc<AtomicBool>,
    /// Whether we can hear the others (`true` unless deafened).
    pub hearing: Arc<AtomicBool>,
    /// Live microphone volume level (0.0 to 1.0) for VU meter visualization.
    pub mic_level: watch::Receiver<f32>,
    /// Whether the microphone loopback test is active (hearing own voice).
    pub mic_loopback: Arc<AtomicBool>,
    pub health: Arc<AudioHealth>,
    pub blip_tx: mpsc::Sender<()>,
    /// The audio hardware stays open for as long as this is kept alive.
    pub devices: AudioDevices,
}

/// Opens the microphone and speaker and starts the audio loops.
pub fn start(me: PeerId, choice: &device::DeviceChoice) -> Result<VoiceIo> {
    let (devices, mut capture, mut playback, health) = device::open(choice)?;

    let (outgoing_tx, outgoing_rx) = mpsc::channel::<Vec<u8>>(64);
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<Incoming>(256);
    let (blip_tx, mut blip_rx) = mpsc::channel::<()>(16);
    let (speaking_tx, speaking_rx) = watch::channel(HashSet::new());
    let (mic_level_tx, mic_level_rx) = watch::channel(0.0f32);
    let (loopback_tx, mut loopback_rx) = mpsc::channel::<Vec<f32>>(16);

    let mic_open = Arc::new(AtomicBool::new(true));
    let hearing = Arc::new(AtomicBool::new(true));
    let mic_loopback = Arc::new(AtomicBool::new(false));

    // ── Capture: microphone → VAD → Opus → network ──────────────────────────
    let capture_mic = mic_open.clone();
    let capture_speaking = speaking_tx.clone();
    let capture_loopback = mic_loopback.clone();
    tokio::spawn(async move {
        let mut encoder = match codec::Encoder::new() {
            Ok(encoder) => encoder,
            Err(err) => {
                tracing::error!("could not start the encoder: {err:#}");
                return;
            }
        };
        let mut detector = Vad::default();
        let mut pcm = vec![0f32; FRAME];
        let mut ticker = tokio::time::interval(FRAME_DURATION);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;

            // Process every whole frame that has piled up, so a missed tick does not
            // turn into growing latency.
            while capture.slots() >= FRAME {
                for slot in pcm.iter_mut() {
                    *slot = capture.pop().unwrap_or(0.0);
                }

                // Compute real-time RMS audio level with logarithmic dB scaling for VU meter.
                let sum_sq: f32 = pcm.iter().map(|&s| s * s).sum();
                let rms = (sum_sq / pcm.len() as f32).sqrt();
                // Map dB range [-50 dB (noise floor) .. -6 dB (peak)] to [0.0 .. 1.0]
                let db = 20.0 * (rms + 1e-5).log10();
                let level = ((db + 50.0) / 44.0).clamp(0.0, 1.0);
                let _ = mic_level_tx.send(level);

                // If loopback test is active, pipe local mic audio to speaker side.
                if capture_loopback.load(Ordering::Relaxed) {
                    let _ = loopback_tx.try_send(pcm.clone());
                }

                // Keep feeding the VAD even while the microphone is closed, so that
                // the hangover state is consistent when it opens again — but send
                // nothing.
                let active = detector.update(&pcm) && capture_mic.load(Ordering::Relaxed);

                capture_speaking.send_if_modified(|speakers| {
                    if active {
                        speakers.insert(me)
                    } else {
                        speakers.remove(&me)
                    }
                });

                if !active {
                    continue;
                }
                match encoder.encode(&pcm) {
                    Ok(packet) => {
                        // If the network cannot keep up, dropping the frame beats
                        // queueing it and accumulating latency.
                        let _ = outgoing_tx.try_send(packet.to_vec());
                    }
                    Err(err) => tracing::warn!("encoding error: {err:#}"),
                }
            }
        }
    });

    // ── Playback: network → jitter → Opus decode → mixing → speaker ─────────
    let playback_hearing = hearing.clone();
    tokio::spawn(async move {
        let mut streams: HashMap<PeerId, PeerStream> = HashMap::new();
        let mut mixer = Mixer::default();
        let mut ticker = tokio::time::interval(FRAME_DURATION);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut decoded = vec![0f32; FRAME];
        let mut mixed = vec![0f32; FRAME];
        let mut blip_samples: Vec<f32> = Vec::new();
        let mut loopback_samples: Vec<f32> = Vec::new();

        loop {
            tokio::select! {
                Some(_) = blip_rx.recv() => {
                    blip_samples.extend(blip::generate_blip());
                }

                Some(pcm_loop) = loopback_rx.recv() => {
                    loopback_samples.extend(pcm_loop);
                }

                Some(packet) = incoming_rx.recv() => {
                    let stream = match streams.get_mut(&packet.from) {
                        Some(stream) => stream,
                        None => match PeerStream::new() {
                            Ok(stream) => streams.entry(packet.from).or_insert(stream),
                            Err(err) => {
                                tracing::warn!("could not open a decoder: {err:#}");
                                continue;
                            }
                        },
                    };
                    stream.last_packet = Instant::now();
                    stream.buffer.push(packet.seq, packet.payload);
                }

                _ = ticker.tick() => {
                    while blip_rx.try_recv().is_ok() {
                        blip_samples.extend(blip::generate_blip());
                    }
                    while let Ok(pcm_loop) = loopback_rx.try_recv() {
                        loopback_samples.extend(pcm_loop);
                    }

                    // Feed the speaker buffer up to its target fill. Looking at the
                    // fill level compensates for tick drift on its own.
                    let capacity_frames = 4;
                    for _ in 0..capacity_frames {
                        if playback.slots() < FRAME {
                            break;
                        }
                        let filled = playback.buffer().capacity() - playback.slots();
                        if filled >= FRAME * PLAYBACK_TARGET_FRAMES {
                            break;
                        }

                        let mut sources: Vec<Vec<f32>> = Vec::new();
                        let mut active: HashSet<PeerId> = HashSet::new();

                        for (peer, stream) in streams.iter_mut() {
                            let frame = stream.buffer.pop();
                            let speaking = !matches!(frame, Frame::Silence);
                            match stream.decoder.decode(&frame, &mut decoded) {
                                Ok(written) if speaking => {
                                    active.insert(*peer);
                                    sources.push(decoded[..written].to_vec());
                                }
                                Ok(_) => {}
                                Err(err) => tracing::warn!("decoding error: {err:#}"),
                            }
                        }

                        let refs: Vec<&[f32]> = sources.iter().map(|s| s.as_slice()).collect();
                        mixer.mix(&refs, &mut mixed);

                        // Mix in UI blips
                        if !blip_samples.is_empty() {
                            let take = std::cmp::min(blip_samples.len(), mixed.len());
                            for (i, sample) in blip_samples.drain(..take).enumerate() {
                                mixed[i] += sample;
                            }
                        }

                        // Mix in local mic loopback test
                        if !loopback_samples.is_empty() {
                            let take = std::cmp::min(loopback_samples.len(), mixed.len());
                            for (i, sample) in loopback_samples.drain(..take).enumerate() {
                                mixed[i] += sample;
                            }
                        }

                        // When deafened we do not stop the stream, we feed silence:
                        // letting the speaker buffer drain would inflate the underrun
                        // counter for the wrong reason.
                        let hearing_now = playback_hearing.load(Ordering::Relaxed);
                        for sample in mixed.iter() {
                            let _ = playback.push(if hearing_now { *sample } else { 0.0 });
                        }

                        speaking_tx.send_if_modified(|speakers| {
                            // The capture side owns our own speaking state; here we
                            // only update the others.
                            let mine = speakers.contains(&me);
                            let mut next = active.clone();
                            if mine {
                                next.insert(me);
                            }
                            if *speakers == next {
                                false
                            } else {
                                *speakers = next;
                                true
                            }
                        });
                    }

                    streams.retain(|_, stream| stream.last_packet.elapsed() < PEER_IDLE_TIMEOUT);
                }
            }
        }
    });

    Ok(VoiceIo {
        outgoing: outgoing_rx,
        incoming: incoming_tx,
        speaking: speaking_rx,
        mic_open,
        hearing,
        mic_level: mic_level_rx,
        mic_loopback,
        health,
        blip_tx,
        devices,
    })
}

/// The state of one peer's incoming audio.
struct PeerStream {
    buffer: JitterBuffer,
    decoder: codec::Decoder,
    last_packet: Instant,
}

impl PeerStream {
    fn new() -> Result<Self> {
        Ok(Self {
            buffer: JitterBuffer::new(JITTER_TARGET),
            decoder: codec::Decoder::new()?,
            last_packet: Instant::now(),
        })
    }
}
