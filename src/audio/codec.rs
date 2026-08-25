//! Wrapper around the Opus encoder and decoder.

use anyhow::{Context, Result};
use audiopus::coder::{Decoder as OpusDecoder, Encoder as OpusEncoder};
use audiopus::{Application, Bitrate, Channels, SampleRate};

use super::{BITRATE, FRAME};
use crate::audio::jitter::Frame;

/// The largest buffer an Opus packet has to fit into.
const MAX_PACKET: usize = 1500;

pub struct Encoder {
    inner: OpusEncoder,
    scratch: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Result<Self> {
        let mut inner = OpusEncoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)
            .context("could not open the Opus encoder")?;
        inner
            .set_bitrate(Bitrate::BitsPerSecond(BITRATE))
            .context("could not set the bitrate")?;
        Ok(Self {
            inner,
            scratch: vec![0u8; MAX_PACKET],
        })
    }

    /// Encodes one frame and returns the packet bytes.
    pub fn encode(&mut self, pcm: &[f32]) -> Result<&[u8]> {
        let written = self
            .inner
            .encode_float(pcm, &mut self.scratch)
            .context("could not encode audio")?;
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
                .context("could not open the Opus decoder")?,
        })
    }

    /// Turns a frame from the jitter buffer into PCM.
    ///
    /// For lost frames the codec's own packet loss concealment is run: dropping
    /// silence into the gap would produce an audible click.
    pub fn decode(&mut self, frame: &Frame, out: &mut [f32]) -> Result<usize> {
        match frame {
            Frame::Packet(payload) => self
                .inner
                .decode_float(Some(payload), out, false)
                .context("could not decode audio"),
            Frame::Lost => self
                .inner
                .decode_float(None::<&[u8]>, out, false)
                .context("could not conceal a lost frame"),
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

    /// Encoded audio must come back recognizable after decoding.
    /// (Opus is lossy, so this compares energy rather than sample-for-sample equality.)
    #[test]
    fn speech_survives_a_round_trip() {
        let mut encoder = Encoder::new().unwrap();
        let mut decoder = Decoder::new().unwrap();
        let input = tone(0.5);
        let mut output = vec![0.0; FRAME];

        // Opus warms up over its first few frames, so feed a few before measuring.
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
            "energy must be preserved: in {input_rms:.3}, out {output_rms:.3}"
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
                "a 20 ms frame must fit comfortably in a datagram: {} bytes",
                packet.len()
            );
        }
    }

    /// A lost frame must produce concealment, not silence.
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
            "concealment must not produce silence, rms: {concealed}"
        );
    }

    #[test]
    fn silence_frames_produce_actual_silence() {
        let mut decoder = Decoder::new().unwrap();
        let mut output = vec![0.7; FRAME];
        let written = decoder.decode(&Frame::Silence, &mut output).unwrap();
        assert_eq!(written, FRAME);
        assert!(output.iter().all(|s| *s == 0.0), "no previous content may remain");
    }
}
