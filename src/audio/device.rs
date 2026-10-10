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
    input: DeviceRoute,
    output: DeviceRoute,
    /// Remembered devices that were not there when we started.
    missing: Arc<std::sync::Mutex<Vec<String>>>,
}

/// Which end of the audio a piece of news is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// A stream that moved to another device or was recovered after a driver error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub side: Side,
    /// The device it reopened on, or `None` when it could not be reopened at all.
    pub device: Option<String>,
}

impl AudioDevices {
    /// Switches the input device at runtime, as a choice: a named device stops the
    /// microphone following the system default, `None` starts it again.
    /// Returns the name of the activated device on success.
    pub fn switch_input(&self, wanted: Option<&str>) -> Result<String> {
        self.input.switch(wanted, |name| self.open_input(name))
    }

    /// Switches the output device at runtime, as a choice. See `switch_input`.
    pub fn switch_output(&self, wanted: Option<&str>) -> Result<String> {
        self.output.switch(wanted, |name| self.open_output(name))
    }

    /// Opens a microphone stream without saying anything about whether it was chosen.
    fn open_input(&self, wanted: Option<&str>) -> Result<String> {
        let host = cpal::default_host();
        let device = match wanted {
            Some(name) => pick(host.input_devices()?, name)
                .with_context(|| format!("no microphone named '{name}'"))?,
            None => host
                .default_input_device()
                .context("no default microphone found")?,
        };

        let in_cfg = device
            .default_input_config()
            .context("could not read microphone config")?;
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
                on_error(&self.input.lost, "microphone"),
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                in_cfg.config(),
                move |data: &[i16], _| {
                    raw_mono.clear();
                    resampled_48k.clear();
                    for chunk in data.chunks(in_channels) {
                        let mono = chunk.iter().map(|&s| s as f32 / 32768.0).sum::<f32>()
                            / in_channels as f32;
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
                on_error(&self.input.lost, "microphone"),
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                in_cfg.config(),
                move |data: &[u16], _| {
                    raw_mono.clear();
                    resampled_48k.clear();
                    for chunk in data.chunks(in_channels) {
                        let mono = chunk
                            .iter()
                            .map(|&s| (s as f32 - 32768.0) / 32768.0)
                            .sum::<f32>()
                            / in_channels as f32;
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
                on_error(&self.input.lost, "microphone"),
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
        Ok(dev_name)
    }

    /// The microphone currently open.
    pub fn active_input(&self) -> Option<String> {
        read(&self.input.active)
    }

    /// The speaker currently open.
    pub fn active_output(&self) -> Option<String> {
        read(&self.output.active)
    }

    pub fn follows_input(&self) -> bool {
        self.input.following.load(Ordering::Relaxed)
    }

    pub fn follows_output(&self) -> bool {
        self.output.following.load(Ordering::Relaxed)
    }

    /// Remembered devices that were not plugged in at start-up.
    pub fn missing(&self) -> Vec<String> {
        match self.missing.lock() {
            Ok(lock) => lock.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Reopens any stream the driver took away, and moves a stream that follows the
    /// system default to wherever the default has gone.
    ///
    /// macOS hands a stream back when the device's sample rate changes underneath it —
    /// which is exactly what a Bluetooth headset does when its microphone opens and it
    /// switches profile. The stream is dead at that point and the resampler behind it
    /// is built for a rate that no longer exists, so the only cure is to open it again
    /// and read the new configuration.
    ///
    /// A new default is the other case. Plugging in a headset leaves the built-in
    /// speaker working, so nothing is lost and the driver says nothing; cpal has no
    /// device-change notification either. So the default's name is read here and
    /// compared with the device in use (#160).
    ///
    /// Only macOS polls defaults. On Linux, ALSA exposes a `default` PCM rather
    /// than the underlying device; PipeWire/PulseAudio handle routing when used.
    /// Windows retains its existing stream recovery behavior.
    ///
    /// Called off the audio thread, once a second. Failed recovery is retried.
    pub fn recover(&self) -> Vec<Recovered> {
        let mut news = Vec::new();
        for side in [Side::Microphone, Side::Speaker] {
            let route = match side {
                Side::Microphone => &self.input,
                Side::Speaker => &self.output,
            };
            if let Some(event) = route.recover(
                side,
                cfg!(target_os = "macos"),
                || default_device_name(side),
                |name| match side {
                    Side::Microphone => self.open_input(name),
                    Side::Speaker => self.open_output(name),
                },
            ) {
                news.push(event);
            }
        }
        news
    }

    /// Opens a speaker stream without saying anything about whether it was chosen.
    fn open_output(&self, wanted: Option<&str>) -> Result<String> {
        let host = cpal::default_host();
        let device = match wanted {
            Some(name) => pick(host.output_devices()?, name)
                .with_context(|| format!("no speaker named '{name}'"))?,
            None => host
                .default_output_device()
                .context("no default speaker found")?,
        };

        let out_cfg = device
            .default_output_config()
            .context("could not read speaker config")?;
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
                        let pull_count = (needed_samples - queued_out.len()).max(32)
                            * (SAMPLE_RATE as usize)
                            / (out_rate as usize + 1);
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
                on_error(&self.output.lost, "speaker"),
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
                        let pull_count = (needed_samples - queued_out.len()).max(32)
                            * (SAMPLE_RATE as usize)
                            / (out_rate as usize + 1);
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
                on_error(&self.output.lost, "speaker"),
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
                        let pull_count = (needed_samples - queued_out.len()).max(32)
                            * (SAMPLE_RATE as usize)
                            / (out_rate as usize + 1);
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
                on_error(&self.output.lost, "speaker"),
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
        Ok(dev_name)
    }
}

/// The user's routing preference and the stream's actual destination are separate:
/// a remembered device falling back to the default is still an explicit preference.
/// Stream construction is supplied by the caller so the same lifecycle can be
/// exercised without opening hardware in tests.
struct DeviceRoute {
    active: Arc<std::sync::Mutex<Option<String>>>,
    lost: Arc<AtomicBool>,
    following: AtomicBool,
}

impl DeviceRoute {
    fn new(wanted: &Wanted) -> Self {
        Self {
            active: Arc::new(std::sync::Mutex::new(None)),
            lost: Arc::new(AtomicBool::new(false)),
            following: AtomicBool::new(matches!(wanted, Wanted::Default)),
        }
    }

    fn initialize(
        &self,
        wanted: &Wanted,
        open: impl Fn(Option<&str>) -> Result<String>,
    ) -> Result<Option<String>> {
        let (device, missing) = open_wanted(wanted, open)?;
        remember(&self.active, &device);
        Ok(missing)
    }

    fn switch(
        &self,
        wanted: Option<&str>,
        open: impl Fn(Option<&str>) -> Result<String>,
    ) -> Result<String> {
        let device = open(wanted)?;
        remember(&self.active, &device);
        self.following.store(wanted.is_none(), Ordering::Relaxed);
        Ok(device)
    }

    fn recover(
        &self,
        side: Side,
        poll_default: bool,
        default: impl FnOnce() -> Option<String>,
        open: impl Fn(Option<&str>) -> Result<String>,
    ) -> Option<Recovered> {
        let lost = self.lost.swap(false, Ordering::Relaxed);
        let following = self.following.load(Ordering::Relaxed);
        let current = read(&self.active);
        if !lost {
            // A pinned route and platforms without default polling never query it.
            if !poll_default || !following {
                return None;
            }
            let default = default();
            if !moves_to_default(following, current.as_deref(), default.as_deref()) {
                return None;
            }
        }

        // Recovery never changes the user's preference. A lost pinned stream tries
        // its current device first, with the existing default fallback on failure.
        let wanted = if following { None } else { current.as_deref() };
        let opened = open(wanted).or_else(|err| {
            if lost && wanted.is_some() {
                open(None)
            } else {
                Err(err)
            }
        });
        match opened {
            Ok(device) => {
                remember(&self.active, &device);
                Some(Recovered {
                    side,
                    device: Some(device),
                })
            }
            Err(err) => {
                tracing::warn!("could not reopen the {}: {err:#}", side.name());
                if lost {
                    self.lost.store(true, Ordering::Relaxed);
                    Some(Recovered { side, device: None })
                } else {
                    // The old stream is still usable; compare again next tick.
                    None
                }
            }
        }
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

/// The name the system gives its default microphone or speaker right now.
fn default_device_name(side: Side) -> Option<String> {
    let host = cpal::default_host();
    let device = match side {
        Side::Microphone => host.default_input_device(),
        Side::Speaker => host.default_output_device(),
    }?;
    device.description().ok().map(|d| d.name().to_string())
}

/// Whether a stream should move to the system default: it follows the default, the
/// default can be named, and it is not the device already in use.
fn moves_to_default(following: bool, current: Option<&str>, default: Option<&str>) -> bool {
    following && default.is_some() && current != default
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

/// The devices worth offering someone choosing a microphone or a speaker.
///
/// On Linux, cpal hands back every PCM ALSA knows of. Most are plugins, not devices:
/// `null` ("Discard all samples"), three sample-rate converters, upmix and downmix,
/// the OSS and JACK bridges. Each card also appears several times over, as `hw`,
/// `plughw`, `front`, one `surround*` per speaker layout, `dmix` and `dsnoop`.
/// Listing them buried the two or three real choices, and opening each to ask for its
/// format made alsa-lib complain on stderr about every one that would not open.
///
/// So they are left out before anything is opened. What stays is `default`, the sound
/// servers, each card once, and anything ALSA's own configuration does not ship, such
/// as a PCM someone defined in their `.asoundrc`. A device can still be named with
/// `--input` or `--output`, listed or not.
fn worth_showing(devices: impl Iterator<Item = cpal::Device>) -> Vec<cpal::Device> {
    let devices: Vec<_> = devices.collect();
    let names: Vec<(String, Option<String>)> = devices
        .iter()
        .map(|device| match device.description() {
            Ok(desc) => (desc.name().to_string(), desc.driver().map(str::to_string)),
            Err(_) => (String::new(), None),
        })
        .collect();
    let keep = keep(&names);
    devices
        .into_iter()
        .zip(keep)
        .filter_map(|(device, keep)| keep.then_some(device))
        .collect()
}

/// Which of `(name, ALSA PCM)` to list. Only ALSA names a PCM, so off Linux nothing
/// is dropped but a repeated name.
fn keep(devices: &[(String, Option<String>)]) -> Vec<bool> {
    let mut seen = std::collections::HashSet::new();
    devices
        .iter()
        .map(|(name, pcm)| {
            let wanted =
                !cfg!(target_os = "linux") || pcm.as_deref().is_none_or(alsa_worth_showing);
            // A card reached as `sysdefault` and again as `plughw` has one name, and
            // picking by name would only ever find the first.
            wanted && seen.insert(name.clone())
        })
        .collect()
}

/// Whether an ALSA PCM (`plughw:CARD=PCH,DEV=0`, `pulse`, …) is a device to offer.
fn alsa_worth_showing(pcm: &str) -> bool {
    let kind = pcm.split(':').next().unwrap_or(pcm);
    let plugin = matches!(
        kind,
        "null"
            | "lavrate"
            | "samplerate"
            | "speexrate"
            | "speex"
            | "upmix"
            | "vdownmix"
            | "oss"
            | "jack"
            | "a52"
            | "usbstream"
            | "equal"
    );
    // Each card's other faces: the raw device without conversions, one PCM per speaker
    // layout and digital output, and the halves of the mixing default already wraps.
    let face = kind == "hw"
        || kind == "front"
        || kind.starts_with("surround")
        || matches!(
            kind,
            "iec958" | "spdif" | "dmix" | "dsnoop" | "rear" | "center_lfe" | "side"
        );
    !plugin && !face
}

/// Lists all available input devices.
pub fn list_input_devices() -> Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));

    let mut list = Vec::new();
    if let Ok(devices) = host.input_devices() {
        for dev in worth_showing(devices) {
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
        for dev in worth_showing(devices) {
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
        input: DeviceRoute::new(&choice.input),
        output: DeviceRoute::new(&choice.output),
        missing: Arc::new(std::sync::Mutex::new(Vec::new())),
    };

    let lost_input = devices
        .input
        .initialize(&choice.input, |name| devices.open_input(name))
        .context("could not initialize microphone")?;
    let lost_output = devices
        .output
        .initialize(&choice.output, |name| devices.open_output(name))
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

/// Lists the system's audio devices (`tincan devices`). `all` includes what the
/// interface leaves out: ALSA's plugins and each card's raw and per-channel-layout PCMs.
pub fn describe_devices(all: bool) -> Result<String> {
    let host = cpal::default_host();
    let mut report = String::new();

    let default_in = host
        .default_input_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));
    let default_out = host
        .default_output_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));

    let shown = |devices: Vec<cpal::Device>| {
        if all {
            devices
        } else {
            worth_showing(devices.into_iter())
        }
    };
    report.push_str("\n  MICROPHONES\n");
    for device in shown(host.input_devices()?.collect()) {
        report.push_str(&line(&device, &default_in, true));
    }
    report.push_str("\n  SPEAKERS\n");
    for device in shown(host.output_devices()?.collect()) {
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
    let mark = if Some(&name) == default.as_ref() {
        "default"
    } else {
        ""
    };
    // One column for the name, one for what it runs at, one for whether it is the
    // one you get by default.
    format!("    {name:<38}  {rate:<16}  {mark}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PipeWire laptop as ALSA describes it: hints first, then the cards cpal adds.
    #[cfg(target_os = "linux")]
    fn pipewire_laptop() -> Vec<(String, Option<String>)> {
        [
            (
                "Discard all samples (playback) or generate zero samples (capture)",
                "null",
            ),
            (
                "Rate Converter Plugin Using Libav/FFmpeg Library",
                "lavrate",
            ),
            (
                "Rate Converter Plugin Using Samplerate Library",
                "samplerate",
            ),
            ("Rate Converter Plugin Using Speex Resampler", "speexrate"),
            ("JACK Audio Connection Kit", "jack"),
            ("Open Sound System", "oss"),
            ("PipeWire Sound Server", "pipewire"),
            ("PulseAudio Sound Server", "pulse"),
            (
                "Plugin using Speex DSP (resample, agc, denoise, echo, dereverb)",
                "speex",
            ),
            ("Plugin for channel upmix (4,6,8)", "upmix"),
            (
                "Plugin for channel downmix (stereo) with a simple spacialization",
                "vdownmix",
            ),
            (
                "Default ALSA Output (currently PipeWire Media Server)",
                "default",
            ),
            ("HDA Intel PCH, ALC257 Analog", "sysdefault:CARD=PCH"),
            ("HDA Intel PCH, ALC257 Analog", "front:CARD=PCH,DEV=0"),
            ("HDA Intel PCH, ALC257 Analog", "surround51:CARD=PCH,DEV=0"),
            ("HDA Intel PCH, HDMI 0", "hdmi:CARD=PCH,DEV=0"),
            ("HDA Intel PCH, ALC257 Analog", "dmix:CARD=PCH,DEV=0"),
            ("HDA Intel PCH, ALC257 Analog", "hw:CARD=PCH,DEV=0"),
            ("HDA Intel PCH, ALC257 Analog", "plughw:CARD=PCH,DEV=0"),
            ("USB Audio, USB Audio", "hw:CARD=Headset,DEV=0"),
            ("USB Audio, USB Audio", "plughw:CARD=Headset,DEV=0"),
            ("My loopback", "myloop"),
        ]
        .into_iter()
        .map(|(name, pcm)| (name.to_string(), Some(pcm.to_string())))
        .collect()
    }

    /// Models hardware availability and stream configuration, rather than a list
    /// of expected function calls. Failed opens leave the working stream intact.
    #[derive(Default)]
    struct TestHost {
        devices: std::cell::RefCell<std::collections::BTreeMap<String, u32>>,
        default: std::cell::RefCell<Option<String>>,
        stream: std::cell::RefCell<Option<(String, u32)>>,
    }

    impl TestHost {
        fn built_in() -> Self {
            let host = Self::default();
            host.connect("Built-in", 48_000);
            host.connect("Headset", 16_000);
            host.set_default(Some("Built-in"));
            host
        }

        fn connect(&self, name: &str, rate: u32) {
            self.devices.borrow_mut().insert(name.into(), rate);
        }

        fn disconnect(&self, name: &str) {
            self.devices.borrow_mut().remove(name);
        }

        fn set_default(&self, name: Option<&str>) {
            *self.default.borrow_mut() = name.map(str::to_string);
        }

        fn default_name(&self) -> Option<String> {
            self.default.borrow().clone()
        }

        fn open(&self, wanted: Option<&str>) -> Result<String> {
            let default = self.default_name();
            let name = wanted.or(default.as_deref()).context("no default")?;
            let devices = self.devices.borrow();
            let (name, rate) = devices
                .iter()
                .find(|(device, _)| device.to_lowercase().contains(&name.to_lowercase()))
                .context("device unavailable")?;
            if *rate == 0 {
                bail!("device will not open");
            }
            *self.stream.borrow_mut() = Some((name.clone(), *rate));
            Ok(name.clone())
        }

        fn start(&self, wanted: &Wanted) -> DeviceRoute {
            let route = DeviceRoute::new(wanted);
            route.initialize(wanted, |name| self.open(name)).unwrap();
            route
        }

        fn recover(&self, route: &DeviceRoute, side: Side) -> Option<Recovered> {
            route.recover(side, true, || self.default_name(), |name| self.open(name))
        }

        fn assert_on(&self, route: &DeviceRoute, name: &str, rate: u32, following: bool) {
            assert_eq!(read(&route.active).as_deref(), Some(name));
            assert_eq!(*self.stream.borrow(), Some((name.into(), rate)));
            assert_eq!(route.following.load(Ordering::Relaxed), following);
        }
    }

    #[test]
    fn a_call_follows_defaults_until_a_device_is_chosen_and_can_follow_again() {
        let mic = TestHost::built_in();
        let speaker = TestHost::built_in();
        let input = mic.start(&Wanted::Default);
        let output = speaker.start(&Wanted::Default);

        mic.set_default(Some("Headset"));
        speaker.set_default(Some("Headset"));
        for (host, route, side) in [
            (&mic, &input, Side::Microphone),
            (&speaker, &output, Side::Speaker),
        ] {
            assert_eq!(
                host.recover(route, side),
                Some(Recovered {
                    side,
                    device: Some("Headset".into()),
                })
            );
            host.assert_on(route, "Headset", 16_000, true);
            assert!(
                host.recover(route, side).is_none(),
                "stable devices emit no change"
            );
        }

        input
            .switch(Some("Built-in"), |name| mic.open(name))
            .unwrap();
        mic.set_default(Some("Built-in"));
        speaker.set_default(Some("Built-in"));
        assert!(mic.recover(&input, Side::Microphone).is_none());
        speaker.recover(&output, Side::Speaker).unwrap();
        mic.assert_on(&input, "Built-in", 48_000, false);
        speaker.assert_on(&output, "Built-in", 48_000, true);

        mic.set_default(Some("Headset"));
        assert!(mic.recover(&input, Side::Microphone).is_none());
        mic.assert_on(&input, "Built-in", 48_000, false);
        input.switch(None, |name| mic.open(name)).unwrap();
        mic.assert_on(&input, "Headset", 16_000, true);
        mic.disconnect("Headset");
        mic.set_default(Some("Built-in"));
        input.lost.store(true, Ordering::Relaxed);
        mic.recover(&input, Side::Microphone).unwrap();
        mic.assert_on(&input, "Built-in", 48_000, true);
    }

    #[test]
    fn remembered_fallback_and_named_recovery_do_not_enable_following() {
        for wanted in [
            Wanted::Named("Head".into()),
            Wanted::Remembered("Head".into()),
        ] {
            let host = TestHost::built_in();
            if matches!(wanted, Wanted::Remembered(_)) {
                host.disconnect("Headset");
            }
            let route = host.start(&wanted);
            host.connect("Headset", 16_000);
            host.set_default(Some("Headset"));
            assert!(host.recover(&route, Side::Microphone).is_none());
            assert!(!route.following.load(Ordering::Relaxed));

            // A pinned device that disappears may fall back, but remains pinned
            // to the replacement instead of following later default changes.
            host.disconnect("Headset");
            host.set_default(Some("Built-in"));
            route.lost.store(true, Ordering::Relaxed);
            host.recover(&route, Side::Microphone).unwrap();
            host.assert_on(&route, "Built-in", 48_000, false);
            host.connect("Headset", 16_000);
            host.set_default(Some("Headset"));
            assert!(host.recover(&route, Side::Microphone).is_none());
            host.assert_on(&route, "Built-in", 48_000, false);
        }
    }

    #[test]
    fn failed_changes_preserve_working_audio_and_recovery_eventually_converges() {
        let host = TestHost::built_in();
        let route = host.start(&Wanted::Default);
        host.connect("Headset", 0);
        assert!(
            route
                .switch(Some("Headset"), |name| host.open(name))
                .is_err()
        );
        host.assert_on(&route, "Built-in", 48_000, true);
        host.set_default(Some("Headset"));
        for _ in 0..3 {
            assert!(host.recover(&route, Side::Microphone).is_none());
            host.assert_on(&route, "Built-in", 48_000, true);
        }
        host.set_default(None);
        assert!(host.recover(&route, Side::Microphone).is_none());
        host.assert_on(&route, "Built-in", 48_000, true);
        host.set_default(Some("Headset"));
        host.connect("Headset", 16_000);
        host.recover(&route, Side::Microphone).unwrap();
        host.assert_on(&route, "Headset", 16_000, true);

        route
            .switch(Some("Headset"), |name| host.open(name))
            .unwrap();
        host.set_default(None);
        assert!(route.switch(None, |name| host.open(name)).is_err());
        host.assert_on(&route, "Headset", 16_000, false);
    }

    #[test]
    fn profile_changes_and_total_device_loss_recover_without_losing_the_preference() {
        for wanted in [Wanted::Default, Wanted::Named("Built-in".into())] {
            let host = TestHost::built_in();
            let route = host.start(&wanted);
            let following = matches!(wanted, Wanted::Default);
            host.connect("Built-in", 16_000);
            route.lost.store(true, Ordering::Relaxed);
            host.recover(&route, Side::Microphone).unwrap();
            host.assert_on(&route, "Built-in", 16_000, following);

            host.disconnect("Built-in");
            host.set_default(None);
            route.lost.store(true, Ordering::Relaxed);
            for _ in 0..3 {
                assert_eq!(
                    host.recover(&route, Side::Microphone),
                    Some(Recovered {
                        side: Side::Microphone,
                        device: None,
                    })
                );
                assert!(
                    route.lost.load(Ordering::Relaxed),
                    "failure remains retryable"
                );
                assert_eq!(route.following.load(Ordering::Relaxed), following);
            }
            host.connect("Built-in", 48_000);
            host.set_default(Some("Built-in"));
            host.recover(&route, Side::Microphone).unwrap();
            host.assert_on(&route, "Built-in", 48_000, following);
            assert!(!route.lost.load(Ordering::Relaxed));
            assert!(host.recover(&route, Side::Microphone).is_none());
        }
    }

    #[test]
    fn platforms_without_polling_and_pinned_devices_never_query_defaults() {
        let host = TestHost::built_in();
        let route = host.start(&Wanted::Default);
        host.set_default(Some("Headset"));
        assert!(
            route
                .recover(
                    Side::Speaker,
                    false,
                    || panic!("must not poll"),
                    |name| host.open(name)
                )
                .is_none()
        );
        host.assert_on(&route, "Built-in", 48_000, true);
        route
            .switch(Some("Built-in"), |name| host.open(name))
            .unwrap();
        assert!(
            route
                .recover(
                    Side::Speaker,
                    true,
                    || panic!("pinned device must not poll"),
                    |name| host.open(name)
                )
                .is_none()
        );
        host.assert_on(&route, "Built-in", 48_000, false);
        route.lost.store(true, Ordering::Relaxed);
        route
            .recover(
                Side::Speaker,
                false,
                || panic!("recovery does not poll"),
                |name| host.open(name),
            )
            .unwrap();
        host.assert_on(&route, "Built-in", 48_000, false);
    }

    #[test]
    fn routing_converges_across_event_sequences_without_overriding_explicit_choices() {
        // Exercise all 6^5 combinations, including repeated choices, failures,
        // disappearance and driver errors. Assert outcomes after every event;
        // no particular sequence of internal calls is prescribed.
        for mut sequence in 0..6usize.pow(5) {
            let host = TestHost::built_in();
            let route = host.start(&Wanted::Default);
            let mut following = true;
            let mut actual = "Built-in".to_string();
            let mut available = true;
            let mut default = "Built-in";
            let mut retry = false;
            for _ in 0..5 {
                let action = sequence % 6;
                sequence /= 6;
                match action {
                    0 => {
                        default = "Built-in";
                        host.set_default(Some(default));
                    }
                    1 => {
                        default = "Headset";
                        host.set_default(Some(default));
                    }
                    2 => {
                        available = !available;
                        if available {
                            host.connect("Headset", 16_000);
                        } else {
                            host.disconnect("Headset");
                        }
                    }
                    3 => {
                        let result = route.switch(Some("Headset"), |name| host.open(name));
                        assert_eq!(result.is_ok(), available);
                        if available {
                            actual = "Headset".into();
                            following = false;
                        }
                    }
                    4 => {
                        let result = route.switch(None, |name| host.open(name));
                        let usable = default == "Built-in" || available;
                        assert_eq!(result.is_ok(), usable);
                        if usable {
                            actual = default.into();
                            following = true;
                        }
                    }
                    5 => {
                        retry = true;
                        route.lost.store(true, Ordering::Relaxed);
                    }
                    _ => unreachable!(),
                }
                let lost = retry;
                let wanted = if following { default } else { actual.as_str() };
                let usable = wanted == "Built-in" || available;
                if (lost || following) && usable {
                    actual = wanted.into();
                } else if lost && !following && (default == "Built-in" || available) {
                    actual = default.into();
                }
                if lost {
                    retry = !(usable || (!following && (default == "Built-in" || available)));
                }
                host.recover(&route, Side::Microphone);
                assert_eq!(route.lost.load(Ordering::Relaxed), retry);
                let rate = if actual == "Headset" { 16_000 } else { 48_000 };
                host.assert_on(&route, &actual, rate, following);
            }
        }
    }

    /// Runs the real CPAL streams, resamplers and ring buffers. BlackHole lets us
    /// verify a known signal without playing it through the user's speakers.
    /// No system defaults or persistent preferences are changed.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires BlackHole 2ch and macOS microphone access; run deliberately"]
    fn live_macos_streams_recover_and_keep_transferring_samples() -> Result<()> {
        use std::time::{Duration, Instant};
        let virtual_device = "BlackHole 2ch";
        let default_input =
            default_device_name(Side::Microphone).context("no default microphone")?;
        let default_output = default_device_name(Side::Speaker).context("no default speaker")?;
        assert_ne!(
            default_input, virtual_device,
            "use a physical system default for this test"
        );
        assert_ne!(
            default_output, virtual_device,
            "use a physical system default for this test"
        );
        let (devices, mut capture, mut playback, _) = open(&DeviceChoice {
            input: Wanted::Named(virtual_device.into()),
            output: Wanted::Named(virtual_device.into()),
        })?;

        assert!(
            devices
                .switch_input(Some("tincan-test-device-that-does-not-exist"))
                .is_err()
        );
        assert!(
            devices
                .switch_output(Some("tincan-test-device-that-does-not-exist"))
                .is_err()
        );
        assert_eq!(devices.active_input().as_deref(), Some(virtual_device));
        assert_eq!(devices.active_output().as_deref(), Some(virtual_device));
        assert!(!devices.follows_input() && !devices.follows_output());

        // Send a known tone through the OS's loopback device, and measure what
        // actually returns through tincan's capture callback, not a mocked one.
        let started = Instant::now();
        let mut phase = 0usize;
        let mut energy = 0.0f64;
        let mut captured = 0usize;
        let mut sine = 0.0f64;
        let mut cosine = 0.0f64;
        while started.elapsed() < Duration::from_millis(600) {
            for _ in 0..playback.slots().min(FRAME) {
                let sample = 0.05
                    * (std::f32::consts::TAU * 440.0 * phase as f32 / SAMPLE_RATE as f32).sin();
                playback.push(sample).unwrap();
                phase += 1;
            }
            while let Ok(sample) = capture.pop() {
                assert!(sample.is_finite() && sample.abs() <= 1.0);
                let angle = std::f64::consts::TAU * 440.0 * captured as f64 / SAMPLE_RATE as f64;
                sine += f64::from(sample) * angle.sin();
                cosine += f64::from(sample) * angle.cos();
                energy += f64::from(sample).powi(2);
                captured += 1;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            captured >= FRAME * 5,
            "real capture callbacks must transfer audio"
        );
        assert!(
            energy / captured as f64 > 1e-8,
            "the loopback must contain the transmitted signal (samples={captured}, mean square={})",
            energy / captured as f64
        );

        // Device volume can attenuate the signal. Check its identity as well as
        // its presence rather than requiring a particular hardware gain.
        let tone_share = 2.0 * (sine * sine + cosine * cosine) / captured as f64 / energy;
        assert!(
            tone_share > 0.2,
            "the captured signal must contain the sent 440 Hz tone (share={tone_share})"
        );

        {
            // Clear the test tone before the real speakers are opened.
            let mut queued = devices.playback_rx.lock().unwrap();
            while queued.pop().is_ok() {}
        }

        // Represent streams opened on a previous default, then observe the actual
        // current system defaults through the public recovery path. This avoids
        // changing the user's global audio settings to manufacture a default change.
        devices.input.following.store(true, Ordering::Relaxed);
        devices.output.following.store(true, Ordering::Relaxed);
        let news = devices.recover();
        assert!(news.contains(&Recovered {
            side: Side::Microphone,
            device: Some(default_input.clone())
        }));
        assert!(news.contains(&Recovered {
            side: Side::Speaker,
            device: Some(default_output.clone())
        }));
        assert_eq!(
            devices.active_input().as_deref(),
            Some(default_input.as_str())
        );
        assert_eq!(
            devices.active_output().as_deref(),
            Some(default_output.as_str())
        );
        assert!(devices.follows_input() && devices.follows_output());
        assert!(devices.recover().is_empty());

        // A driver loss must rebuild both streams without replacing the buffers
        // held by the audio engine or changing the user's following preference.
        devices.input.lost.store(true, Ordering::Relaxed);
        devices.output.lost.store(true, Ordering::Relaxed);
        let recovered = devices.recover();
        assert_eq!(recovered.len(), 2);
        assert!(recovered.iter().all(|event| event.device.is_some()));
        while capture.pop().is_ok() {}
        let deadline = Instant::now() + Duration::from_secs(2);
        while capture.slots() < FRAME && Instant::now() < deadline {
            for _ in 0..playback.slots().min(FRAME) {
                playback.push(0.0).unwrap();
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            capture.slots() >= FRAME,
            "the original capture consumer must still receive samples after recovery"
        );
        for _ in 0..FRAME {
            assert!(capture.pop().unwrap().is_finite());
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_list_is_the_sound_servers_and_each_card_once() {
        let devices = pipewire_laptop();
        let shown: Vec<&str> = devices
            .iter()
            .zip(keep(&devices))
            .filter(|(_, keep)| *keep)
            .map(|((_, pcm), _)| pcm.as_deref().unwrap())
            .collect();
        assert_eq!(
            shown,
            [
                "pipewire",
                "pulse",
                "default",
                "sysdefault:CARD=PCH",
                "hdmi:CARD=PCH,DEV=0",
                "plughw:CARD=Headset,DEV=0",
                "myloop",
            ]
        );
    }

    /// The speakers of the desktop in #148, with ALSA's hints in the order it gives them
    /// (it hides `hw`, `plughw` and `dmix` from hints by default) and then the numbered
    /// `hw` and `plughw` pairs cpal adds for every card.
    #[cfg(target_os = "linux")]
    fn desktop_with_hdmi_and_usb() -> Vec<(String, Option<String>)> {
        let mut devices = vec![
            (
                "Discard all samples (playback) or generate zero samples (capture)",
                "null".to_string(),
            ),
            (
                "Rate Converter Plugin Using Libav/FFmpeg Library",
                "lavrate".into(),
            ),
            (
                "Rate Converter Plugin Using Samplerate Library",
                "samplerate".into(),
            ),
            (
                "Rate Converter Plugin Using Speex Resampler",
                "speexrate".into(),
            ),
            ("JACK Audio Connection Kit", "jack".into()),
            ("Open Sound System", "oss".into()),
            ("PipeWire Sound Server", "pipewire".into()),
            ("PulseAudio Sound Server", "pulse".into()),
            (
                "Plugin using Speex DSP (resample, agc, denoise, echo, dereverb)",
                "speex".into(),
            ),
            ("Plugin for channel upmix (4,6,8)", "upmix".into()),
            (
                "Plugin for channel downmix (stereo) with a simple spacialization",
                "vdownmix".into(),
            ),
            (
                "Default ALSA Output (currently PulseAudio Sound Server)",
                "default".into(),
            ),
        ];
        let analog = "HDA Intel PCH, ALC887-VD Analog";
        devices.push((analog, "sysdefault:CARD=PCH".into()));
        devices.push((analog, "front:CARD=PCH,DEV=0".into()));
        for layout in ["21", "40", "41", "50", "51", "71"] {
            devices.push((analog, format!("surround{layout}:CARD=PCH,DEV=0")));
        }
        devices.push((
            "HDA Intel PCH, ALC887-VD Digital",
            "iec958:CARD=PCH,DEV=0".into(),
        ));
        let hdmi = [
            "HDA Intel PCH, HDMI 0",
            "HDA Intel PCH, HDMI 1",
            "HDA Intel PCH, HDMI 2",
        ];
        for (n, name) in hdmi.iter().enumerate() {
            devices.push((name, format!("hdmi:CARD=PCH,DEV={n}")));
        }
        devices.push(("HDA Intel PCH", "usbstream:CARD=PCH".into()));
        let nvidia = [
            "HDA NVidia, 27G4",
            "HDA NVidia, HDMI 1",
            "HDA NVidia, HDMI 2",
            "HDA NVidia, HDMI 3",
        ];
        for (n, name) in nvidia.iter().enumerate() {
            devices.push((name, format!("hdmi:CARD=NVidia,DEV={n}")));
        }
        devices.push(("HDA NVidia", "usbstream:CARD=NVidia".into()));
        devices.push(("HP Webcam HD 4310", "usbstream:CARD=U0x4f20x2e2".into()));
        let usb = "X-Rest 7.1, USB Audio";
        devices.push((usb, "sysdefault:CARD=X71".into()));
        devices.push((usb, "front:CARD=X71,DEV=0".into()));
        for layout in ["21", "40", "41", "50", "51", "71"] {
            devices.push((usb, format!("surround{layout}:CARD=X71,DEV=0")));
        }
        devices.push((usb, "iec958:CARD=X71,DEV=0".into()));
        devices.push(("X-Rest 7.1", "usbstream:CARD=X71".into()));
        let numbered = [
            (0, 0, analog),
            (0, 1, "HDA Intel PCH, ALC887-VD Digital"),
            (0, 3, hdmi[0]),
            (0, 7, hdmi[1]),
            (0, 8, hdmi[2]),
            (1, 3, nvidia[0]),
            (1, 7, nvidia[1]),
            (1, 8, nvidia[2]),
            (1, 9, nvidia[3]),
            (3, 0, usb),
        ];
        for (card, dev, name) in numbered {
            devices.push((name, format!("hw:CARD={card},DEV={dev}")));
            devices.push((name, format!("plughw:CARD={card},DEV={dev}")));
        }
        devices
            .into_iter()
            .map(|(name, pcm)| (name.to_string(), Some(pcm)))
            .collect()
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_desktop_lists_each_output_once() {
        let devices = desktop_with_hdmi_and_usb();
        let shown: Vec<&str> = devices
            .iter()
            .zip(keep(&devices))
            .filter(|(_, keep)| *keep)
            .map(|((name, _), _)| name.as_str())
            .collect();
        assert_eq!(
            shown,
            [
                "PipeWire Sound Server",
                "PulseAudio Sound Server",
                "Default ALSA Output (currently PulseAudio Sound Server)",
                "HDA Intel PCH, ALC887-VD Analog",
                "HDA Intel PCH, HDMI 0",
                "HDA Intel PCH, HDMI 1",
                "HDA Intel PCH, HDMI 2",
                "HDA NVidia, 27G4",
                "HDA NVidia, HDMI 1",
                "HDA NVidia, HDMI 2",
                "HDA NVidia, HDMI 3",
                "X-Rest 7.1, USB Audio",
                "HDA Intel PCH, ALC887-VD Digital",
            ]
        );
    }

    #[test]
    fn without_alsa_only_a_repeated_name_is_dropped() {
        let devices = vec![
            ("MacBook Pro Microphone".to_string(), None),
            ("AirPods".to_string(), None),
            ("AirPods".to_string(), None),
        ];
        assert_eq!(keep(&devices), [true, true, false]);
    }

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
        assert_eq!(
            Wanted::pick(None, Some("MacBook".into())),
            Wanted::Remembered("MacBook".into())
        );
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
        assert_eq!(
            missing.as_deref(),
            Some("QCY H4"),
            "and the room has to be told why"
        );
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
        assert_eq!(
            missing, None,
            "nothing went wrong, so there is nothing to report"
        );
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
