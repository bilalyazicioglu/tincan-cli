//! Ses düzlemi: yakalama, kodlama, jitter tamponu, miksaj, çalma.

pub mod codec;
pub mod device;
pub mod jitter;
pub mod mixer;
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

/// Opus'un doğal çalışma oranı; tüm zincir bunun üzerine kurulu.
pub const SAMPLE_RATE: u32 = 48_000;
/// 20ms'lik çerçeve. Gecikme ile paket başı ek yük arasındaki standart denge.
pub const FRAME: usize = 960;
/// Kişi başı hedef bit hızı. 6 kişilik mesh'te ~160 kbps upload demek.
pub const BITRATE: i32 = 32_000;
/// Çerçeve süresi.
pub const FRAME_DURATION: Duration = Duration::from_millis(20);
/// Jitter tamponunun hedef derinliği (3 çerçeve ≈ 60ms).
const JITTER_TARGET: usize = 3;
/// Hoparlör tamponunda tutulmaya çalışılan çerçeve sayısı.
const PLAYBACK_TARGET_FRAMES: usize = 3;
/// Bir peer bu süre boyunca hiç paket göndermezse kaynakları serbest bırakılır.
const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Ağdan gelen bir ses çerçevesi.
#[derive(Debug, Clone)]
pub struct Incoming {
    pub from: PeerId,
    pub seq: u32,
    pub payload: Vec<u8>,
}

/// Ses motorunun dış dünyaya açılan uçları.
pub struct VoiceIo {
    /// Kodlanmış kendi sesimiz — ağ katmanı bunları mesh'e dağıtır.
    pub outgoing: mpsc::Receiver<Vec<u8>>,
    /// Ağdan gelen çerçeveler buraya yazılır.
    pub incoming: mpsc::Sender<Incoming>,
    /// O anda konuşanlar (kendimiz dahil) — arayüzdeki gösterge bunu dinler.
    pub speaking: watch::Receiver<HashSet<PeerId>>,
    /// Mikrofon kapalı mı. Ses motoru her çerçevede buna bakar.
    pub muted: Arc<AtomicBool>,
    pub health: Arc<AudioHealth>,
    /// Hayatta tutulduğu sürece ses donanımı açık kalır.
    pub devices: AudioDevices,
}

/// Mikrofonu ve hoparlörü açar, ses döngülerini başlatır.
pub fn start(me: PeerId) -> Result<VoiceIo> {
    let (devices, mut capture, mut playback, health) = device::open()?;

    let (outgoing_tx, outgoing_rx) = mpsc::channel::<Vec<u8>>(64);
    let (incoming_tx, mut incoming_rx) = mpsc::channel::<Incoming>(256);
    let (speaking_tx, speaking_rx) = watch::channel(HashSet::new());
    let muted = Arc::new(AtomicBool::new(false));

    // ── Yakalama: mikrofon → VAD → Opus → ağ ────────────────────────────────
    let capture_muted = muted.clone();
    let capture_speaking = speaking_tx.clone();
    tokio::spawn(async move {
        let mut encoder = match codec::Encoder::new() {
            Ok(encoder) => encoder,
            Err(err) => {
                tracing::error!("kodlayıcı başlatılamadı: {err:#}");
                return;
            }
        };
        let mut detector = Vad::default();
        let mut pcm = vec![0f32; FRAME];
        let mut ticker = tokio::time::interval(FRAME_DURATION);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;

            // Biriken tüm tam çerçeveleri işle: bir tick kaçarsa gecikme büyümesin.
            while capture.slots() >= FRAME {
                for slot in pcm.iter_mut() {
                    *slot = capture.pop().unwrap_or(0.0);
                }

                let muted_now = capture_muted.load(Ordering::Relaxed);
                // Susturulmuşken de VAD'i besliyoruz ki susturma kalkınca
                // hangover durumu tutarlı olsun; ama paket göndermiyoruz.
                let active = detector.update(&pcm) && !muted_now;

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
                        // Ağ yetişemiyorsa çerçeveyi düşürmek, kuyrukta
                        // bekletip gecikme biriktirmekten iyidir.
                        let _ = outgoing_tx.try_send(packet.to_vec());
                    }
                    Err(err) => tracing::warn!("kodlama hatası: {err:#}"),
                }
            }
        }
    });

    // ── Çalma: ağ → jitter → Opus çözme → miksaj → hoparlör ─────────────────
    tokio::spawn(async move {
        let mut streams: HashMap<PeerId, PeerStream> = HashMap::new();
        let mut mixer = Mixer::default();
        let mut ticker = tokio::time::interval(FRAME_DURATION);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let mut decoded = vec![0f32; FRAME];
        let mut mixed = vec![0f32; FRAME];

        loop {
            tokio::select! {
                Some(packet) = incoming_rx.recv() => {
                    let stream = match streams.get_mut(&packet.from) {
                        Some(stream) => stream,
                        None => match PeerStream::new() {
                            Ok(stream) => streams.entry(packet.from).or_insert(stream),
                            Err(err) => {
                                tracing::warn!("çözücü açılamadı: {err:#}");
                                continue;
                            }
                        },
                    };
                    stream.last_packet = Instant::now();
                    stream.buffer.push(packet.seq, packet.payload);
                }

                _ = ticker.tick() => {
                    // Hoparlör tamponunu hedef doluluğa kadar besle. Doluluğa
                    // bakmak, tick kaymalarını kendiliğinden telafi eder.
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
                                Err(err) => tracing::warn!("çözme hatası: {err:#}"),
                            }
                        }

                        let refs: Vec<&[f32]> = sources.iter().map(|s| s.as_slice()).collect();
                        mixer.mix(&refs, &mut mixed);
                        for sample in mixed.iter() {
                            let _ = playback.push(*sample);
                        }

                        speaking_tx.send_if_modified(|speakers| {
                            // Kendi konuşma durumumuzu yakalama tarafı yönetiyor;
                            // burada yalnızca karşı tarafları güncelliyoruz.
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
        muted,
        health,
        devices,
    })
}

/// Bir peer'dan gelen sesin durumu.
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
