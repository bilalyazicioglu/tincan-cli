//! Combining several peers' audio into a single output.
//!
//! In the mesh everyone sends their voice as a separate stream, and since only one
//! signal reaches the speaker they have to be summed. Naive summing overflows the
//! amplitude when two people talk at once, and hard clipping (distortion) is audible.
//!
//! The fix is a limiter that gently pushes the sum down when it rises above the
//! ceiling. So the gain does not jump around — which is itself audible, as "pumping" —
//! it approaches its target gradually.

/// The limiter engages once the sum exceeds this value.
const CEILING: f32 = 0.95;
/// How fast the gain recovers, per frame.
const RECOVERY: f32 = 0.05;

pub struct Mixer {
    /// The current attenuation factor; 1.0 means no attenuation.
    gain: f32,
}

impl Default for Mixer {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

impl Mixer {
    /// Sums the sources into `out`. `out` need not be zeroed beforehand — the
    /// function clears it itself.
    pub fn mix(&mut self, sources: &[&[f32]], out: &mut [f32]) {
        out.fill(0.0);
        for source in sources {
            for (slot, sample) in out.iter_mut().zip(source.iter()) {
                *slot += sample;
            }
        }

        let peak = out.iter().fold(0f32, |max, s| max.max(s.abs()));

        // If the peak is over the ceiling, drop the gain at once (preventing
        // distortion is urgent); otherwise climb back slowly, because a sudden rise
        // sounds like pumping.
        let needed = if peak > CEILING { CEILING / peak } else { 1.0 };
        if needed < self.gain {
            self.gain = needed;
        } else {
            self.gain = (self.gain + RECOVERY).min(1.0);
        }

        if self.gain < 1.0 {
            for sample in out.iter_mut() {
                *sample *= self.gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(value: f32, len: usize) -> Vec<f32> {
        vec![value; len]
    }

    #[test]
    fn mixing_nothing_produces_silence() {
        let mut mixer = Mixer::default();
        let mut out = vec![7.0; 8];
        mixer.mix(&[], &mut out);
        assert!(out.iter().all(|s| *s == 0.0), "previous content must be cleared");
    }

    #[test]
    fn single_quiet_source_passes_through_untouched() {
        let mut mixer = Mixer::default();
        let source = constant(0.3, 4);
        let mut out = vec![0.0; 4];
        mixer.mix(&[&source], &mut out);
        assert!(out.iter().all(|s| (*s - 0.3).abs() < 1e-6), "{out:?}");
    }

    #[test]
    fn quiet_sources_sum() {
        let mut mixer = Mixer::default();
        let a = constant(0.2, 4);
        let b = constant(0.3, 4);
        let mut out = vec![0.0; 4];
        mixer.mix(&[&a, &b], &mut out);
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6), "{out:?}");
    }

    /// The whole point: audio must not distort in a crowded channel.
    #[test]
    fn loud_sources_are_limited_not_clipped() {
        let mut mixer = Mixer::default();
        let sources: Vec<Vec<f32>> = (0..5).map(|_| constant(0.8, 16)).collect();
        let refs: Vec<&[f32]> = sources.iter().map(|s| s.as_slice()).collect();
        let mut out = vec![0.0; 16];

        mixer.mix(&refs, &mut out);

        let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 1.0, "the output must not overflow, peak: {peak}");
        assert!(peak > 0.5, "audio must stay audible, peak: {peak}");
        // They all share a sign, so the waveform must be preserved (no flat clipping).
        assert!(out.iter().all(|s| (*s - out[0]).abs() < 1e-6), "the signal must not distort");
    }

    /// The gain must return once the loud passage is over, or everything stays quiet.
    #[test]
    fn gain_recovers_after_the_loud_passage() {
        let mut mixer = Mixer::default();
        let loud: Vec<Vec<f32>> = (0..5).map(|_| constant(0.9, 8)).collect();
        let refs: Vec<&[f32]> = loud.iter().map(|s| s.as_slice()).collect();
        let mut out = vec![0.0; 8];
        mixer.mix(&refs, &mut out);
        assert!(mixer.gain < 0.5, "loud audio must be attenuated");

        let quiet = constant(0.1, 8);
        for _ in 0..100 {
            mixer.mix(&[&quiet], &mut out);
        }
        assert!((mixer.gain - 1.0).abs() < 1e-6, "the gain must come back");
        assert!((out[0] - 0.1).abs() < 1e-6, "a quiet signal must not be turned down");
    }

    /// Sources may differ in length (a lost frame, a short concealment) — no panics.
    #[test]
    fn tolerates_sources_shorter_than_the_output() {
        let mut mixer = Mixer::default();
        let short = constant(0.5, 2);
        let full = constant(0.1, 8);
        let mut out = vec![0.0; 8];

        mixer.mix(&[&short, &full], &mut out);

        assert!((out[0] - 0.6).abs() < 1e-6, "the short source must be audible at the start");
        assert!((out[7] - 0.1).abs() < 1e-6, "past its end nothing may be disturbed");
    }
}
