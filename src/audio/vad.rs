//! Voice activity detection (VAD).
//!
//! It has two jobs: feeding the "who is talking" indicator in the people list, and
//! stopping transmission during silence (DTX) — in a six-person room usually one person
//! is speaking, and there is no point carrying the other five's silence over the
//! network.
//!
//! A plain RMS threshold is not enough on its own: the indicator flickers during the
//! short pauses between words, and the tail of a word gets clipped. So the gate stays
//! open for a short while after speech stops (hangover).

/// The frame's average energy (root mean square).
pub fn rms(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum: f32 = pcm.iter().map(|s| s * s).sum();
    (sum / pcm.len() as f32).sqrt()
}

pub struct Vad {
    threshold: f32,
    /// How many further frames to stay open after falling back to silence.
    hangover: u32,
    remaining: u32,
}

impl Default for Vad {
    fn default() -> Self {
        // ~0.01 RMS sits just above a whisper in a quiet room; at 20 ms per frame,
        // 15 frames ≈ 300 ms of hangover, a comfortable margin between words.
        Self::new(0.01, 15)
    }
}

impl Vad {
    pub fn new(threshold: f32, hangover: u32) -> Self {
        Self {
            threshold,
            hangover,
            remaining: 0,
        }
    }

    /// Evaluates a frame; `true` means the frame should be sent and the user shown
    /// as speaking.
    pub fn update(&mut self, pcm: &[f32]) -> bool {
        if rms(pcm) >= self.threshold {
            self.remaining = self.hangover;
            true
        } else {
            self.remaining = self.remaining.saturating_sub(1);
            self.remaining > 0
        }
    }

    pub fn is_active(&self) -> bool {
        self.remaining > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32) -> Vec<f32> {
        (0..960)
            .map(|i| amplitude * (i as f32 * 0.1).sin())
            .collect()
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 100]), 0.0);
    }

    #[test]
    fn rms_grows_with_amplitude() {
        assert!(rms(&tone(0.5)) > rms(&tone(0.1)));
    }

    #[test]
    fn detects_speech_and_ignores_room_noise() {
        let mut vad = Vad::default();
        assert!(vad.update(&tone(0.5)), "clear speech must be detected");

        let mut quiet = Vad::default();
        assert!(!quiet.update(&[0.0001; 960]), "room noise must not count as speech");
    }

    /// A short silence between words must not switch the indicator off.
    #[test]
    fn short_pauses_do_not_cut_speech_off() {
        let mut vad = Vad::new(0.01, 15);
        assert!(vad.update(&tone(0.5)));

        for frame in 0..14 {
            assert!(
                vad.update(&[0.0; 960]),
                "must not cut off at silent frame {frame}"
            );
        }
    }

    /// But it must close when someone really has stopped — otherwise DTX never engages.
    #[test]
    fn sustained_silence_eventually_stops_transmission() {
        let mut vad = Vad::new(0.01, 15);
        vad.update(&tone(0.5));
        for _ in 0..15 {
            vad.update(&[0.0; 960]);
        }
        assert!(!vad.is_active(), "must close after a long silence");
        assert!(!vad.update(&[0.0; 960]));
    }

    #[test]
    fn speech_resets_the_hangover() {
        let mut vad = Vad::new(0.01, 5);
        vad.update(&tone(0.5));
        vad.update(&[0.0; 960]);
        vad.update(&[0.0; 960]);
        // When speech resumes, the counter must refill from the top.
        vad.update(&tone(0.5));
        for _ in 0..4 {
            assert!(vad.update(&[0.0; 960]));
        }
    }
}
