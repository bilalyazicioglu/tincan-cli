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
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::{mpsc, watch};

use crate::proto::PeerId;
use blip::Blip;
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
/// The smallest change worth redrawing for: finer than the widest meter we draw.
const METER_STEP: f32 = 1.0 / 64.0;
/// How much of a meter survives each 20 ms frame once its peer goes quiet. A meter
/// that stops dead looks frozen; one that falls looks like someone stopped talking.
const LEVEL_RELEASE: f32 = 0.75;

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
    /// How loud each peer sounds right now, already quantised to the five steps the
    /// interface can draw. Quantising here is what lets the interface sleep: it
    /// wakes only when a bar would actually change, not fifty times a second.
    pub peer_levels: watch::Receiver<HashMap<PeerId, u8>>,
    /// Whether the microphone loopback test is active (hearing own voice).
    pub mic_loopback: Arc<AtomicBool>,
    /// The RMS floor under which the microphone is treated as room noise, held as
    /// `f32::to_bits`. The interface writes it and the capture loop reads it, so the
    /// user can drag the gate while the detector is running.
    pub gate: Arc<AtomicU32>,
    pub health: Arc<AudioHealth>,
    pub blip_tx: mpsc::Sender<Blip>,
    /// The audio hardware stays open for as long as this is kept alive.
    pub devices: AudioDevices,
}

/// Opens the microphone and speaker and starts the audio loops.
pub fn start(me: PeerId, choice: &device::DeviceChoice) -> Result<VoiceIo> {
    let (devices, mut capture, mut playback, health) = device::open(choice)?;

    let (outgoing_tx, outgoing_rx) = mpsc::channel::<Vec<u8>>(64);
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<Incoming>(256);
    let (blip_tx, mut blip_rx) = mpsc::channel::<Blip>(16);
    let (speaking_tx, speaking_rx) = watch::channel(HashSet::new());
    let (mic_level_tx, mic_level_rx) = watch::channel(0.0f32);
    let (peer_levels_tx, peer_levels_rx) = watch::channel(HashMap::new());
    let (loopback_tx, mut loopback_rx) = mpsc::channel::<Vec<f32>>(16);

    let gate = Arc::new(AtomicU32::new(
        rms_for(crate::config::DEFAULT_GATE).to_bits(),
    ));
    let mic_open = Arc::new(AtomicBool::new(true));
    let hearing = Arc::new(AtomicBool::new(true));
    let mic_loopback = Arc::new(AtomicBool::new(false));

    // ── Capture: microphone → VAD → Opus → network ──────────────────────────
    let capture_mic = mic_open.clone();
    let capture_speaking = speaking_tx.clone();
    let capture_loopback = mic_loopback.clone();
    let capture_gate = gate.clone();
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

                // Only publish a level the meter could actually draw differently.
                // Sending every frame would redraw the whole interface fifty times a
                // second for a bar that never moved.
                let level = loudness(&pcm);
                mic_level_tx.send_if_modified(|shown| {
                    if (level - *shown).abs() < METER_STEP {
                        false
                    } else {
                        *shown = level;
                        true
                    }
                });

                // If loopback test is active, pipe local mic audio to speaker side.
                if capture_loopback.load(Ordering::Relaxed) {
                    let _ = loopback_tx.try_send(pcm.clone());
                }

                // The gate can move under us while someone drags it, so it is read
                // per frame rather than baked into the detector when it was built.
                detector.set_threshold(f32::from_bits(capture_gate.load(Ordering::Relaxed)));

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
        let mut levels: HashMap<PeerId, f32> = HashMap::new();
        let mut mixer = Mixer::default();
        let mut ticker = tokio::time::interval(FRAME_DURATION);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut decoded = vec![0f32; FRAME];
        let mut mixed = vec![0f32; FRAME];
        let mut blip_samples: Vec<f32> = Vec::new();
        let mut loopback_samples: Vec<f32> = Vec::new();
        let mut interface = vec![0f32; FRAME];

        loop {
            tokio::select! {
                Some(blip) = blip_rx.recv() => {
                    blip_samples.extend(blip::of(blip));
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
                    // Every meter falls a little each frame and is pushed back up by
                    // the audio actually decoded below. That gives an instant attack
                    // and a soft release, which is how a meter reads as a voice
                    // rather than as a flickering light.
                    for level in levels.values_mut() {
                        *level *= LEVEL_RELEASE;
                    }

                    while let Ok(blip) = blip_rx.try_recv() {
                        blip_samples.extend(blip::of(blip));
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
                                    let heard = loudness(&decoded[..written]);
                                    let level = levels.entry(*peer).or_insert(0.0);
                                    *level = level.max(heard);
                                    sources.push(decoded[..written].to_vec());
                                }
                                Ok(_) => {}
                                Err(err) => tracing::warn!("decoding error: {err:#}"),
                            }
                        }

                        let refs: Vec<&[f32]> = sources.iter().map(|s| s.as_slice()).collect();
                        mixer.mix(&refs, &mut mixed);

                        // Everything the program itself makes, as opposed to the room:
                        // its own sounds, and your microphone while you are testing it.
                        interface.fill(0.0);
                        queue_into(&mut blip_samples, &mut interface);
                        queue_into(&mut loopback_samples, &mut interface);

                        let hearing_now = playback_hearing.load(Ordering::Relaxed);
                        to_speaker(&mut mixed, hearing_now, &interface);
                        for sample in mixed.iter() {
                            let _ = playback.push(*sample);
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

                    levels.retain(|peer, level| *level > 0.01 && streams.contains_key(peer));
                    let bars: HashMap<PeerId, u8> = levels
                        .iter()
                        .map(|(peer, level)| (*peer, bar(*level)))
                        .collect();
                    peer_levels_tx.send_if_modified(|shown| {
                        if *shown == bars {
                            false
                        } else {
                            *shown = bars;
                            true
                        }
                    });

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
        peer_levels: peer_levels_rx,
        mic_loopback,
        gate,
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

/// Takes as much of a queued sound as fits into this frame.
fn queue_into(queue: &mut Vec<f32>, frame: &mut [f32]) {
    let take = queue.len().min(frame.len());
    for (slot, sample) in frame.iter_mut().zip(queue.drain(..take)) {
        *slot += sample;
    }
}

/// What actually reaches the speaker.
///
/// Deafening silences the room, not the interface: the sound telling you your ears are
/// shut has to survive the very state it is announcing, and so does the microphone test
/// you opened on purpose. The stream keeps running either way — letting the speaker
/// buffer drain would inflate the underrun counter for the wrong reason.
fn to_speaker(room: &mut [f32], hearing: bool, interface: &[f32]) {
    if !hearing {
        room.fill(0.0);
    }
    for (slot, sample) in room.iter_mut().zip(interface) {
        *slot += sample;
    }
}

/// The quietest sound the meter can show, in decibels.
const FLOOR_DB: f32 = -50.0;
/// How many decibels the meter spans, from its floor to full.
const SPAN_DB: f32 = 44.0;
/// Keeps the logarithm finite on digital silence.
const EPSILON: f32 = 1e-5;

/// Loudness on the scale the meters draw: 0.0 at the noise floor, 1.0 at a shout.
///
/// The ear works in decibels, so a linear RMS bar spends most of its travel doing
/// nothing at all. The window here is -50 dB (a quiet room) to -6 dB (a loud voice).
pub fn loudness(pcm: &[f32]) -> f32 {
    level_of(vad::rms(pcm))
}

/// Where an RMS reading sits on the meter.
pub fn level_of(rms: f32) -> f32 {
    let db = 20.0 * (rms + EPSILON).log10();
    ((db - FLOOR_DB) / SPAN_DB).clamp(0.0, 1.0)
}

/// The RMS threshold a position on the meter stands for — the inverse of `level_of`,
/// so what the user drags along the meter and what the detector compares against are
/// the same number in two units.
///
/// The bottom of the meter is not -50 dB but nothing at all: a gate dragged all the
/// way down means *never gate*, which is a state worth being able to ask for.
pub fn rms_for(level: f32) -> f32 {
    if level <= 0.0 {
        return 0.0;
    }
    let db = level.min(1.0) * SPAN_DB + FLOOR_DB;
    (10f32.powf(db / 20.0) - EPSILON).max(0.0)
}

/// Which of the five steps of the three-cell meter a loudness lands on.
pub fn bar(level: f32) -> u8 {
    (level.clamp(0.0, 1.0) * 4.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32) -> Vec<f32> {
        (0..FRAME)
            .map(|i| amplitude * (i as f32 * 0.1).sin())
            .collect()
    }

    #[test]
    fn deafening_silences_the_room_but_not_the_interface() {
        let mut room = vec![0.5; 4];
        let interface = vec![0.2; 4];

        to_speaker(&mut room, false, &interface);
        assert_eq!(room, vec![0.2; 4], "you must still hear yourself turn your ears back on");

        let mut room = vec![0.5; 4];
        to_speaker(&mut room, true, &interface);
        assert_eq!(room, vec![0.7; 4], "hearing normally, both arrive");
    }

    #[test]
    fn a_sound_longer_than_a_frame_carries_over_to_the_next() {
        let mut queued: Vec<f32> = vec![1.0; FRAME + 10];
        let mut frame = vec![0.0; FRAME];

        queue_into(&mut queued, &mut frame);
        assert_eq!(queued.len(), 10, "the tail has to wait for the next frame");
        assert!(frame.iter().all(|s| *s == 1.0));

        let mut frame = vec![0.0; FRAME];
        queue_into(&mut queued, &mut frame);
        assert!(queued.is_empty());
        assert_eq!(frame[..10], [1.0; 10], "and then it plays");
        assert_eq!(frame[10], 0.0, "with silence after it, not a repeat");
    }

    #[test]
    fn the_meter_scale_converts_both_ways() {
        for step in 1..=20 {
            let level = step as f32 / 20.0;
            let back = level_of(rms_for(level));
            assert!(
                (back - level).abs() < 0.01,
                "{level} became {back}; the meter and the gate must agree"
            );
        }
    }

    #[test]
    fn the_bottom_of_the_meter_means_never_gate() {
        assert_eq!(rms_for(0.0), 0.0, "a gate at zero must let digital silence through");
        assert_eq!(rms_for(-1.0), 0.0, "and must not go negative");
    }

    #[test]
    fn the_gate_we_shipped_with_is_where_the_marker_now_sits() {
        // The detector's old hard-coded 0.01 RMS is what the default has to reproduce,
        // or everyone's microphone changes behaviour on upgrade.
        let level = level_of(0.01);
        assert!(
            (level - crate::config::DEFAULT_GATE).abs() < 0.02,
            "0.01 RMS sits at {level}, the default claims {}",
            crate::config::DEFAULT_GATE
        );
    }

    #[test]
    fn silence_reads_as_nothing() {
        assert_eq!(loudness(&vec![0.0; FRAME]), 0.0);
        assert_eq!(bar(loudness(&vec![0.0; FRAME])), 0, "silence must draw an empty meter");
    }

    #[test]
    fn an_empty_frame_does_not_divide_by_zero() {
        assert_eq!(loudness(&[]), 0.0);
    }

    #[test]
    fn louder_audio_reads_higher() {
        let quiet = loudness(&tone(0.01));
        let talking = loudness(&tone(0.2));
        let shouting = loudness(&tone(0.9));
        assert!(quiet < talking, "{quiet} < {talking}");
        assert!(talking < shouting, "{talking} < {shouting}");
        assert!(shouting <= 1.0, "the meter must not run off its scale");
    }

    #[test]
    fn a_normal_speaking_voice_lands_in_the_middle_of_the_meter() {
        let step = bar(loudness(&tone(0.2)));
        assert!((1..=3).contains(&step), "a speaking voice drew step {step} of 4");
    }

    #[test]
    fn a_shout_fills_the_meter() {
        assert_eq!(bar(loudness(&tone(1.0))), 4);
    }

    #[test]
    fn a_quiet_peer_falls_silent_within_a_fifth_of_a_second() {
        let mut level = 1.0f32;
        let frames = (0.2 / FRAME_DURATION.as_secs_f32()) as usize;
        for _ in 0..frames {
            level *= LEVEL_RELEASE;
        }
        assert!(level < 0.125, "the meter still showed {level} after 200 ms of silence");
        assert_eq!(bar(level), 0);
    }
}
