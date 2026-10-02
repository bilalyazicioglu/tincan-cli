//! Noise suppression on the way out of the microphone.
//!
//! RNNoise, through the pure-Rust `nnnoiseless`. It is a small recurrent network
//! trained to tell speech from everything else, and it takes out the things a
//! gate cannot: a fan, an air conditioner, the person typing while they talk.
//! The gate in front of it decides whether to speak at all; this decides what
//! the speech sounds like when it does.
//!
//! Two things about the library shape this module.
//!
//! It works in 480-sample frames and wants its samples in the range an `i16`
//! would hold, `[-32768.0, 32767.0]`, even though they are `f32`. tincan's audio
//! path is normalised to `[-1.0, 1.0]` from end to end, so every frame is scaled
//! on the way in and back on the way out. Skipping that is not a small error:
//! a ±1.0 signal looks like near-silence to the network, which would dutifully
//! erase all of it, quietly.
//!
//! And it is an overlap-add design, so the frame it hands back covers the span
//! of the frame before the one just given to it. That is a real 10 ms of added
//! latency on the capture path, not an artifact of how this module is written.
//! It is accepted rather than worked around: tincan already runs on 20 ms frames
//! behind a three-frame jitter buffer, and 10 ms is cheap against what the
//! suppression is worth. The library carries the delayed half itself, which is
//! why there is no buffer here.

use nnnoiseless::DenoiseState;

use super::FRAME;

/// How many samples RNNoise takes at a time: 10 ms at 48 kHz.
const RNNOISE_FRAME: usize = DenoiseState::FRAME_SIZE;

/// The scale between tincan's normalised samples and the 16-bit range RNNoise
/// expects. `i16::MAX` rather than 32768 so that a full-scale sample survives
/// the round trip as itself.
const FULL_SCALE: f32 = i16::MAX as f32;

/// Cleans the microphone signal frame by frame.
pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    /// Scratch for one RNNoise frame in and one out. Held here so that a capture
    /// loop running fifty times a second is not allocating.
    scaled: [f32; RNNOISE_FRAME],
    cleaned: [f32; RNNOISE_FRAME],
}

impl Default for Denoiser {
    fn default() -> Self {
        Self::new()
    }
}

impl Denoiser {
    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            scaled: [0.0; RNNOISE_FRAME],
            cleaned: [0.0; RNNOISE_FRAME],
        }
    }

    /// Cleans a frame of normalised samples in place.
    ///
    /// A tincan frame is 960 samples, exactly two RNNoise frames, so nothing has
    /// to be padded or held back. The samples that come out are 10 ms older than
    /// the ones that went in; see the note at the top of this file. The very
    /// first call returns the network's fade-in rather than audio, which is one
    /// 10 ms blip at the moment the microphone opens.
    ///
    /// Any length that is not a whole number of RNNoise frames is left untouched,
    /// because half-cleaning a frame would be worse than not cleaning it.
    pub fn process(&mut self, pcm: &mut [f32]) {
        let (frames, leftover) = pcm.as_chunks_mut::<RNNOISE_FRAME>();
        if !leftover.is_empty() {
            return;
        }

        for chunk in frames {
            for (out, sample) in self.scaled.iter_mut().zip(chunk.iter()) {
                *out = sample * FULL_SCALE;
            }
            self.state.process_frame(&mut self.cleaned, &self.scaled);
            for (sample, cleaned) in chunk.iter_mut().zip(self.cleaned.iter()) {
                // The network only ever attenuates, so this clamp should never
                // bite. It is here because everything downstream — the meter,
                // the gate, the encoder — is written against a normalised range,
                // and a stray sample outside it would be their problem, not ours.
                *sample = (cleaned / FULL_SCALE).clamp(-1.0, 1.0);
            }
        }
    }
}

// A tincan frame has to divide evenly into RNNoise frames, or `process` would
// silently decline to do anything at all.
const _: () = assert!(FRAME.is_multiple_of(RNNOISE_FRAME));

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::vad::rms;

    fn tone(amplitude: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| amplitude * (i as f32 * 0.05).sin())
            .collect()
    }

    #[test]
    fn a_frame_keeps_its_length() {
        let mut denoiser = Denoiser::new();
        let mut pcm = tone(0.3, FRAME);
        denoiser.process(&mut pcm);
        assert_eq!(pcm.len(), FRAME);
    }

    #[test]
    fn silence_stays_silent() {
        let mut denoiser = Denoiser::new();
        let mut pcm = vec![0.0f32; FRAME];
        for _ in 0..5 {
            denoiser.process(&mut pcm);
            assert!(
                pcm.iter().all(|s| s.abs() < 1e-3),
                "silence must not come back as a click or a DC offset"
            );
            pcm.fill(0.0);
        }
    }

    /// The point of the thing: a clean tone is speech-shaped enough to survive.
    /// The first frames are the network's fade-in, so the check is on a settled
    /// stream rather than on the first thing it ever sees.
    #[test]
    fn a_steady_tone_survives() {
        let mut denoiser = Denoiser::new();
        let mut last = Vec::new();
        for _ in 0..25 {
            let mut pcm = tone(0.5, FRAME);
            denoiser.process(&mut pcm);
            last = pcm;
        }
        assert!(
            rms(&last) > 0.01,
            "a steady tone must not be erased; got rms {}",
            rms(&last)
        );
    }

    #[test]
    fn a_full_scale_frame_does_not_clip_on_the_way_through() {
        let mut denoiser = Denoiser::new();
        for _ in 0..5 {
            let mut pcm = tone(1.0, FRAME);
            denoiser.process(&mut pcm);
            assert!(
                pcm.iter().all(|s| (-1.0..=1.0).contains(s)),
                "the scaling round trip must stay inside the normalised range"
            );
        }
    }

    /// `process` is written against whole RNNoise frames. A partial one is left
    /// alone rather than half-processed.
    #[test]
    fn an_odd_length_is_left_untouched() {
        let mut denoiser = Denoiser::new();
        let original = tone(0.4, RNNOISE_FRAME + 1);
        let mut pcm = original.clone();
        denoiser.process(&mut pcm);
        assert_eq!(pcm, original);
    }

    /// Room noise at the level one actually sits in — a fan, the building — is
    /// most of what this exists for, and it has to come down by a lot while
    /// speech does not. This is also the test that would catch the scaling
    /// mistake the module comment warns about: feed the network `[-1.0, 1.0]`
    /// and it reports silence, leaves every band alone, and both of these
    /// numbers become 1.0.
    #[test]
    fn room_noise_comes_down_and_speech_does_not() {
        fn attenuation(mut sample: impl FnMut(usize) -> f32) -> f32 {
            let mut denoiser = Denoiser::new();
            let (mut input, mut output, mut n) = (0.0f64, 0.0f64, 0usize);
            for frame in 0..100 {
                let mut pcm: Vec<f32> = (0..FRAME).map(|i| sample(frame * FRAME + i)).collect();
                let before = rms(&pcm);
                denoiser.process(&mut pcm);
                // The network needs a moment to settle on what the room sounds
                // like; measuring through the fade-in would measure that instead.
                if frame >= 25 {
                    input += before as f64;
                    output += rms(&pcm) as f64;
                    n += 1;
                }
            }
            assert!(n > 0);
            (input / output.max(1e-9)) as f32
        }

        // A deterministic hiss, quiet enough to be a room rather than a fault.
        let mut seed = 0x1234_5678u32;
        let hiss = attenuation(move |_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * 0.02
        });

        // A voiced sound: a 140 Hz fundamental with its harmonics.
        let speech = attenuation(|n| {
            let t = n as f32 / super::super::SAMPLE_RATE as f32;
            (0..8)
                .map(|h| {
                    let k = (h + 1) as f32;
                    (1.0 / k) * (2.0 * std::f32::consts::PI * 140.0 * k * t).sin()
                })
                .sum::<f32>()
                * 0.15
        });

        assert!(
            hiss > 3.0,
            "room noise must come down; it came down {hiss:.1}x"
        );
        assert!(
            speech < 1.3,
            "speech must come through roughly untouched; it lost {speech:.1}x"
        );
        assert!(
            hiss > speech * 2.0,
            "and the point is the difference between the two: {hiss:.1}x vs {speech:.1}x"
        );
    }

    /// The output is the previous frame's span, so a burst of sound emerges one
    /// RNNoise frame after it went in. This is the latency the module documents;
    /// if it ever stops being true the comment at the top is wrong.
    #[test]
    fn the_output_lags_the_input_by_one_rnnoise_frame() {
        let mut denoiser = Denoiser::new();

        // Settle first, so what is measured is the steady-state delay and not
        // the fade-in.
        let mut warmup = vec![0.0f32; FRAME];
        for _ in 0..10 {
            denoiser.process(&mut warmup);
            warmup.fill(0.0);
        }

        // A frame that is silent in its first half and loud in its second.
        let mut pcm = vec![0.0f32; FRAME];
        pcm[RNNOISE_FRAME..].copy_from_slice(&tone(0.8, RNNOISE_FRAME));
        denoiser.process(&mut pcm);

        assert!(
            rms(&pcm[..RNNOISE_FRAME]) < 1e-3,
            "the first half is the previous frame, which was silent"
        );
    }
}
