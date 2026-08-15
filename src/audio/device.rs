//! Ses donanımı ile async dünya arasındaki köprü.
//!
//! cpal'ın callback'leri gerçek zamanlı bir bağlamda çalışır: içinde bellek ayırmak,
//! kilit almak ya da `.await` etmek ses kesintisine (çıtırtı) yol açar. Bu yüzden
//! callback'ler yalnızca kilitsiz ring buffer'a dokunur; kodlama, ağ ve miksaj
//! normal task'lerde yapılır.
//!
//! ```text
//! [mikrofon callback] → ring → [kodlayıcı task] → ağ
//! ağ → [çözücü + mikser task] → ring → [hoparlör callback]
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtrb::{Consumer, Producer, RingBuffer};

use super::{FRAME, SAMPLE_RATE};

/// Ring buffer'ların tuttuğu ses miktarı (~200ms).
const RING_CAPACITY: usize = FRAME * 10;

/// Gerçek zamanlı callback'lerin dışarı bildirdiği sayaçlar.
#[derive(Default)]
pub struct AudioHealth {
    /// Hoparlör veri bulamadı → duyulabilir kesinti.
    pub underruns: AtomicU64,
    /// Mikrofon verisi tamponu aştı → yakalanan ses düştü.
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

/// `open()` sonucu: açık akışlar, mikrofon ucu, hoparlör ucu ve sağlık sayaçları.
pub type OpenAudio = (AudioDevices, Consumer<f32>, Producer<f32>, Arc<AudioHealth>);

/// Açık ses akışları. Düşürüldüğünde donanım serbest bırakılır.
pub struct AudioDevices {
    _input: cpal::Stream,
    _output: cpal::Stream,
}

/// Hangi cihazın kullanılacağı. `None` = sistemin varsayılanı.
#[derive(Debug, Clone, Default)]
pub struct DeviceChoice {
    pub input: Option<String>,
    pub output: Option<String>,
}

/// İsme göre cihaz seçer. Eşleşme büyük/küçük harf duyarsız ve kısmi:
/// kullanıcı `tincan devices` çıktısındaki adın ayırt edici bir parçasını yazabilsin.
fn pick(mut devices: impl Iterator<Item = cpal::Device>, wanted: &str) -> Option<cpal::Device> {
    let wanted = wanted.to_lowercase();
    devices.find(|d| {
        d.description()
            .map(|desc| desc.name().to_lowercase().contains(&wanted))
            .unwrap_or(false)
    })
}

/// Mikrofon ve hoparlörü açar; yakalama ve çalma uçlarını döndürür.
pub fn open(choice: &DeviceChoice) -> Result<OpenAudio> {
    let host = cpal::default_host();

    let input = match &choice.input {
        Some(name) => pick(host.input_devices()?, name)
            .with_context(|| format!("'{name}' adında bir mikrofon yok (tincan devices ile listeleyin)"))?,
        None => host.default_input_device().context("mikrofon bulunamadı")?,
    };
    let output = match &choice.output {
        Some(name) => pick(host.output_devices()?, name)
            .with_context(|| format!("'{name}' adında bir hoparlör yok (tincan devices ile listeleyin)"))?,
        None => host.default_output_device().context("hoparlör bulunamadı")?,
    };

    let in_cfg = input.default_input_config().context("mikrofon ayarı okunamadı")?;
    let out_cfg = output.default_output_config().context("hoparlör ayarı okunamadı")?;

    // MVP'de yeniden örnekleme yok. Sessizce bozuk ses üretmektense açıkça söylüyoruz.
    if in_cfg.sample_rate() != SAMPLE_RATE || out_cfg.sample_rate() != SAMPLE_RATE {
        bail!(
            "ses cihazları 48kHz olmalı (mikrofon {} Hz, hoparlör {} Hz). \
             Ses ayarlarından 48000 Hz seçebilirsiniz.",
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
            |err| tracing::warn!("mikrofon hatası: {err}"),
            None,
        )
        .context("mikrofon akışı açılamadı")?;

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
                    // Mono kaynağı tüm kanallara dağıt.
                    chunk.fill(sample);
                }
            },
            |err| tracing::warn!("hoparlör hatası: {err}"),
            None,
        )
        .context("hoparlör akışı açılamadı")?;

    input_stream.play().context("mikrofon başlatılamadı")?;
    output_stream.play().context("hoparlör başlatılamadı")?;

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

/// Sistemdeki ses cihazlarını listeler (`tincan devices`).
pub fn describe_devices() -> Result<String> {
    let host = cpal::default_host();
    let mut report = String::new();

    let default_in = host
        .default_input_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));
    let default_out = host
        .default_output_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_string()));

    report.push_str("\n  Mikrofonlar:\n");
    for device in host.input_devices()? {
        report.push_str(&line(&device, &default_in, true));
    }
    report.push_str("\n  Hoparlörler:\n");
    for device in host.output_devices()? {
        report.push_str(&line(&device, &default_out, false));
    }
    report.push_str("\n  Not: tincan şu an yalnızca 48000 Hz cihazlarla çalışır.\n");
    Ok(report)
}

fn line(device: &cpal::Device, default: &Option<String>, input: bool) -> String {
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "(isimsiz)".into());
    let config = if input {
        device.default_input_config().ok()
    } else {
        device.default_output_config().ok()
    };
    let rate = config
        .map(|c| format!("{} Hz, {} kanal", c.sample_rate(), c.channels()))
        .unwrap_or_else(|| "ayar okunamadı".into());
    let mark = if Some(&name) == default.as_ref() { " ←" } else { "" };
    format!("    • {name}  [{rate}]{mark}\n")
}
