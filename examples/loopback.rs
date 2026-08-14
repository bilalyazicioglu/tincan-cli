//! Faz 0 probe — ATILABILIR KOD.
//!
//! Ses zincirini uçtan uca test eder: mikrofon → Opus encode → Opus decode → hoparlör.
//! Asıl sınadığı şey kodek değil, **gerçek zamanlı disiplin**: cpal'ın ses callback'i
//! gerçek zamanlı bir bağlamda çalışır (bellek ayırma / kilit / await yasak), bu yüzden
//! async dünya ile arasındaki köprü kilitsiz ring buffer'larla kuruluyor. Bu köprü
//! bozuksa ses çıtırdar — buradaki `underrun` sayacı onun erken uyarı sistemidir.
//!
//! Kullanım:
//!     cargo run --example loopback              # sessiz: sadece ölçüm, hoparlöre çıkmaz
//!     cargo run --example loopback -- --play    # KULAKLIK TAKIN, yoksa uğultu olur
//!     cargo run --example loopback -- --devices # ses cihazlarını listele

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Bitrate, Channels, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::RingBuffer;

/// Opus'un desteklediği ve mesh için seçtiğimiz oran.
const SAMPLE_RATE: u32 = 48_000;
/// 20ms çerçeve = 960 örnek. Gecikme ile paket başı yük arasındaki standart denge.
const FRAME: usize = 960;
/// Ring buffer'lar ~200ms tutar; bu, planlayıcı gecikmelerini yutmaya yeter.
const RING_CAPACITY: usize = FRAME * 10;
const BITRATE: i32 = 32_000;

/// Ses callback'lerinin gerçek zamanlı bağlamdan raporladığı sayaçlar.
/// Sadece atomik artırma yapılır — kilit yok, ayırma yok.
#[derive(Default)]
struct Counters {
    /// Çıkış callback'i veri bulamadı → duyulabilir çıtırtı. Sıfır olmalı.
    underruns: AtomicU64,
    /// Giriş callback'i ring buffer'ı dolu buldu → yakalanan ses düştü.
    overruns: AtomicU64,
    captured_frames: AtomicU64,
    played_frames: AtomicU64,
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--devices") {
        return list_devices();
    }
    let play = args.iter().any(|a| a == "--play");
    run(play)
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "(isimsiz)".into())
}

fn list_devices() -> Result<()> {
    let host = cpal::default_host();
    let default_in = host.default_input_device().map(|d| device_name(&d));
    let default_out = host.default_output_device().map(|d| device_name(&d));

    println!("\n  Giriş cihazları:");
    for device in host.input_devices()? {
        let name = device_name(&device);
        let mark = if Some(&name) == default_in.as_ref() { " (varsayılan)" } else { "" };
        println!("    • {name}{mark}  {}", describe(device.default_input_config().ok().as_ref()));
    }
    println!("\n  Çıkış cihazları:");
    for device in host.output_devices()? {
        let name = device_name(&device);
        let mark = if Some(&name) == default_out.as_ref() { " (varsayılan)" } else { "" };
        println!("    • {name}{mark}  {}", describe(device.default_output_config().ok().as_ref()));
    }
    println!();
    Ok(())
}

fn describe(cfg: Option<&cpal::SupportedStreamConfig>) -> String {
    match cfg {
        Some(c) => format!("[{} Hz, {} kanal, {:?}]", c.sample_rate(), c.channels(), c.sample_format()),
        None => "[yapılandırma okunamadı]".into(),
    }
}

fn run(play: bool) -> Result<()> {
    let host = cpal::default_host();
    let input = host.default_input_device().context("giriş cihazı yok")?;
    let output = host.default_output_device().context("çıkış cihazı yok")?;

    let in_cfg = input.default_input_config()?;
    let out_cfg = output.default_output_config()?;
    println!("\n  giriş : {}  {}", device_name(&input), describe(Some(&in_cfg)));
    println!("  çıkış : {}  {}", device_name(&output), describe(Some(&out_cfg)));

    if in_cfg.sample_rate() != SAMPLE_RATE || out_cfg.sample_rate() != SAMPLE_RATE {
        // MVP'de yeniden örnekleme yok; cihaz 48kHz değilse bunu erken bilmek istiyoruz.
        bail!(
            "bu probe 48kHz varsayıyor (giriş {} Hz, çıkış {} Hz) — MVP'de resampler gerekecek",
            in_cfg.sample_rate(),
            out_cfg.sample_rate()
        );
    }

    let in_channels = in_cfg.channels() as usize;
    let out_channels = out_cfg.channels() as usize;
    let counters = Arc::new(Counters::default());

    // Ses thread'i ↔ işleme thread'i köprüleri. Tek üretici / tek tüketici, kilitsiz.
    let (mut capture_tx, mut capture_rx) = RingBuffer::<f32>::new(RING_CAPACITY);
    let (mut playback_tx, mut playback_rx) = RingBuffer::<f32>::new(RING_CAPACITY);

    // ── Giriş akışı: mikrofon → mono'ya indirge → ring buffer ────────────────────
    let in_counters = counters.clone();
    let in_stream = input.build_input_stream(
        in_cfg.config(),
        move |data: &[f32], _| {
            for chunk in data.chunks(in_channels) {
                let mono = chunk.iter().sum::<f32>() / in_channels as f32;
                if capture_tx.push(mono).is_err() {
                    in_counters.overruns.fetch_add(1, Ordering::Relaxed);
                }
            }
        },
        |err| eprintln!("  giriş akışı hatası: {err}"),
        None,
    )?;

    // ── Çıkış akışı: ring buffer → hoparlör (veri yoksa sessizlik + underrun) ────
    let out_counters = counters.clone();
    let out_stream = output.build_output_stream(
        out_cfg.config(),
        move |data: &mut [f32], _| {
            for chunk in data.chunks_mut(out_channels) {
                let sample = match playback_rx.pop() {
                    Ok(s) => s,
                    Err(_) => {
                        out_counters.underruns.fetch_add(1, Ordering::Relaxed);
                        0.0
                    }
                };
                chunk.fill(if play { sample } else { 0.0 });
            }
        },
        |err| eprintln!("  çıkış akışı hatası: {err}"),
        None,
    )?;

    in_stream.play()?;
    out_stream.play()?;

    if play {
        println!("\n  ⚠ --play açık: KULAKLIK TAKIN, yoksa mikrofon hoparlörü duyar ve uğuldar.");
    } else {
        println!("\n  (sessiz mod — hoparlöre çıkmıyor; duymak için --play)");
    }
    println!("  10 saniye ölçülüyor, konuşun...\n");

    // ── İşleme döngüsü: yakala → encode → decode → oynat ────────────────────────
    // Gerçek üründe encode ile decode arasında ağ var; burada zinciri kapatıyoruz.
    let mut encoder = Encoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
    encoder.set_bitrate(Bitrate::BitsPerSecond(BITRATE))?;
    let mut decoder = Decoder::new(SampleRate::Hz48000, Channels::Mono)?;

    let mut pcm = vec![0f32; FRAME];
    let mut packet = vec![0u8; 4000];
    let mut decoded = vec![0f32; FRAME];

    let mut packet_sizes: Vec<usize> = Vec::new();
    let mut encode_times: Vec<Duration> = Vec::new();
    let mut peak = 0f32;

    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if capture_rx.slots() < FRAME {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        for slot in pcm.iter_mut() {
            *slot = capture_rx.pop().unwrap_or(0.0);
        }
        peak = peak.max(pcm.iter().fold(0f32, |m, s| m.max(s.abs())));
        counters.captured_frames.fetch_add(1, Ordering::Relaxed);

        let t0 = Instant::now();
        let encoded = encoder.encode_float(&pcm, &mut packet)?;
        let n = decoder.decode_float(Some(&packet[..encoded]), &mut decoded, false)?;
        encode_times.push(t0.elapsed());
        packet_sizes.push(encoded);

        for &sample in &decoded[..n] {
            let _ = playback_tx.push(sample);
        }
        counters.played_frames.fetch_add(1, Ordering::Relaxed);
    }

    drop(in_stream);
    drop(out_stream);
    report(&counters, &packet_sizes, &encode_times, peak);
    Ok(())
}

fn report(counters: &Counters, sizes: &[usize], times: &[Duration], peak: f32) {
    let load = |c: &AtomicU64| c.load(Ordering::Relaxed);
    println!("  ── Sonuçlar ──");

    if sizes.is_empty() {
        println!("  ⚠ hiç çerçeve işlenmedi — mikrofon izni verilmemiş olabilir");
        return;
    }

    let frames = load(&counters.captured_frames);
    let avg_bytes = sizes.iter().sum::<usize>() as f64 / sizes.len() as f64;
    let kbps = avg_bytes * 8.0 * 50.0 / 1000.0; // 50 çerçeve/saniye
    let avg_encode = times.iter().sum::<Duration>() / times.len() as u32;
    let worst_encode = times.iter().max().copied().unwrap_or_default();

    println!("  işlenen çerçeve : {frames} (~{} saniye)", frames / 50);
    println!("  Opus paketi     : ort {avg_bytes:.0} bayt → ~{kbps:.0} kbps kişi başı");
    println!("  → 6 kişilik mesh'te en kötü durum: ~{:.0} kbps upload", kbps * 5.0);
    println!("  encode+decode   : ort {avg_encode:?}, en kötü {worst_encode:?} (bütçe: 20ms)");
    println!("  mikrofon tepesi : {peak:.3} {}", if peak < 0.001 { "⚠ sessiz — mikrofon çalışmıyor olabilir" } else { "✓" });

    let underruns = load(&counters.underruns);
    let overruns = load(&counters.overruns);
    println!("  underrun        : {underruns} {}", if underruns == 0 { "✓" } else { "⚠ çıtırtı yaşanıyor" });
    println!("  overrun         : {overruns} {}", if overruns == 0 { "✓" } else { "⚠ yakalanan ses düşüyor" });
    println!("\n  → gerçek zamanlı köprü {}\n",
        if underruns == 0 && overruns == 0 { "sağlam: mimari bu haliyle taşınabilir" } else { "sorunlu: ring buffer disiplini gözden geçirilmeli" });
}
