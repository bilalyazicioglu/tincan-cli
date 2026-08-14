//! Konuşma algılama (VAD).
//!
//! İki işi var: kullanıcı listesinde "kim konuşuyor" göstergesini beslemek ve sessizken
//! paket göndermeyi kesmek (DTX) — 6 kişilik bir odada genelde bir kişi konuşur, geri
//! kalan beşin sessizliğini ağdan taşımanın anlamı yok.
//!
//! Basit bir RMS eşiği tek başına yetmez: kelimeler arasındaki kısa duraklarda gösterge
//! titrer ve sesin sonu kesilir. Bu yüzden konuşma bittikten sonra kısa bir süre daha
//! açık kalınır (hangover).

/// Çerçevenin ortalama enerjisi (kök ortalama kare).
pub fn rms(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let sum: f32 = pcm.iter().map(|s| s * s).sum();
    (sum / pcm.len() as f32).sqrt()
}

pub struct Vad {
    threshold: f32,
    /// Sessizliğe düştükten sonra kaç çerçeve daha açık kalınacağı.
    hangover: u32,
    remaining: u32,
}

impl Default for Vad {
    fn default() -> Self {
        // ~0.01 RMS sessiz bir odadaki fısıltının biraz üstü; 20ms'lik çerçevelerde
        // 15 çerçeve ≈ 300ms hangover, kelime araları için rahat bir pay.
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

    /// Çerçeveyi değerlendirir; `true` ise bu çerçeve gönderilmeli ve kullanıcı
    /// "konuşuyor" olarak gösterilmeli.
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
        assert!(vad.update(&tone(0.5)), "net konuşma algılanmalı");

        let mut quiet = Vad::default();
        assert!(!quiet.update(&[0.0001; 960]), "ortam gürültüsü konuşma sayılmamalı");
    }

    /// Kelime arasındaki kısa sessizlik göstergeyi söndürmemeli.
    #[test]
    fn short_pauses_do_not_cut_speech_off() {
        let mut vad = Vad::new(0.01, 15);
        assert!(vad.update(&tone(0.5)));

        for frame in 0..14 {
            assert!(
                vad.update(&[0.0; 960]),
                "{frame}. sessiz çerçevede kesilmemeli"
            );
        }
    }

    /// Ama gerçekten susulduğunda kapanmalı — yoksa DTX hiç devreye girmez.
    #[test]
    fn sustained_silence_eventually_stops_transmission() {
        let mut vad = Vad::new(0.01, 15);
        vad.update(&tone(0.5));
        for _ in 0..15 {
            vad.update(&[0.0; 960]);
        }
        assert!(!vad.is_active(), "uzun sessizlikte kapanmalı");
        assert!(!vad.update(&[0.0; 960]));
    }

    #[test]
    fn speech_resets_the_hangover() {
        let mut vad = Vad::new(0.01, 5);
        vad.update(&tone(0.5));
        vad.update(&[0.0; 960]);
        vad.update(&[0.0; 960]);
        // Yeniden konuşma başlarsa sayaç baştan dolmalı.
        vad.update(&tone(0.5));
        for _ in 0..4 {
            assert!(vad.update(&[0.0; 960]));
        }
    }
}
