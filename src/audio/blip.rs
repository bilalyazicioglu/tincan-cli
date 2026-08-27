//! Synthesizes pleasant UI feedback tones (blips) for channel actions.

use std::f32::consts::PI;

/// Generates a two-tone UI blip chime (48 kHz mono PCM `f32` samples).
///
/// Produces a warm, bell-like rising chime (C5 -> E5) with zero DC-offset click,
/// smooth envelope attack/decay, and soft harmonic saturation.
pub fn generate_blip() -> Vec<f32> {
    let sample_rate = 48_000.0;

    // Note 1: 523.25 Hz (C5) for 35 ms
    // Note 2: 659.25 Hz (E5) for 55 ms
    let dur1 = 0.035;
    let dur2 = 0.055;

    let len1 = (sample_rate * dur1) as usize;
    let len2 = (sample_rate * dur2) as usize;
    let mut samples = Vec::with_capacity(len1 + len2);

    let freq1 = 523.25;
    let freq2 = 659.25;

    // Tone 1: C5
    for i in 0..len1 {
        let t = i as f32 / sample_rate;
        let progress = i as f32 / len1 as f32;
        // Smooth sine attack/decay envelope
        let env = (progress * PI).sin() * (1.0 - progress).powf(0.5);
        let fundamental = (2.0 * PI * freq1 * t).sin();
        let harmonic = 0.08 * (4.0 * PI * freq1 * t).sin(); // 2nd harmonic warmth
        samples.push(0.12 * env * (fundamental + harmonic));
    }

    // Tone 2: E5
    for i in 0..len2 {
        let t = i as f32 / sample_rate;
        let progress = i as f32 / len2 as f32;
        let env = (progress * PI).sin() * (1.0 - progress).powf(0.5);
        let fundamental = (2.0 * PI * freq2 * t).sin();
        let harmonic = 0.08 * (4.0 * PI * freq2 * t).sin();
        samples.push(0.15 * env * (fundamental + harmonic));
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
        assert_eq!(samples.len(), (48000.0 * 0.090) as usize);

        for &sample in &samples {
            assert!(
                sample >= -0.3 && sample <= 0.3,
                "blip amplitude must be within soft pleasant bounds [-0.3, 0.3], got {sample}"
            );
        }
    }
}
