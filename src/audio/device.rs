//! The bridge between the audio hardware and the async world.
//!
//! cpal's callbacks run in a real-time context: allocating, taking a lock or awaiting
//! inside one causes a dropout (a crackle). So the callbacks touch nothing but a
//! lock-free ring buffer; encoding, networking and mixing all happen in ordinary
//! tasks.
//!
//! ```text
//! [microphone callback] → ring → [encoder task] → network
//! network → [decoder + mixer task] → ring → [speaker callback]
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};

use super::{FRAME, SAMPLE_RATE};

/// How much audio the ring buffers hold (~200 ms).
const RING_CAPACITY: usize = FRAME * 10;

/// Counters the real-time callbacks report to the outside world.
#[derive(Default)]
pub struct AudioHealth {
    /// The speaker found no data → an audible dropout.
    pub underruns: AtomicU64,
    /// Microphone data overflowed the buffer → captured audio was dropped.
    pub overruns: AtomicU64,
}

impl AudioHealth {
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }
}

/// What `open()` returns: the live streams, the capture end, the playback end and
/// the health counters.
pub type OpenAudio = (AudioDevices, Consumer<f32>, Producer<f32>, Arc<AudioHealth>);

/// The open audio streams. Dropping this releases the hardware.
pub struct AudioDevices {
    _input: cpal::Stream,
    _output: cpal::Stream,
}

/// Which device to use. `None` means the system default.
#[derive(Debug, Clone, Default)]
pub struct DeviceChoice {
    pub input: Option<String>,
    pub output: Option<String>,
}

/// Picks a device by name. Matching is case-insensitive and partial, so the user can
/// type any distinctive part of a name from the `tincan devices` output.
fn pick(mut devices: impl Iterator<Item = cpal::Device>, wanted: &str) -> Option<cpal::Device> {
    let wanted = wanted.to_lowercase();
    devices.find(|d| {
        d.description()
            .map(|desc| desc.name().to_lowercase().contains(&wanted))
            .unwrap_or(false)
    })
}

/// Opens the microphone and speaker and returns the capture and playback ends.
pub fn open(choice: &DeviceChoice) -> Result<OpenAudio> {
    let host = cpal::default_host();

    let input = match &choice.input {
        Some(name) => pick(host.input_devices()?, name)
            .with_context(|| {
                format!("no microphone named '{name}' (list them with: tincan devices)")
            })?,
        None => host.default_input_device().context("no microphone found")?,
    };
    let output = match &choice.output {
        Some(name) => pick(host.output_devices()?, name)
            .with_context(|| {
                format!("no speaker named '{name}' (list them with: tincan devices)")
            })?,
        None => host.default_output_device().context("no speaker found")?,
    };

    let in_cfg = input.default_input_config().context("could not read the microphone config")?;
    let out_cfg = output.default_output_config().context("could not read the speaker config")?;

    // There is no resampling in the MVP. Better to say so plainly than to produce
    // broken audio in silence.
    if in_cfg.sample_rate() != SAMPLE_RATE || out_cfg.sample_rate() != SAMPLE_RATE {
        bail!(
            "audio devices must run at 48 kHz (microphone {} Hz, speaker {} Hz). \
             You can select 48000 Hz in your sound settings.",
            in_cfg.sample_rate(),
            out_cfg.sample_rate()
        );
    }

    let in_channels = in_cfg.channels() as usize;
    let out_channels = out_cfg.channels() as usize;
    let health = Arc::new(AudioHealth::default());

    let (mut capture_tx, capture_rx) = RingBuffer::<f32>::new(RING_CAPACITY);
    let (playback_tx, mut playback_rx) = RingBuffer::<f32>::new(RING_CAPACITY);

    let capture_health = health.clone();
    let input_stream = input
        .build_input_stream(
            in_cfg.config(),
            move |data: &[f32], _| {
                for chunk in data.chunks(in_channels) {
                    let mono = chunk.iter().sum::<f32>() / in_channels as f32;
                    if capture_tx.push(mono).is_err() {
                        capture_health.overruns.fetch_add(1, Ordering::Relaxed);
                    }
                }
            },
            |err| tracing::warn!("microphone error: {err}"),
            None,
        )
        .context("could not open the microphone stream")?;

    let playback_health = health.clone();
    let output_stream = output
        .build_output_stream(
            out_cfg.config(),
            move |data: &mut [f32], _| {
                for chunk in data.chunks_mut(out_channels) {
                    let sample = match playback_rx.pop() {
                        Ok(sample) => sample,
                        Err(_) => {
                            playback_health.underruns.fetch_add(1, Ordering::Relaxed);
                            0.0
                        }
                    };
                    // Spread the mono source across every channel.
                    chunk.fill(sample);
                }
            },
            |err| tracing::warn!("speaker error: {err}"),
            None,
        )
        .context("could not open the speaker stream")?;

    input_stream.play().context("could not start the microphone")?;
    output_stream.play().context("could not start the speaker")?;

    Ok((
        AudioDevices {
            _input: input_stream,
            _output: output_stream,
        },
        capture_rx,
        playback_tx,
        health,
    ))
}

/// Lists the system's audio devices (`tincan devices`).
pub fn describe_devices() -> Result<String> {
    let host = cpal::default_host();
    let mut report = String::new();

    let default_in = host
        .default_input_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));
    let default_out = host
        .default_output_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));

    report.push_str("\n  Microphones:\n");
    for device in host.input_devices()? {
        report.push_str(&line(&device, &default_in, true));
    }
    report.push_str("\n  Speakers:\n");
    for device in host.output_devices()? {
        report.push_str(&line(&device, &default_out, false));
    }
    report.push_str("\n  Note: tincan currently works only with 48000 Hz devices.\n");
    Ok(report)
}

fn line(device: &cpal::Device, default: &Option<String>, input: bool) -> String {
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "(unnamed)".into());
    let config = if input {
        device.default_input_config().ok()
    } else {
        device.default_output_config().ok()
    };
    let rate = config
        .map(|c| format!("{} Hz, {} ch", c.sample_rate(), c.channels()))
        .unwrap_or_else(|| "config unreadable".into());
    let mark = if Some(&name) == default.as_ref() { " ←" } else { "" };
    format!("    • {name}  [{rate}]{mark}\n")
}
