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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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
    /// Set when the driver takes a stream away under us — a device unplugged, or its
    /// sample rate changed. Written from the audio callback, which can do nothing
    /// about it itself, and acted on by `recover`.
    input_lost: Arc<AtomicBool>,
    output_lost: Arc<AtomicBool>,
    /// Remembered devices that were not there when we started, and had to be replaced
    /// by the default.
    missing: Arc<std::sync::Mutex<Vec<String>>>,
}

/// Which end of the audio a piece of news is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Microphone,
    Speaker,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::Speaker => "speaker",
        }
    }
}

/// A stream the driver took away, and what came of trying to get it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub side: Side,
    /// The device it reopened on, or `None` when it could not be reopened at all.
    pub device: Option<String>,
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
                on_error(&self.input_lost, "microphone"),
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
                on_error(&self.input_lost, "microphone"),
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
                on_error(&self.input_lost, "microphone"),
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

    /// Remembered devices that were not plugged in at start-up.
    pub fn missing(&self) -> Vec<String> {
        match self.missing.lock() {
            Ok(lock) => lock.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Reopens any stream the driver took away.
    ///
    /// macOS hands a stream back when the device's sample rate changes underneath it —
    /// which is exactly what a Bluetooth headset does when its microphone opens and it
    /// switches profile. The stream is dead at that point and the resampler behind it
    /// is built for a rate that no longer exists, so the only cure is to open it again
    /// and read the new configuration.
    ///
    /// Called off the audio thread, once a second. A failure leaves the flag up so the
    /// next second tries again.
    pub fn recover(&self) -> Vec<Recovered> {
        let mut news = Vec::new();
        for side in [Side::Microphone, Side::Speaker] {
            let (lost, active) = match side {
                Side::Microphone => (&self.input_lost, &self.active_input),
                Side::Speaker => (&self.output_lost, &self.active_output),
            };
            if !lost.swap(false, Ordering::Relaxed) {
                continue;
            }

            let wanted = read(active);
            let reopen = |name: Option<&str>| match side {
                Side::Microphone => self.switch_input(name),
                Side::Speaker => self.switch_output(name),
            };
            // The device it was on may itself be what disappeared, so the default is
            // the fallback here too.
            match reopen(wanted.as_deref()).or_else(|_| reopen(None)) {
                Ok(device) => news.push(Recovered { side, device: Some(device) }),
                Err(err) => {
                    tracing::warn!("could not reopen the {}: {err:#}", side.name());
                    lost.store(true, Ordering::Relaxed);
                    news.push(Recovered { side, device: None });
                }
            }
        }
        news
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
                        // The integer formats below clamp; this one used to hand
                        // out-of-range floats straight to the driver.
                        chunk.fill(sample.clamp(-1.0, 1.0));
                    }
                },
                on_error(&self.output_lost, "speaker"),
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
                on_error(&self.output_lost, "speaker"),
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
                on_error(&self.output_lost, "speaker"),
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

/// Which device to use, and how hard to insist on it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Wanted {
    /// Whatever the system calls default.
    #[default]
    Default,
    /// Named on the command line. The user typed it, so not finding it is an error.
    Named(String),
    /// Remembered from last time. Hardware gets unplugged, so not finding it is
    /// ordinary and we fall back to the default.
    Remembered(String),
}

impl Wanted {
    /// A device typed on the command line outranks one remembered from last time.
    pub fn pick(named: Option<String>, remembered: Option<String>) -> Self {
        match (named, remembered) {
            (Some(name), _) => Self::Named(name),
            (None, Some(name)) => Self::Remembered(name),
            (None, None) => Self::Default,
        }
    }
}

/// Which devices to use.
#[derive(Debug, Clone, Default)]
pub struct DeviceChoice {
    pub input: Wanted,
    pub output: Wanted,
}

/// Picks a device by name. Matching is case-insensitive and partial, so the user can
/// type any distinctive part of a name from the `tincan devices` output.
/// The audio callback cannot rebuild its own stream, so all it does is say that the
/// stream is gone. `recover` picks it up from there.
fn on_error(
    lost: &Arc<AtomicBool>,
    side: &'static str,
) -> impl FnMut(cpal::Error) + Send + 'static {
    let lost = lost.clone();
    move |err| {
        tracing::warn!("{side} error: {err}");
        lost.store(true, Ordering::Relaxed);
    }
}

/// Opens the wanted device.
///
/// A device named on the command line is a demand: not finding it is an error, because
/// the user typed it. A device remembered from last time is a preference — hardware
/// gets unplugged — and losing every bit of audio over a headset that is not on the
/// desk today is far worse than quietly using the built-in one. Returns the device it
/// opened, and the name it was looking for when it had to give up on it.
fn open_wanted(
    wanted: &Wanted,
    open: impl Fn(Option<&str>) -> Result<String>,
) -> Result<(String, Option<String>)> {
    match wanted {
        Wanted::Default => Ok((open(None)?, None)),
        Wanted::Named(name) => Ok((open(Some(name))?, None)),
        Wanted::Remembered(name) => match open(Some(name)) {
            Ok(opened) => Ok((opened, None)),
            Err(_) => Ok((open(None)?, Some(name.clone()))),
        },
    }
}

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
        input_lost: Arc::new(AtomicBool::new(false)),
        output_lost: Arc::new(AtomicBool::new(false)),
        missing: Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let (_, lost_input) = open_wanted(&choice.input, |name| devices.switch_input(name))
        .context("could not initialize microphone")?;
    let (_, lost_output) = open_wanted(&choice.output, |name| devices.switch_output(name))
        .context("could not initialize speaker")?;

    for (missing, replacement) in [
        (lost_input, devices.active_input()),
        (lost_output, devices.active_output()),
    ] {
        if let Some(missing) = missing {
            let note = match replacement {
                Some(using) => format!("{missing} is not here — using {using}"),
                None => format!("{missing} is not here"),
            };
            if let Ok(mut lock) = devices.missing.lock() {
                lock.push(note);
            }
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for the sound card: `present` is what is plugged in, and `None` asks
    /// for the default the way `switch_*` does.
    fn hardware<'a>(present: &'a [&'a str]) -> impl Fn(Option<&str>) -> Result<String> + 'a {
        move |wanted| match wanted {
            None => present
                .first()
                .map(|name| name.to_string())
                .context("no default device"),
            Some(name) => present
                .iter()
                .find(|have| have.to_lowercase().contains(&name.to_lowercase()))
                .map(|have| have.to_string())
                .with_context(|| format!("no device named '{name}'")),
        }
    }

    #[test]
    fn a_device_typed_on_the_command_line_outranks_one_remembered() {
        assert_eq!(
            Wanted::pick(Some("QCY".into()), Some("MacBook".into())),
            Wanted::Named("QCY".into())
        );
        assert_eq!(Wanted::pick(None, Some("MacBook".into())), Wanted::Remembered("MacBook".into()));
        assert_eq!(Wanted::pick(None, None), Wanted::Default);
    }

    #[test]
    fn a_remembered_device_that_is_unplugged_falls_back_to_the_default() {
        // The bug this exists for: a headset remembered from yesterday and not on the
        // desk today used to take every bit of audio down with it.
        let devices = hardware(&["MacBook Pro Microphone"]);
        let (opened, missing) = open_wanted(&Wanted::Remembered("QCY H4".into()), &devices)
            .expect("a missing preference must not be fatal");

        assert_eq!(opened, "MacBook Pro Microphone");
        assert_eq!(missing.as_deref(), Some("QCY H4"), "and the room has to be told why");
    }

    #[test]
    fn a_device_named_on_the_command_line_is_an_error_when_it_is_missing() {
        let devices = hardware(&["MacBook Pro Microphone"]);
        assert!(
            open_wanted(&Wanted::Named("QCY H4".into()), &devices).is_err(),
            "the user typed it, so silently using something else would be a lie"
        );
    }

    #[test]
    fn a_remembered_device_that_is_here_is_simply_used() {
        let devices = hardware(&["MacBook Pro Microphone", "QCY H4"]);
        let (opened, missing) = open_wanted(&Wanted::Remembered("QCY".into()), &devices).unwrap();
        assert_eq!(opened, "QCY H4");
        assert_eq!(missing, None, "nothing went wrong, so there is nothing to report");
    }

    #[test]
    fn with_no_preference_at_all_the_default_is_opened() {
        let devices = hardware(&["MacBook Pro Microphone"]);
        let (opened, missing) = open_wanted(&Wanted::Default, &devices).unwrap();
        assert_eq!(opened, "MacBook Pro Microphone");
        assert_eq!(missing, None);
    }

    #[test]
    fn a_machine_with_no_devices_at_all_still_fails() {
        let devices = hardware(&[]);
        assert!(open_wanted(&Wanted::Remembered("QCY H4".into()), &devices).is_err());
        assert!(open_wanted(&Wanted::Default, &devices).is_err());
    }
}
