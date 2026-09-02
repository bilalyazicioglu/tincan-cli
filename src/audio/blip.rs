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
//!
//! A message is the only sound here that is a single note, because it is the only one
//! that happens over and over: one note, one message. And a key going down is not a
//! note at all — it is noise, which is what a key actually is.

use std::f32::consts::PI;

use super::SAMPLE_RATE;

/// What the interface has to say.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Blip {
    /// Someone arrived, or you moved between screens.
    Chime,
    /// A message was sent or arrived.
    Message,
    /// A key on the way down. Carries the key itself, so the same one always sounds
    /// the same, and how loud the user asked for it to be.
    Click { key: char, volume: f32 },
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

/// One or two tones and how to voice them.
struct Gesture {
    first: f32,
    /// `None` for the sounds that are a single note.
    second: Option<f32>,
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
                second: Some(E5),
                short: 0.040,
                long: 0.060,
                harmonic: 0.15,
                amplitude: 0.40,
            },
            // Short and quiet: it fires every time anyone says anything, and a sound
            // you will hear a hundred times has to be one you can stop noticing.
            Blip::Message => Gesture {
                first: D5,
                second: None,
                short: 0.045,
                long: 0.0,
                harmonic: 0.10,
                amplitude: 0.22,
            },
            Blip::MicOn => Gesture {
                first: G4,
                second: Some(D5),
                short: 0.035,
                long: 0.055,
                harmonic: 0.12,
                amplitude: 0.35,
            },
            Blip::MicOff => Gesture {
                first: D5,
                second: Some(G4),
                ..Blip::MicOn.gesture()
            },
            // Lower and longer, because losing the room is the heavier state. The
            // extra harmonic is not brightness for its own sake: a 196 Hz fundamental
            // is more than a small laptop speaker can move, and the overtone is what
            // carries it there.
            Blip::EarsOn => Gesture {
                first: G3,
                second: Some(D4),
                short: 0.045,
                long: 0.075,
                harmonic: 0.25,
                amplitude: 0.35,
            },
            Blip::EarsOff => Gesture {
                first: D4,
                second: Some(G3),
                ..Blip::EarsOn.gesture()
            },
            // Not a gesture at all; `of` deals with it before ever asking.
            Blip::Click { .. } => unreachable!("a key click is noise, not notes"),
        }
    }
}

/// Renders one of the interface's sounds as 48 kHz mono PCM.
pub fn of(blip: Blip) -> Vec<f32> {
    if let Blip::Click { key, volume } = blip {
        return click(key, volume);
    }
    let gesture = blip.gesture();
    let mut samples = Vec::new();
    tone(&mut samples, gesture.first, gesture.short, &gesture, 0.0);
    if let Some(second) = gesture.second {
        tone(&mut samples, second, gesture.long, &gesture, LIFT);
    }
    samples
}

/// How loud a key is at full volume. Well under the notes: it happens constantly, and
/// is meant to sit under what you are doing rather than announce itself.
const CLICK_AMPLITUDE: f32 = 0.22;
/// Long enough to have a body, short enough to keep up with fast hands.
const CLICK_SECONDS: f32 = 0.018;
/// The spacebar is a bigger key and sounds like one.
const SPACE_SECONDS: f32 = 0.026;
/// A rise this long keeps the first sample off a step, which would be a click on top
/// of the click.
const CLICK_ATTACK: f32 = 0.0008;
/// Backspace, as a key code.
const BACKSPACE: char = '\u{8}';

/// A key going down.
///
/// Not a note: a burst of noise under a fast decay, which is what a key actually is.
/// The noise is seeded from the character, so the same key always sounds the same and
/// two different keys do not — a keyboard has a consistent voice, and a random one
/// would only sound arbitrary. The spacebar is lower and longer, being under your
/// thumb; backspace is duller, being the key you press to undo something.
fn click(key: char, volume: f32) -> Vec<f32> {
    let volume = volume.clamp(0.0, 1.0);
    if volume <= 0.0 {
        return Vec::new();
    }

    let mut noise = Noise::seeded(key);
    let seconds = match key {
        ' ' => SPACE_SECONDS,
        BACKSPACE => CLICK_SECONDS * 0.8,
        // A little spread, so a sentence does not tick like a metronome.
        _ => CLICK_SECONDS * (0.85 + noise.unit() * 0.3),
    };
    // A lower cut-off is a duller key; the two big keys sit under the letters.
    let cutoff = match key {
        ' ' => 0.18,
        BACKSPACE => 0.16,
        _ => 0.28 + noise.unit() * 0.30,
    };

    let rate = SAMPLE_RATE as f32;
    let len = (rate * seconds) as usize;
    let attack = (rate * CLICK_ATTACK).max(1.0);
    let mut filtered = 0.0f32;
    let mut samples = Vec::with_capacity(len);

    for i in 0..len {
        let raw = noise.unit() * 2.0 - 1.0;
        filtered += (raw - filtered) * cutoff;

        let progress = i as f32 / len as f32;
        let rise = (i as f32 / attack).min(1.0);
        let fall = (1.0 - progress).powf(2.5);
        samples.push(CLICK_AMPLITUDE * volume * rise * fall * filtered);
    }
    samples
}

/// A tiny deterministic noise source, seeded from a key so that key always sounds the
/// same. No dependency, and no difference between one run and the next.
struct Noise(u32);

impl Noise {
    fn seeded(key: char) -> Self {
        // Mixed rather than used raw, or neighbouring letters would sound alike.
        let seed = (key as u32).wrapping_mul(2_654_435_761) ^ 0x9E37_79B9;
        Self(seed | 1)
    }

    /// The next value in 0.0..1.0.
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }
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

    const EVERY: [Blip; 6] = [
        Blip::Chime,
        Blip::Message,
        Blip::MicOn,
        Blip::MicOff,
        Blip::EarsOn,
        Blip::EarsOff,
    ];

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
        if gesture.second.is_none() {
            let single = pitch(&samples);
            return (single, single);
        }
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
    fn a_message_is_the_one_sound_that_is_a_single_note() {
        // Counting zero crossings over a 45 ms window resolves to about ten hertz, so
        // this asks whether it is D5 rather than pretending to measure it exactly.
        let heard = pitch(&of(Blip::Message));
        assert!((heard - D5).abs() < D5 * 0.03, "one note, and it is D5: heard {heard}");

        for blip in EVERY {
            if blip == Blip::Message {
                continue;
            }
            let (first, second) = halves(blip);
            assert_ne!(first.round(), second.round(), "{blip:?} is a gesture, not a note");
        }
    }

    #[test]
    fn a_message_sits_under_the_things_that_only_happen_once() {
        let message = of(Blip::Message);
        let chime = of(Blip::Chime);
        assert!(message.len() < chime.len(), "it fires far more often than an arrival");

        let peak = |samples: &[f32]| samples.iter().fold(0f32, |peak, s| peak.max(s.abs()));
        assert!(peak(&message) < peak(&chime), "so it has to be quieter too");
    }

    #[test]
    fn the_same_key_always_sounds_the_same_and_two_keys_do_not() {
        let a = Blip::Click { key: 'a', volume: 1.0 };
        let b = Blip::Click { key: 'b', volume: 1.0 };
        assert_eq!(of(a), of(a), "a keyboard is consistent");
        assert_ne!(of(a), of(b), "and a keyboard where every key sounds alike is what we are avoiding");
    }

    #[test]
    fn the_spacebar_is_the_biggest_key_on_the_board() {
        let space = of(Blip::Click { key: ' ', volume: 1.0 });
        let letter = of(Blip::Click { key: 'k', volume: 1.0 });
        assert!(space.len() > letter.len(), "it is longer under the thumb");
    }

    #[test]
    fn a_click_turned_all_the_way_down_is_silence() {
        assert!(of(Blip::Click { key: 'a', volume: 0.0 }).is_empty());

        let peak = |samples: Vec<f32>| samples.iter().fold(0f32, |peak, s| peak.max(s.abs()));
        assert!(
            peak(of(Blip::Click { key: 'a', volume: 0.2 }))
                < peak(of(Blip::Click { key: 'a', volume: 1.0 })),
            "and the dial in between has to do something"
        );
    }

    #[test]
    fn a_click_opens_and_closes_without_a_step_of_its_own() {
        let click = of(Blip::Click { key: 'q', volume: 1.0 });
        assert!(click[0].abs() < 0.01, "opens on {}", click[0]);
        assert!(click[click.len() - 1].abs() < 0.01, "closes on {}", click[click.len() - 1]);
        assert!(click.iter().all(|s| s.abs() <= 0.3), "and never gets loud");
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
        let mic = mic.second.unwrap() / mic.first;
        let ears = ears.second.unwrap() / ears.first;

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
