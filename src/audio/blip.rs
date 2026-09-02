//! The interface's own voice.
//!
//! Five sounds cut from one shape: two tones, the second longer and a touch louder
//! than the first, under one envelope and one timbre. They are told apart by the two
//! things an ear separates without being taught — which way the interval moves, and
//! how high it sits.
//!
//! Closing your microphone and closing your ears are the same gesture an octave
//! apart, so learning one teaches the other; up always means open and down always
//! means closed. The arrival chime sits above both and is the only one that is not a
//! fifth, which is what keeps a room event from sounding like something you did.

use std::f32::consts::PI;

use super::SAMPLE_RATE;

/// What the interface has to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blip {
    /// Someone arrived, or you moved between screens.
    Chime,
    /// Your microphone just closed.
    MicOff,
    /// Your microphone just opened.
    MicOn,
    /// You just stopped hearing the room.
    EarsOff,
    /// You can hear the room again.
    EarsOn,
}

const C5: f32 = 523.25;
const E5: f32 = 659.25;
/// The microphone pair: a perfect fifth in the middle of the register.
const G4: f32 = 392.00;
const D5: f32 = 587.33;
/// The ears pair: the same fifth, one octave down.
const G3: f32 = 196.00;
const D4: f32 = 293.66;

/// Two tones and how to voice them.
struct Gesture {
    first: f32,
    second: f32,
    /// Seconds.
    short: f32,
    long: f32,
    /// How much second harmonic rides on the fundamental.
    harmonic: f32,
    amplitude: f32,
}

/// The second tone lands a little louder than the first. That is what makes a pair of
/// tones read as one gesture rather than two beeps.
const LIFT: f32 = 0.05;

impl Blip {
    fn gesture(self) -> Gesture {
        match self {
            Blip::Chime => Gesture {
                first: C5,
                second: E5,
                short: 0.040,
                long: 0.060,
                harmonic: 0.15,
                amplitude: 0.40,
            },
            Blip::MicOn => Gesture {
                first: G4,
                second: D5,
                short: 0.035,
                long: 0.055,
                harmonic: 0.12,
                amplitude: 0.35,
            },
            Blip::MicOff => Gesture {
                first: D5,
                second: G4,
                ..Blip::MicOn.gesture()
            },
            // Lower and longer, because losing the room is the heavier state. The
            // extra harmonic is not brightness for its own sake: a 196 Hz fundamental
            // is more than a small laptop speaker can move, and the overtone is what
            // carries it there.
            Blip::EarsOn => Gesture {
                first: G3,
                second: D4,
                short: 0.045,
                long: 0.075,
                harmonic: 0.25,
                amplitude: 0.35,
            },
            Blip::EarsOff => Gesture {
                first: D4,
                second: G3,
                ..Blip::EarsOn.gesture()
            },
        }
    }
}

/// Renders one of the interface's sounds as 48 kHz mono PCM.
pub fn of(blip: Blip) -> Vec<f32> {
    let gesture = blip.gesture();
    let mut samples = Vec::new();
    tone(&mut samples, gesture.first, gesture.short, &gesture, 0.0);
    tone(&mut samples, gesture.second, gesture.long, &gesture, LIFT);
    samples
}

/// One tone, opening and closing at silence so it cannot click.
fn tone(out: &mut Vec<f32>, freq: f32, seconds: f32, gesture: &Gesture, lift: f32) {
    let rate = SAMPLE_RATE as f32;
    let len = (rate * seconds) as usize;
    let amplitude = gesture.amplitude + lift;

    for i in 0..len {
        let t = i as f32 / rate;
        let progress = i as f32 / len as f32;
        let envelope = (progress * PI).sin() * (1.0 - progress).powf(0.3);
        let fundamental = (2.0 * PI * freq * t).sin();
        let overtone = gesture.harmonic * (4.0 * PI * freq * t).sin();
        out.push(amplitude * envelope * (fundamental + overtone));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY: [Blip; 5] = [Blip::Chime, Blip::MicOn, Blip::MicOff, Blip::EarsOn, Blip::EarsOff];

    /// Estimates the pitch of a stretch of samples by counting how often it crosses
    /// zero. The tones are a sine plus a weak second harmonic, which adds no crossings
    /// of its own, so this is exact enough to tell one note from another.
    fn pitch(samples: &[f32]) -> f32 {
        let crossings = samples
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        let seconds = samples.len() as f32 / SAMPLE_RATE as f32;
        crossings as f32 / 2.0 / seconds
    }

    /// The pitch of each half of a gesture.
    fn halves(blip: Blip) -> (f32, f32) {
        let gesture = blip.gesture();
        let split = (SAMPLE_RATE as f32 * gesture.short) as usize;
        let samples = of(blip);
        (pitch(&samples[..split]), pitch(&samples[split..]))
    }

    #[test]
    fn every_sound_is_audible_and_stays_within_pleasant_bounds() {
        for blip in EVERY {
            let samples = of(blip);
            assert!(!samples.is_empty(), "{blip:?} made no sound");
            for sample in samples {
                assert!((-0.6..=0.6).contains(&sample), "{blip:?} peaked at {sample}");
            }
        }
    }

    #[test]
    fn no_sound_starts_or_ends_with_a_click() {
        for blip in EVERY {
            let samples = of(blip);
            assert!(samples[0].abs() < 0.01, "{blip:?} opens on a step: {}", samples[0]);
            let last = samples[samples.len() - 1];
            assert!(last.abs() < 0.01, "{blip:?} closes on a step: {last}");
        }
    }

    #[test]
    fn open_rises_and_closed_falls() {
        for (blip, opening) in [
            (Blip::MicOn, true),
            (Blip::EarsOn, true),
            (Blip::MicOff, false),
            (Blip::EarsOff, false),
        ] {
            let (first, second) = halves(blip);
            if opening {
                assert!(second > first, "{blip:?} must rise: {first} -> {second}");
            } else {
                assert!(second < first, "{blip:?} must fall: {first} -> {second}");
            }
        }
    }

    #[test]
    fn the_ears_are_the_microphone_an_octave_down() {
        // The point of the family: the two pairs are one gesture at two heights, so
        // whichever you learn first teaches the other.
        let (mic_low, mic_high) = halves(Blip::MicOn);
        let (ears_low, ears_high) = halves(Blip::EarsOn);

        assert!((mic_low / ears_low - 2.0).abs() < 0.05, "{mic_low} vs {ears_low}");
        assert!((mic_high / ears_high - 2.0).abs() < 0.05, "{mic_high} vs {ears_high}");
    }

    /// Read from the notes rather than from the rendered audio: at 196 Hz a 45 ms
    /// tone is only nine cycles, and counting zero crossings over nine cycles cannot
    /// resolve an interval to better than a few percent. The rendered audio is what
    /// the octave and direction tests check; this one is about the notes themselves.
    #[test]
    fn both_pairs_are_the_same_interval() {
        let mic = Blip::MicOn.gesture();
        let ears = Blip::EarsOn.gesture();
        let mic = mic.second / mic.first;
        let ears = ears.second / ears.first;

        assert!((mic - ears).abs() < 0.001, "one gesture, two heights: {mic} vs {ears}");
        assert!((mic - 1.5).abs() < 0.005, "a perfect fifth, not something arbitrary: {mic}");
    }

    #[test]
    fn a_room_event_does_not_sound_like_something_you_did() {
        let (chime_low, chime_high) = halves(Blip::Chime);
        assert!((chime_high / chime_low - 1.5).abs() > 0.1, "the chime keeps its own interval");
        let (mic_low, _) = halves(Blip::MicOn);
        assert!(chime_low > mic_low, "the chime sits above the pair you control");
    }

    #[test]
    fn no_two_sounds_are_the_same() {
        let mut heard: Vec<(i32, i32)> = EVERY
            .iter()
            .map(|blip| {
                let (first, second) = halves(*blip);
                (first as i32 / 5, second as i32 / 5)
            })
            .collect();
        let all = heard.len();
        heard.sort_unstable();
        heard.dedup();
        assert_eq!(heard.len(), all, "two sounds land on the same pair of notes");
    }
}
