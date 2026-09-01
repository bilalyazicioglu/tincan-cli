//! High-quality, low-latency audio resampler.
//!
//! Uses Cubic Hermite interpolation with phase accumulation to convert
//! between arbitrary device sample rates (e.g. 16 kHz Bluetooth HFP, 44.1 kHz USB)
//! and tincan's internal 48 kHz Opus standard rate.

/// Real-time streaming resampler for mono audio streams.
#[derive(Debug, Clone)]
pub struct Resampler {
    from_rate: u32,
    to_rate: u32,
    /// Ratio = from_rate / to_rate
    ratio: f64,
    /// Fractional phase in input space
    phase: f64,
    /// Ring buffer of recent input samples for 4-point Hermite interpolation:
    /// [y_{-1}, y_0, y_1, y_2]
    history: [f32; 4],
    history_initialized: bool,
}

impl Resampler {
    /// Creates a new resampler from `from_rate` to `to_rate`.
    pub fn new(from_rate: u32, to_rate: u32) -> Self {
        let from = from_rate.max(1);
        let to = to_rate.max(1);
        Self {
            from_rate: from,
            to_rate: to,
            ratio: from as f64 / to as f64,
            phase: 0.0,
            history: [0.0; 4],
            history_initialized: false,
        }
    }

    /// Whether this resampler is a direct pass-through (rates are equal).
    pub fn is_identity(&self) -> bool {
        self.from_rate == self.to_rate
    }

    /// Resamples an incoming slice of samples into the provided `output` vector.
    pub fn process(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if self.is_identity() {
            output.extend_from_slice(input);
            return;
        }

        if input.is_empty() {
            return;
        }

        let mut in_idx = 0;
        let in_len = input.len();

        while in_idx < in_len {
            let sample = input[in_idx];
            in_idx += 1;

            if !self.history_initialized {
                self.history = [sample; 4];
                self.history_initialized = true;
            } else {
                self.history[0] = self.history[1];
                self.history[1] = self.history[2];
                self.history[2] = self.history[3];
                self.history[3] = sample;
            }

            // Generate output samples while phase is within the current interval [0, 1)
            while self.phase < 1.0 {
                let t = self.phase as f32;
                let y0 = self.history[1];
                let y1 = self.history[2];
                let ym1 = self.history[0];
                let y2 = self.history[3];

                // 4-point, 3rd-order Hermite interpolation polynomial
                let c0 = y0;
                let c1 = 0.5 * (y1 - ym1);
                let c2 = ym1 - 2.5 * y0 + 2.0 * y1 - 0.5 * y2;
                let c3 = 0.5 * (y2 - ym1) + 1.5 * (y0 - y1);
                let interpolated = ((c3 * t + c2) * t + c1) * t + c0;

                output.push(interpolated);
                self.phase += self.ratio;
            }

            self.phase -= 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_resampling_preserves_samples() {
        let mut r = Resampler::new(48000, 48000);
        assert!(r.is_identity());

        let input = vec![0.1, 0.2, -0.3, 0.4];
        let mut output = Vec::new();
        r.process(&input, &mut output);
        assert_eq!(input, output);
    }

    #[test]
    fn upsampling_16k_to_48k_produces_3x_samples() {
        let mut r = Resampler::new(16000, 48000);
        assert!(!r.is_identity());

        // 160 samples at 16 kHz = 10 ms -> should produce exactly 480 samples at 48 kHz
        let input: Vec<f32> = (0..160)
            .map(|i| (i as f32 * 0.1).sin())
            .collect();

        let mut output = Vec::new();
        r.process(&input, &mut output);

        let diff = (output.len() as isize - 480).abs();
        assert!(diff <= 1, "expected 480 samples, got {}", output.len());
    }

    #[test]
    fn downsampling_48k_to_16k_produces_one_third_samples() {
        let mut r = Resampler::new(48000, 16000);
        let input: Vec<f32> = (0..480)
            .map(|i| (i as f32 * 0.05).sin())
            .collect();

        let mut output = Vec::new();
        r.process(&input, &mut output);

        let diff = (output.len() as isize - 160).abs();
        assert!(diff <= 1, "expected 160 samples, got {}", output.len());
    }

    #[test]
    fn sine_wave_energy_is_preserved_across_resampling() {
        let mut up = Resampler::new(16000, 48000);
        let mut down = Resampler::new(48000, 16000);

        // Generate a 440 Hz test tone at 16 kHz for 100 ms (1600 samples)
        let f = 440.0;
        let original: Vec<f32> = (0..1600)
            .map(|i| (2.0 * std::f32::consts::PI * f * (i as f32 / 16000.0)).sin())
            .collect();

        let mut upsampled = Vec::new();
        up.process(&original, &mut upsampled);

        let mut roundtrip = Vec::new();
        down.process(&upsampled, &mut roundtrip);

        // Check that signal energy (RMS) is preserved within 5%
        let rms_orig = (original.iter().map(|s| s * s).sum::<f32>() / original.len() as f32).sqrt();
        let rms_resamp = (roundtrip.iter().map(|s| s * s).sum::<f32>() / roundtrip.len() as f32).sqrt();

        assert!((rms_orig - rms_resamp).abs() < 0.05, "RMS original {rms_orig}, RMS resampled {rms_resamp}");
    }
}
