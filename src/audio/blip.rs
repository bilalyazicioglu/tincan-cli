//! Synthesizes pleasant UI feedback tones (blips) for channel actions.

use std::f32::consts::PI;

/// Generates a two-tone UI blip chime (48 kHz mono PCM `f32` samples).
///
/// Produces a warm, bell-like rising chime (C5 -> E5) with zero DC-offset click,
/// smooth envelope attack/decay, and crisp audible volume.
pub fn generate_blip() -> Vec<f32> {
    let sample_rate = 48_000.0;

    // Note 1: 523.25 Hz (C5) for 40 ms
    // Note 2: 659.25 Hz (E5) for 60 ms
    let dur1 = 0.040;
    let dur2 = 0.060;

    let len1 = (sample_rate * dur1) as usize;
    let len2 = (sample_rate * dur2) as usize;
    let mut samples = Vec::with_capacity(len1 + len2);

    let freq1 = 523.25;
    let freq2 = 659.25;

    // Tone 1: C5
    for i in 0..len1 {
        let t = i as f32 / sample_rate;
        let progress = i as f32 / len1 as f32;
        let env = (progress * PI).sin() * (1.0 - progress).powf(0.3);
        let fundamental = (2.0 * PI * freq1 * t).sin();
        let harmonic = 0.15 * (4.0 * PI * freq1 * t).sin();
        samples.push(0.40 * env * (fundamental + harmonic));
    }

    // Tone 2: E5
    for i in 0..len2 {
        let t = i as f32 / sample_rate;
        let progress = i as f32 / len2 as f32;
        let env = (progress * PI).sin() * (1.0 - progress).powf(0.3);
        let fundamental = (2.0 * PI * freq2 * t).sin();
        let harmonic = 0.15 * (4.0 * PI * freq2 * t).sin();
        samples.push(0.45 * env * (fundamental + harmonic));
    }

    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_non_empty_bounded_blip_samples() {
        let samples = generate_blip();
        assert!(!samples.is_empty(), "blip samples must not be empty");

        for &sample in &samples {
            assert!(
                (-0.6..=0.6).contains(&sample),
                "blip amplitude must be within pleasant bounds [-0.6, 0.6], got {sample}"
            );
        }
    }
}
