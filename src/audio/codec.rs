//! Opus kodlayıcı/çözücü sarmalayıcısı.

use anyhow::{Context, Result};
use audiopus::coder::{Decoder as OpusDecoder, Encoder as OpusEncoder};
use audiopus::{Application, Bitrate, Channels, SampleRate};

use super::{BITRATE, FRAME};
use crate::audio::jitter::Frame;

/// Opus paketinin sığacağı en büyük tampon.
const MAX_PACKET: usize = 1500;

pub struct Encoder {
    inner: OpusEncoder,
    scratch: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Result<Self> {
        let mut inner = OpusEncoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)
            .context("Opus kodlayıcı açılamadı")?;
        inner
            .set_bitrate(Bitrate::BitsPerSecond(BITRATE))
            .context("bit hızı ayarlanamadı")?;
        Ok(Self {
            inner,
            scratch: vec![0u8; MAX_PACKET],
        })
    }

    /// Bir çerçeveyi kodlar ve paket baytlarını döndürür.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<&[u8]> {
        let written = self
            .inner
            .encode_float(pcm, &mut self.scratch)
            .context("ses kodlanamadı")?;
        Ok(&self.scratch[..written])
    }
}

pub struct Decoder {
    inner: OpusDecoder,
}

impl Decoder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: OpusDecoder::new(SampleRate::Hz48000, Channels::Mono)
                .context("Opus çözücü açılamadı")?,
        })
    }

    /// Jitter tamponundan gelen çerçeveyi PCM'e çevirir.
    ///
    /// Kayıp çerçevelerde kodeğin kendi örtme (packet loss concealment) mekanizması
    /// çalıştırılır: eksik yere sessizlik koymak duyulur bir "tık" yaratırdı.
    pub fn decode(&mut self, frame: &Frame, out: &mut [f32]) -> Result<usize> {
        match frame {
            Frame::Packet(payload) => self
                .inner
                .decode_float(Some(payload), out, false)
                .context("ses çözülemedi"),
            Frame::Lost => self
                .inner
                .decode_float(None::<&[u8]>, out, false)
                .context("kayıp çerçeve örtülemedi"),
            Frame::Silence => {
                out[..FRAME].fill(0.0);
                Ok(FRAME)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32) -> Vec<f32> {
        (0..FRAME)
            .map(|i| amplitude * (i as f32 * 0.05).sin())
            .collect()
    }

    /// Kodlanan ses çözüldüğünde tanınabilir biçimde geri gelmeli.
    /// (Opus kayıplıdır; örnek örnek eşitlik değil, enerji benzerliği aranır.)
    #[test]
    fn speech_survives_a_round_trip() {
        let mut encoder = Encoder::new().unwrap();
        let mut decoder = Decoder::new().unwrap();
        let input = tone(0.5);
        let mut output = vec![0.0; FRAME];

        // Opus'un ilk çerçevelerinde kodek "ısınır"; birkaç çerçeve besleyip ölçüyoruz.
        for _ in 0..5 {
            let packet = encoder.encode(&input).unwrap().to_vec();
            decoder
                .decode(&Frame::Packet(packet), &mut output)
                .unwrap();
        }

        let input_rms = super::super::vad::rms(&input);
        let output_rms = super::super::vad::rms(&output);
        assert!(
            (output_rms - input_rms).abs() < input_rms * 0.5,
            "enerji korunmalı: giriş {input_rms:.3}, çıkış {output_rms:.3}"
        );
    }

    #[test]
    fn encoded_frames_are_small_enough_for_a_datagram() {
        let mut encoder = Encoder::new().unwrap();
        let input = tone(0.5);
        for _ in 0..10 {
            let packet = encoder.encode(&input).unwrap();
            assert!(
                packet.len() < 200,
                "20ms çerçeve datagram'a rahat sığmalı: {} bayt",
                packet.len()
            );
        }
    }

    /// Kayıp çerçeve sessizlik değil, örtme üretmeli.
    #[test]
    fn loss_is_concealed_rather_than_silenced() {
        let mut encoder = Encoder::new().unwrap();
        let mut decoder = Decoder::new().unwrap();
        let input = tone(0.5);
        let mut output = vec![0.0; FRAME];

        for _ in 0..5 {
            let packet = encoder.encode(&input).unwrap().to_vec();
            decoder.decode(&Frame::Packet(packet), &mut output).unwrap();
        }

        decoder.decode(&Frame::Lost, &mut output).unwrap();
        let concealed = super::super::vad::rms(&output);
        assert!(
            concealed > 0.01,
            "örtme sessizlik üretmemeli, rms: {concealed}"
        );
    }

    #[test]
    fn silence_frames_produce_actual_silence() {
        let mut decoder = Decoder::new().unwrap();
        let mut output = vec![0.7; FRAME];
        let written = decoder.decode(&Frame::Silence, &mut output).unwrap();
        assert_eq!(written, FRAME);
        assert!(output.iter().all(|s| *s == 0.0), "önceki içerik kalmamalı");
    }
}
