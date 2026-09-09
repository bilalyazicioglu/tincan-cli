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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
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
/// How long a recorded microphone test runs, and then how long it plays back.
pub const TEST_LENGTH: Duration = Duration::from_secs(3);
/// The ceiling on the buffer holding it.
const TEST_SAMPLES: usize = 3 * SAMPLE_RATE as usize;

/// What the microphone test is doing.
///
/// Recording and playing back are separate states rather than one "test" flag because
/// the whole point is that the speaker stays quiet while the microphone is open: on a
/// laptop, anything else closes an acoustic loop between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MicTest {
    #[default]
    Off,
    /// The microphone is being recorded and nothing is going to the speaker.
    Recording,
    /// What was recorded is playing back and the microphone is ignored.
    Playing,
    /// The microphone goes straight to the speaker. Safe on headphones and nowhere
    /// else, which is why it is not the default.
    Monitoring,
}

impl MicTest {
    pub fn bits(self) -> u8 {
        self as u8
    }

    pub fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Recording,
            2 => Self::Playing,
            3 => Self::Monitoring,
            _ => Self::Off,
        }
    }

    /// Whether the microphone is being taken in at all.
    pub fn listening(self) -> bool {
        matches!(self, Self::Recording | Self::Monitoring)
    }
}

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
    /// How loud each person is played back, where a missing entry means untouched.
    /// The interface writes it and the playback loop reads it, so turning someone down
    /// takes effect on the next frame.
    pub peer_gains: watch::Sender<HashMap<PeerId, f32>>,
    /// What the microphone test is doing, as `MicTest::bits`.
    pub mic_test: Arc<AtomicU8>,
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
    let (peer_gains_tx, peer_gains_rx) = watch::channel(HashMap::new());
    let (loopback_tx, mut loopback_rx) = mpsc::channel::<Vec<f32>>(16);

    let gate = Arc::new(AtomicU32::new(
        rms_for(crate::config::DEFAULT_GATE).to_bits(),
    ));
    let mic_open = Arc::new(AtomicBool::new(true));
    let hearing = Arc::new(AtomicBool::new(true));
    let mic_test = Arc::new(AtomicU8::new(MicTest::Off.bits()));

    // ── Capture: microphone → VAD → Opus → network ──────────────────────────
    let capture_mic = mic_open.clone();
    let capture_speaking = speaking_tx.clone();
    let capture_test = mic_test.clone();
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

                // Hand the microphone to the test while it wants it. Where those
                // samples go — the speaker now, or a buffer for later — is the
                // playback side's decision.
                if MicTest::from_bits(capture_test.load(Ordering::Relaxed)).listening() {
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
    let playback_test = mic_test.clone();
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
        let mut recorded: Vec<f32> = Vec::new();
        let mut interface = vec![0f32; FRAME];

        loop {
            tokio::select! {
                Some(blip) = blip_rx.recv() => {
                    overlay(&mut blip_samples, blip::of(blip));
                }

                Some(pcm_loop) = loopback_rx.recv() => {
                    take_microphone(
                        MicTest::from_bits(playback_test.load(Ordering::Relaxed)),
                        pcm_loop,
                        &mut loopback_samples,
                        &mut recorded,
                    );
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
                        overlay(&mut blip_samples, blip::of(blip));
                    }
                    let test = MicTest::from_bits(playback_test.load(Ordering::Relaxed));
                    while let Ok(pcm_loop) = loopback_rx.try_recv() {
                        take_microphone(test, pcm_loop, &mut loopback_samples, &mut recorded);
                    }
                    match test {
                        // The recording is handed over whole the moment the interface
                        // says it is time to hear it.
                        MicTest::Playing => loopback_samples.append(&mut recorded),
                        MicTest::Off => {
                            recorded.clear();
                            loopback_samples.clear();
                        }
                        MicTest::Recording | MicTest::Monitoring => {}
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

                        let gains = peer_gains_rx.borrow();
                        for (peer, stream) in streams.iter_mut() {
                            let frame = stream.buffer.pop();
                            let speaking = !matches!(frame, Frame::Silence);
                            match stream.decoder.decode(&frame, &mut decoded) {
                                Ok(written) if speaking => {
                                    // Both of these come before the volume is applied.
                                    // Someone you have turned down is still speaking,
                                    // and their meter still has to say so — otherwise
                                    // a silenced peer is indistinguishable from one
                                    // who has gone quiet.
                                    active.insert(*peer);
                                    let heard = loudness(&decoded[..written]);
                                    let level = levels.entry(*peer).or_insert(0.0);
                                    *level = level.max(heard);

                                    let gain = gains.get(peer).copied().unwrap_or(1.0);
                                    if let Some(source) = at_volume(&decoded[..written], gain) {
                                        sources.push(source);
                                    }
                                }
                                Ok(_) => {}
                                Err(err) => tracing::warn!("decoding error: {err:#}"),
                            }
                        }
                        drop(gains);

                        // Everything the program itself makes, as opposed to the room:
                        // its own sounds, and your own microphone played back at you.
                        interface.fill(0.0);
                        queue_into(&mut blip_samples, &mut interface);
                        queue_into(&mut loopback_samples, &mut interface);

                        // One bus, one limiter. The interface used to be added after
                        // the limiter had already done its work, which left its own
                        // sounds — a played-back recording most of all — with nothing
                        // holding them under full scale.
                        let hearing_now = playback_hearing.load(Ordering::Relaxed);
                        let bus = speaker_bus(&sources, &interface, hearing_now);
                        mixer.mix(&bus, &mut mixed);
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
        peer_gains: peer_gains_tx,
        mic_test,
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

/// Where a frame of microphone audio goes while a test is running.
fn take_microphone(
    test: MicTest,
    pcm: Vec<f32>,
    speaker: &mut Vec<f32>,
    recorded: &mut Vec<f32>,
) {
    match test {
        MicTest::Monitoring => speaker.extend(pcm),
        MicTest::Recording => {
            // Bounded: a recording that outgrew its own playback would be a leak.
            let room = TEST_SAMPLES.saturating_sub(recorded.len());
            recorded.extend(pcm.into_iter().take(room));
        }
        MicTest::Off | MicTest::Playing => {}
    }
}

/// What goes on the speaker bus.
///
/// The program's own sounds always; the room only while the ears are open. Deafening
/// leaves the room off the bus rather than zeroing it afterwards, so the limiter is
/// never working against audio nobody is going to hear.
fn speaker_bus<'a>(room: &'a [Vec<f32>], interface: &'a [f32], hearing: bool) -> Vec<&'a [f32]> {
    let mut bus: Vec<&[f32]> = Vec::with_capacity(room.len() + 1);
    if hearing {
        bus.extend(room.iter().map(|source| source.as_slice()));
    }
    bus.push(interface);
    bus
}

/// One person's decoded audio at the volume you have chosen for them.
///
/// Returns nothing for someone you have silenced. Leaving them off the bus rather than
/// mixing in a frame of zeroes is the same reasoning as deafening: the limiter should
/// never be working against audio nobody is going to hear.
fn at_volume(pcm: &[f32], gain: f32) -> Option<Vec<f32>> {
    if gain <= 0.0 {
        return None;
    }
    if gain == 1.0 {
        return Some(pcm.to_vec());
    }
    Some(pcm.iter().map(|sample| sample * gain).collect())
}

/// Adds a sound to whatever is already waiting, starting now.
///
/// Two sounds triggered at almost the same moment should overlap rather than take
/// turns. Queueing them would put every key click behind the one before it and let a
/// fast typist outrun their own keyboard. The limiter on the speaker bus is what keeps
/// the sum in bounds.
fn overlay(queue: &mut Vec<f32>, sound: Vec<f32>) {
    if queue.len() < sound.len() {
        queue.resize(sound.len(), 0.0);
    }
    for (slot, sample) in queue.iter_mut().zip(sound) {
        *slot += sample;
    }
}

/// Takes as much of a queued sound as fits into this frame.
fn queue_into(queue: &mut Vec<f32>, frame: &mut [f32]) {
    let take = queue.len().min(frame.len());
    for (slot, sample) in frame.iter_mut().zip(queue.drain(..take)) {
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
        let room = vec![vec![0.5; 4]];
        let interface = vec![0.2; 4];

        let shut = speaker_bus(&room, &interface, false);
        assert_eq!(shut.len(), 1, "the room must be off the bus");
        assert_eq!(shut[0], &interface[..], "you must still hear your ears reopen");

        let open = speaker_bus(&room, &interface, true);
        assert_eq!(open.len(), 2, "hearing normally, both arrive");
    }

    #[test]
    fn a_played_back_recording_cannot_leave_full_scale() {
        // It used to: the interface bus was added after the limiter had run, so
        // nothing held it down. Feedback then clipped instead of merely being loud.
        let mut mixer = Mixer::default();
        let room: Vec<Vec<f32>> = Vec::new();
        let interface = vec![1.8; FRAME];
        let mut out = vec![0.0; FRAME];

        mixer.mix(&speaker_bus(&room, &interface, true), &mut out);

        let peak = out.iter().fold(0f32, |peak, s| peak.max(s.abs()));
        assert!(peak <= 1.0, "the speaker saw {peak}");
        assert!(peak > 0.5, "and it must still be audible");
    }

    #[test]
    fn the_speaker_stays_out_of_it_while_the_microphone_is_recording() {
        let mut speaker = Vec::new();
        let mut recorded = Vec::new();

        take_microphone(MicTest::Recording, vec![0.5; 100], &mut speaker, &mut recorded);
        assert!(
            speaker.is_empty(),
            "a speaker that plays while the microphone is open is the whole bug"
        );
        assert_eq!(recorded.len(), 100);

        take_microphone(MicTest::Monitoring, vec![0.5; 100], &mut speaker, &mut recorded);
        assert_eq!(speaker.len(), 100, "monitoring is the one mode that does play live");
    }

    #[test]
    fn a_recording_cannot_grow_past_its_own_length() {
        let mut speaker = Vec::new();
        let mut recorded = Vec::new();
        for _ in 0..(TEST_SAMPLES / FRAME + 10) {
            take_microphone(MicTest::Recording, vec![0.1; FRAME], &mut speaker, &mut recorded);
        }
        assert_eq!(recorded.len(), TEST_SAMPLES);
    }

    #[test]
    fn a_test_that_is_over_leaves_nothing_behind() {
        let mut speaker = Vec::new();
        let mut recorded = Vec::new();
        take_microphone(MicTest::Off, vec![0.5; 100], &mut speaker, &mut recorded);
        take_microphone(MicTest::Playing, vec![0.5; 100], &mut speaker, &mut recorded);
        assert!(speaker.is_empty() && recorded.is_empty());
    }

    #[test]
    fn two_sounds_at_once_overlap_rather_than_queue() {
        let mut queued = vec![0.5; 10];
        overlay(&mut queued, vec![0.25; 4]);

        assert_eq!(queued.len(), 10, "a short sound must not lengthen what is playing");
        assert_eq!(queued[0], 0.75, "it starts now, not after");
        assert_eq!(queued[4], 0.5, "and stops where it ends");
    }

    #[test]
    fn a_sound_longer_than_what_is_playing_extends_it() {
        let mut queued = vec![0.5; 2];
        overlay(&mut queued, vec![0.25; 5]);
        assert_eq!(queued, vec![0.75, 0.75, 0.25, 0.25, 0.25]);
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

    #[test]
    fn someone_you_silenced_never_reaches_the_bus() {
        assert!(
            at_volume(&tone(0.5), 0.0).is_none(),
            "a silenced peer must be left off the bus entirely, not mixed in as zeroes"
        );
    }

    #[test]
    fn turning_someone_down_scales_their_voice() {
        let voice = tone(0.8);
        let quieter = at_volume(&voice, 0.25).expect("a quiet peer is still on the bus");

        assert_eq!(quieter.len(), voice.len(), "the frame must keep its length");
        for (scaled, original) in quieter.iter().zip(voice.iter()) {
            assert!(
                (scaled - original * 0.25).abs() < 1e-6,
                "expected {} at a quarter, got {scaled}",
                original * 0.25
            );
        }
    }
}
