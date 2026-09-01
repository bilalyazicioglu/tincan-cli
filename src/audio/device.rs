//! The bridge between the audio hardware and the async world.
//!
//! cpal's callbacks run in a real-time context: allocating, taking a lock or awaiting
//! inside one causes a dropout (a crackle). So the callbacks touch nothing but a
//! lock-free ring buffer; encoding, networking and mixing all happen in ordinary
//! tasks.
//!
//! Built-in high-quality cubic resampling allows any sample rate (e.g. 16 kHz Bluetooth HFP,
//! 44.1 kHz USB audio) to work seamlessly with tincan's native 48 kHz pipeline.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};

use super::resample::Resampler;
use super::{FRAME, SAMPLE_RATE};

/// How much audio the ring buffers hold (~200 ms at 48 kHz).
const RING_CAPACITY: usize = FRAME * 10;

/// Information about a detected audio hardware device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub is_default: bool,
    pub is_supported: bool,
}

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

/// What `open()` returns: the live streams controller, the capture end, the playback end and
/// the health counters.
pub type OpenAudio = (AudioDevices, Consumer<f32>, Producer<f32>, Arc<AudioHealth>);

/// The open audio streams. Allows dynamic switching of input/output devices at runtime.
pub struct AudioDevices {
    input_stream: Arc<std::sync::Mutex<Option<cpal::Stream>>>,
    output_stream: Arc<std::sync::Mutex<Option<cpal::Stream>>>,
    capture_tx: Arc<std::sync::Mutex<Producer<f32>>>,
    playback_rx: Arc<std::sync::Mutex<Consumer<f32>>>,
    health: Arc<AudioHealth>,
    /// The devices actually in use. `switch_*` resolves a partial name or a default
    /// into a real one, and the interface needs to show what it landed on.
    active_input: Arc<std::sync::Mutex<Option<String>>>,
    active_output: Arc<std::sync::Mutex<Option<String>>>,
}

impl AudioDevices {
    /// Switches the input device dynamically at runtime.
    /// Returns the name of the activated device on success.
    pub fn switch_input(&self, wanted: Option<&str>) -> Result<String> {
        let host = cpal::default_host();
        let device = match wanted {
            Some(name) => pick(host.input_devices()?, name)
                .with_context(|| format!("no microphone named '{name}'"))?,
            None => host.default_input_device().context("no default microphone found")?,
        };

        let in_cfg = device.default_input_config().context("could not read microphone config")?;
        let in_rate = in_cfg.sample_rate();
        if in_rate == 0 {
            bail!("invalid sample rate reported by microphone");
        }

        let dev_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "Microphone".into());
        let in_channels = in_cfg.channels() as usize;
        let capture_tx = self.capture_tx.clone();
        let capture_health = self.health.clone();

        let mut resampler = Resampler::new(in_rate, SAMPLE_RATE);
        let mut raw_mono = Vec::new();
        let mut resampled_48k = Vec::new();

        let stream = match in_cfg.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                in_cfg.config(),
                move |data: &[f32], _| {
                    raw_mono.clear();
                    resampled_48k.clear();
                    for chunk in data.chunks(in_channels) {
                        let mono = chunk.iter().sum::<f32>() / in_channels as f32;
                        raw_mono.push(mono);
                    }
                    resampler.process(&raw_mono, &mut resampled_48k);

                    let mut guard = match capture_tx.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    for sample in &resampled_48k {
                        if guard.push(*sample).is_err() {
                            capture_health.overruns.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                },
                |err| tracing::warn!("microphone error: {err}"),
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                in_cfg.config(),
                move |data: &[i16], _| {
                    raw_mono.clear();
                    resampled_48k.clear();
                    for chunk in data.chunks(in_channels) {
                        let mono = chunk.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / in_channels as f32;
                        raw_mono.push(mono);
                    }
                    resampler.process(&raw_mono, &mut resampled_48k);

                    let mut guard = match capture_tx.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    for sample in &resampled_48k {
                        if guard.push(*sample).is_err() {
                            capture_health.overruns.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                },
                |err| tracing::warn!("microphone error: {err}"),
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                in_cfg.config(),
                move |data: &[u16], _| {
                    raw_mono.clear();
                    resampled_48k.clear();
                    for chunk in data.chunks(in_channels) {
                        let mono = chunk.iter().map(|&s| (s as f32 - 32768.0) / 32768.0).sum::<f32>() / in_channels as f32;
                        raw_mono.push(mono);
                    }
                    resampler.process(&raw_mono, &mut resampled_48k);

                    let mut guard = match capture_tx.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };
                    for sample in &resampled_48k {
                        if guard.push(*sample).is_err() {
                            capture_health.overruns.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                },
                |err| tracing::warn!("microphone error: {err}"),
                None,
            ),
            format => bail!("unsupported microphone sample format: {format:?}"),
        }
        .context("could not open microphone stream")?;

        stream.play().context("could not start microphone stream")?;

        let mut lock = match self.input_stream.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *lock = Some(stream);
        remember(&self.active_input, &dev_name);
        Ok(dev_name)
    }

    /// The microphone currently open.
    pub fn active_input(&self) -> Option<String> {
        read(&self.active_input)
    }

    /// The speaker currently open.
    pub fn active_output(&self) -> Option<String> {
        read(&self.active_output)
    }

    /// Switches the output device dynamically at runtime.
    /// Returns the name of the activated device on success.
    pub fn switch_output(&self, wanted: Option<&str>) -> Result<String> {
        let host = cpal::default_host();
        let device = match wanted {
            Some(name) => pick(host.output_devices()?, name)
                .with_context(|| format!("no speaker named '{name}'"))?,
            None => host.default_output_device().context("no default speaker found")?,
        };

        let out_cfg = device.default_output_config().context("could not read speaker config")?;
        let out_rate = out_cfg.sample_rate();
        if out_rate == 0 {
            bail!("invalid sample rate reported by speaker");
        }

        let dev_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "Speaker".into());
        let out_channels = out_cfg.channels() as usize;
        let playback_rx = self.playback_rx.clone();
        let playback_health = self.health.clone();

        let mut resampler = Resampler::new(SAMPLE_RATE, out_rate);
        let mut pcm_48k = Vec::new();
        let mut resampled_out = Vec::new();
        let mut queued_out = Vec::new();

        let stream = match out_cfg.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                out_cfg.config(),
                move |data: &mut [f32], _| {
                    let needed_samples = data.len() / out_channels;
                    let mut guard = match playback_rx.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };

                    while queued_out.len() < needed_samples {
                        pcm_48k.clear();
                        let pull_count = (needed_samples - queued_out.len()).max(32) * (SAMPLE_RATE as usize) / (out_rate as usize + 1);
                        let pull_count = pull_count.max(16);
                        for _ in 0..pull_count {
                            match guard.pop() {
                                Ok(sample) => pcm_48k.push(sample),
                                Err(_) => {
                                    playback_health.underruns.fetch_add(1, Ordering::Relaxed);
                                    pcm_48k.push(0.0);
                                }
                            }
                        }
                        resampled_out.clear();
                        resampler.process(&pcm_48k, &mut resampled_out);
                        queued_out.extend_from_slice(&resampled_out);
                    }

                    for chunk in data.chunks_mut(out_channels) {
                        let sample = if !queued_out.is_empty() {
                            queued_out.remove(0)
                        } else {
                            0.0
                        };
                        chunk.fill(sample);
                    }
                },
                |err| tracing::warn!("speaker error: {err}"),
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                out_cfg.config(),
                move |data: &mut [i16], _| {
                    let needed_samples = data.len() / out_channels;
                    let mut guard = match playback_rx.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };

                    while queued_out.len() < needed_samples {
                        pcm_48k.clear();
                        let pull_count = (needed_samples - queued_out.len()).max(32) * (SAMPLE_RATE as usize) / (out_rate as usize + 1);
                        let pull_count = pull_count.max(16);
                        for _ in 0..pull_count {
                            match guard.pop() {
                                Ok(sample) => pcm_48k.push(sample),
                                Err(_) => {
                                    playback_health.underruns.fetch_add(1, Ordering::Relaxed);
                                    pcm_48k.push(0.0);
                                }
                            }
                        }
                        resampled_out.clear();
                        resampler.process(&pcm_48k, &mut resampled_out);
                        queued_out.extend_from_slice(&resampled_out);
                    }

                    for chunk in data.chunks_mut(out_channels) {
                        let sample = if !queued_out.is_empty() {
                            queued_out.remove(0)
                        } else {
                            0.0
                        };
                        let i16_sample = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
                        chunk.fill(i16_sample);
                    }
                },
                |err| tracing::warn!("speaker error: {err}"),
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                out_cfg.config(),
                move |data: &mut [u16], _| {
                    let needed_samples = data.len() / out_channels;
                    let mut guard = match playback_rx.lock() {
                        Ok(g) => g,
                        Err(poisoned) => poisoned.into_inner(),
                    };

                    while queued_out.len() < needed_samples {
                        pcm_48k.clear();
                        let pull_count = (needed_samples - queued_out.len()).max(32) * (SAMPLE_RATE as usize) / (out_rate as usize + 1);
                        let pull_count = pull_count.max(16);
                        for _ in 0..pull_count {
                            match guard.pop() {
                                Ok(sample) => pcm_48k.push(sample),
                                Err(_) => {
                                    playback_health.underruns.fetch_add(1, Ordering::Relaxed);
                                    pcm_48k.push(0.0);
                                }
                            }
                        }
                        resampled_out.clear();
                        resampler.process(&pcm_48k, &mut resampled_out);
                        queued_out.extend_from_slice(&resampled_out);
                    }

                    for chunk in data.chunks_mut(out_channels) {
                        let sample = if !queued_out.is_empty() {
                            queued_out.remove(0)
                        } else {
                            0.0
                        };
                        let u16_sample = ((sample.clamp(-1.0, 1.0) * 32767.0) + 32768.0) as u16;
                        chunk.fill(u16_sample);
                    }
                },
                |err| tracing::warn!("speaker error: {err}"),
                None,
            ),
            format => bail!("unsupported speaker sample format: {format:?}"),
        }
        .context("could not open speaker stream")?;

        stream.play().context("could not start speaker stream")?;

        let mut lock = match self.output_stream.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *lock = Some(stream);
        remember(&self.active_output, &dev_name);
        Ok(dev_name)
    }
}

/// A poisoned lock here means another thread panicked mid-switch; the name it was
/// writing is worth less than staying up, so the guard is taken either way.
fn remember(slot: &Arc<std::sync::Mutex<Option<String>>>, name: &str) {
    let mut lock = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    *lock = Some(name.to_string());
}

fn read(slot: &Arc<std::sync::Mutex<Option<String>>>) -> Option<String> {
    match slot.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
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

/// Lists all available input devices.
pub fn list_input_devices() -> Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));

    let mut list = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for dev in devices {
            let name = dev
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| "(unnamed)".into());
            let (rate, channels) = dev
                .default_input_config()
                .map(|c| (c.sample_rate(), c.channels()))
                .unwrap_or((0, 0));
            let is_default = default_name.as_deref() == Some(&name);
            let is_supported = rate > 0;
            list.push(AudioDeviceInfo {
                name,
                sample_rate: rate,
                channels,
                is_default,
                is_supported,
            });
        }
    }
    Ok(list)
}

/// Lists all available output devices.
pub fn list_output_devices() -> Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));

    let mut list = Vec::new();
    if let Ok(devices) = host.output_devices() {
        for dev in devices {
            let name = dev
                .description()
                .map(|d| d.name().to_string())
                .unwrap_or_else(|_| "(unnamed)".into());
            let (rate, channels) = dev
                .default_output_config()
                .map(|c| (c.sample_rate(), c.channels()))
                .unwrap_or((0, 0));
            let is_default = default_name.as_deref() == Some(&name);
            let is_supported = rate > 0;
            list.push(AudioDeviceInfo {
                name,
                sample_rate: rate,
                channels,
                is_default,
                is_supported,
            });
        }
    }
    Ok(list)
}

/// Opens the microphone and speaker and returns the capture and playback ends.
pub fn open(choice: &DeviceChoice) -> Result<OpenAudio> {
    let health = Arc::new(AudioHealth::default());
    let (capture_tx, capture_rx) = RingBuffer::<f32>::new(RING_CAPACITY);
    let (playback_tx, playback_rx) = RingBuffer::<f32>::new(RING_CAPACITY);

    let devices = AudioDevices {
        input_stream: Arc::new(std::sync::Mutex::new(None)),
        output_stream: Arc::new(std::sync::Mutex::new(None)),
        capture_tx: Arc::new(std::sync::Mutex::new(capture_tx)),
        playback_rx: Arc::new(std::sync::Mutex::new(playback_rx)),
        health: health.clone(),
        active_input: Arc::new(std::sync::Mutex::new(None)),
        active_output: Arc::new(std::sync::Mutex::new(None)),
    };

    devices
        .switch_input(choice.input.as_deref())
        .context("could not initialize microphone")?;
    devices
        .switch_output(choice.output.as_deref())
        .context("could not initialize speaker")?;

    Ok((devices, capture_rx, playback_tx, health))
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

    report.push_str("\n  MICROPHONES\n");
    for device in host.input_devices()? {
        report.push_str(&line(&device, &default_in, true));
    }
    report.push_str("\n  SPEAKERS\n");
    for device in host.output_devices()? {
        report.push_str(&line(&device, &default_out, false));
    }
    report.push_str("\n  Every rate is resampled — 16 kHz Bluetooth headsets included.\n");
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
        .map(|c| format!("{} kHz · {} ch", c.sample_rate() / 1000, c.channels()))
        .unwrap_or_else(|| "reports no format".into());
    let mark = if Some(&name) == default.as_ref() { "default" } else { "" };
    // One column for the name, one for what it runs at, one for whether it is the
    // one you get by default.
    format!("    {name:<38}  {rate:<16}  {mark}\n")
}
