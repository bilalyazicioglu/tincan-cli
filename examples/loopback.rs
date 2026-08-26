//! Faz 0 probe — ATILABILIR KOD.
//!
//! Tests the audio chain end to end: microphone → Opus encode → Opus decode → speaker.
//! What it really exercises is not the codec but **real-time discipline**: cpal's audio
//! callback runs in a real-time context (no allocation, no locks, no await), so the
//! bridge to the async world is built from lock-free ring buffers. If that bridge is
//! broken the audio crackles — the `underrun` counter here is its early warning system.
//!
//! Usage:
//!     cargo run --example loopback              # quiet: measure only, nothing played
//!     cargo run --example loopback -- --play    # WEAR HEADPHONES or it will howl
//!     cargo run --example loopback -- --devices # list the audio devices

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use audiopus::coder::{Decoder, Encoder};
use audiopus::{Application, Bitrate, Channels, SampleRate};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::RingBuffer;

/// The rate Opus supports and the one we chose for the mesh.
const SAMPLE_RATE: u32 = 48_000;
/// A 20 ms frame = 960 samples. The standard balance between latency and per-packet
/// overhead.
const FRAME: usize = 960;
/// The ring buffers hold ~200 ms, enough to absorb scheduler hiccups.
const RING_CAPACITY: usize = FRAME * 10;
const BITRATE: i32 = 32_000;

/// Counters the audio callbacks report from their real-time context.
/// Atomic increments only — no locks, no allocation.
#[derive(Default)]
struct Counters {
    /// The output callback found no data → an audible crackle. Should be zero.
    underruns: AtomicU64,
    /// The input callback found the ring buffer full → captured audio was dropped.
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

    println!("\n  Input devices:");
    for device in host.input_devices()? {
        let name = device_name(&device);
        let mark = if Some(&name) == default_in.as_ref() { " (default)" } else { "" };
        println!("    • {name}{mark}  {}", describe(device.default_input_config().ok().as_ref()));
    }
    println!("\n  Output devices:");
    for device in host.output_devices()? {
        let name = device_name(&device);
        let mark = if Some(&name) == default_out.as_ref() { " (default)" } else { "" };
        println!("    • {name}{mark}  {}", describe(device.default_output_config().ok().as_ref()));
    }
    println!();
    Ok(())
}

fn describe(cfg: Option<&cpal::SupportedStreamConfig>) -> String {
    match cfg {
        Some(c) => format!("[{} Hz, {} kanal, {:?}]", c.sample_rate(), c.channels(), c.sample_format()),
        None => "[config unreadable]".into(),
    }
}

fn run(play: bool) -> Result<()> {
    let host = cpal::default_host();
    let input = host.default_input_device().context("no input device")?;
    let output = host.default_output_device().context("no output device")?;

    let in_cfg = input.default_input_config()?;
    let out_cfg = output.default_output_config()?;
    println!("\n  input  : {}  {}", device_name(&input), describe(Some(&in_cfg)));
    println!("  output : {}  {}", device_name(&output), describe(Some(&out_cfg)));

    if in_cfg.sample_rate() != SAMPLE_RATE || out_cfg.sample_rate() != SAMPLE_RATE {
        // No resampling in the MVP; if a device is not 48 kHz we want to know early.
        bail!(
            "this probe assumes 48 kHz (input {} Hz, output {} Hz) — the MVP would need a resampler",
            in_cfg.sample_rate(),
            out_cfg.sample_rate()
        );
    }

    let in_channels = in_cfg.channels() as usize;
    let out_channels = out_cfg.channels() as usize;
    let counters = Arc::new(Counters::default());

    // Bridges between the audio thread and the processing thread. Single producer,
    // single consumer, lock-free.
    let (mut capture_tx, mut capture_rx) = RingBuffer::<f32>::new(RING_CAPACITY);
    let (mut playback_tx, mut playback_rx) = RingBuffer::<f32>::new(RING_CAPACITY);

    // ── Input stream: microphone → downmix to mono → ring buffer ─────────────────
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
        |err| eprintln!("  input stream error: {err}"),
        None,
    )?;

    // ── Output stream: ring buffer → speaker (silence + underrun when empty) ─────
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
        |err| eprintln!("  output stream error: {err}"),
        None,
    )?;

    in_stream.play()?;
    out_stream.play()?;

    if play {
        println!("\n  ⚠ --play is on: WEAR HEADPHONES, or the microphone will hear the speaker and howl.");
    } else {
        println!("\n  (quiet mode — nothing reaches the speaker; use --play to hear it)");
    }
    println!("  Measuring for 10 seconds, say something...\n");

    // ── Processing loop: capture → encode → decode → play ───────────────────────
    // In the real product the network sits between encode and decode; here we close
    // the loop.
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
    println!("  ── Results ──");

    if sizes.is_empty() {
        println!("  ⚠ no frames processed — microphone permission may not have been granted");
        return;
    }

    let frames = load(&counters.captured_frames);
    let avg_bytes = sizes.iter().sum::<usize>() as f64 / sizes.len() as f64;
    let kbps = avg_bytes * 8.0 * 50.0 / 1000.0; // 50 frames per second
    let avg_encode = times.iter().sum::<Duration>() / times.len() as u32;
    let worst_encode = times.iter().max().copied().unwrap_or_default();

    println!("  frames processed : {frames} (~{} seconds)", frames / 50);
    println!("  Opus packet      : avg {avg_bytes:.0} bytes → ~{kbps:.0} kbps per person");
    println!("  → worst case in a six-person mesh: ~{:.0} kbps upload", kbps * 5.0);
    println!("  encode+decode    : avg {avg_encode:?}, worst {worst_encode:?} (budget: 20 ms)");
    println!("  microphone peak  : {peak:.3} {}", if peak < 0.001 { "⚠ silent — the microphone may not be working" } else { "✓" });

    let underruns = load(&counters.underruns);
    let overruns = load(&counters.overruns);
    println!("  underruns        : {underruns} {}", if underruns == 0 { "✓" } else { "⚠ audible crackling" });
    println!("  overruns         : {overruns} {}", if overruns == 0 { "✓" } else { "⚠ captured audio is being dropped" });
    println!("\n  → the real-time bridge is {}\n",
        if underruns == 0 && overruns == 0 { "sound: the architecture can be carried forward as is" } else { "troubled: the ring buffer discipline needs another look" });
}
