//! Birden fazla peer'ın sesini tek çıkışta birleştirme.
//!
//! Mesh'te herkes kendi sesini ayrı bir akış olarak gönderir; hoparlöre giden tek bir
//! sinyal olduğu için bunları toplamak gerekir. Naif toplama, iki kişi aynı anda
//! konuştuğunda genliği taşırır ve sert kırpılma (bozulma) duyulur.
//!
//! Çözüm: toplamı sınırın üstüne çıktığında yumuşakça bastıran bir limitleyici.
//! Kazanç ani değişmesin diye (bu da "pompalama" olarak duyulur) hedefe doğru
//! kademeli yaklaşır.

/// Toplam bu değeri aşarsa limitleyici devreye girer.
const CEILING: f32 = 0.95;
/// Kazancın çerçeve başına toparlanma hızı.
const RECOVERY: f32 = 0.05;

pub struct Mixer {
    /// O anki bastırma katsayısı; 1.0 = bastırma yok.
    gain: f32,
}

impl Default for Mixer {
    fn default() -> Self {
        Self { gain: 1.0 }
    }
}

impl Mixer {
    /// Kaynakları `out` üzerine toplar. `out` çağrıdan önce sıfırlanmış olmalı değil —
    /// fonksiyon kendisi temizler.
    pub fn mix(&mut self, sources: &[&[f32]], out: &mut [f32]) {
        out.fill(0.0);
        for source in sources {
            for (slot, sample) in out.iter_mut().zip(source.iter()) {
                *slot += sample;
            }
        }

        let peak = out.iter().fold(0f32, |max, s| max.max(s.abs()));

        // Tepe tavanı aşıyorsa kazancı hemen indir (bozulmayı önlemek acildir),
        // aşmıyorsa yavaşça geri tırman (ani yükseliş pompalama sesi yapar).
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
        assert!(out.iter().all(|s| *s == 0.0), "önceki içerik temizlenmeli");
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

    /// Asıl mesele: kalabalık kanalda ses bozulmamalı.
    #[test]
    fn loud_sources_are_limited_not_clipped() {
        let mut mixer = Mixer::default();
        let sources: Vec<Vec<f32>> = (0..5).map(|_| constant(0.8, 16)).collect();
        let refs: Vec<&[f32]> = sources.iter().map(|s| s.as_slice()).collect();
        let mut out = vec![0.0; 16];

        mixer.mix(&refs, &mut out);

        let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 1.0, "çıkış taşmamalı, tepe: {peak}");
        assert!(peak > 0.5, "ses duyulur seviyede kalmalı, tepe: {peak}");
        // Hepsi aynı işaretli olduğu için sinyal biçimi korunmalı (düz kırpılma yok).
        assert!(out.iter().all(|s| (*s - out[0]).abs() < 1e-6), "sinyal bozulmamalı");
    }

    /// Gürültü geçtikten sonra kazanç geri gelmeli, yoksa ses kısık kalır.
    #[test]
    fn gain_recovers_after_the_loud_passage() {
        let mut mixer = Mixer::default();
        let loud: Vec<Vec<f32>> = (0..5).map(|_| constant(0.9, 8)).collect();
        let refs: Vec<&[f32]> = loud.iter().map(|s| s.as_slice()).collect();
        let mut out = vec![0.0; 8];
        mixer.mix(&refs, &mut out);
        assert!(mixer.gain < 0.5, "yüksek seste bastırılmalı");

        let quiet = constant(0.1, 8);
        for _ in 0..100 {
            mixer.mix(&[&quiet], &mut out);
        }
        assert!((mixer.gain - 1.0).abs() < 1e-6, "kazanç geri dönmeli");
        assert!((out[0] - 0.1).abs() < 1e-6, "sessiz sinyal kısılmamalı");
    }

    /// Kaynaklar farklı uzunlukta olabilir (kayıp çerçeve, kısa örtme) — panik olmamalı.
    #[test]
    fn tolerates_sources_shorter_than_the_output() {
        let mut mixer = Mixer::default();
        let short = constant(0.5, 2);
        let full = constant(0.1, 8);
        let mut out = vec![0.0; 8];

        mixer.mix(&[&short, &full], &mut out);

        assert!((out[0] - 0.6).abs() < 1e-6, "kısa kaynak başta duyulmalı");
        assert!((out[7] - 0.1).abs() < 1e-6, "bittiği yerden sonrası bozulmamalı");
    }
}
