//! Peer başına jitter buffer.
//!
//! Ağ paketleri düzensiz aralıklarla, sırasız ve eksik gelir; ses kartı ise her 20ms'de
//! bir çerçeve ister, gecikmeyi kabul etmez. Bu tampon ikisinin arasını kurar: küçük bir
//! gecikme biriktirip (varsayılan 60ms) akışı düzleştirir.
//!
//! Üç durumu birbirinden ayırmak kritik:
//!
//! * **Paket var** → çal.
//! * **Paket kayıp** (sonrası geldi, kendisi gelmedi) → kodekten örtme (PLC) iste;
//!   sessizlik koymak "tık" sesi yaratır.
//! * **Kimse konuşmuyor** → gerçek sessizlik. Konuşmayan peer paket göndermediği için
//!   (DTX) bu durum normaldir ve kayıp sayılmamalıdır.

use std::collections::BTreeMap;

/// Tampondan çıkan bir çerçeve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// Çalınacak Opus paketi.
    Packet(Vec<u8>),
    /// Paket kayboldu; kodek örtme üretmeli.
    Lost,
    /// Karşı taraf konuşmuyor.
    Silence,
}

pub struct JitterBuffer {
    /// Çalmaya başlamadan önce biriktirilecek çerçeve sayısı.
    target: usize,
    /// Bu sınırın üstünde birikirse gecikme kabul edilemez hale gelir; öne sarılır.
    max_depth: usize,
    /// Sırada beklenen çerçeve. `None` ise akış duraklamış demektir.
    next_seq: Option<u32>,
    packets: BTreeMap<u32, Vec<u8>>,
    /// Üst üste kaç kez boş çıkıldığı — akışın durduğuna karar vermek için.
    starved: usize,
}

/// Akış durdu sayılmadan önce tolere edilen ardışık boş çerçeve sayısı.
const STARVE_LIMIT: usize = 5;

impl JitterBuffer {
    /// `target` çerçeve cinsinden hedef gecikmedir (20ms'lik çerçevelerde 3 ≈ 60ms).
    pub fn new(target: usize) -> Self {
        Self {
            target: target.max(1),
            max_depth: target.max(1) * 4,
            next_seq: None,
            packets: BTreeMap::new(),
            starved: 0,
        }
    }

    pub fn depth(&self) -> usize {
        self.packets.len()
    }

    /// Gelen paketi yerleştirir. Çok geç kalmış ya da tekrarlanan paketler için `false`.
    pub fn push(&mut self, seq: u32, payload: Vec<u8>) -> bool {
        if let Some(next) = self.next_seq
            && seq < next
        {
            // Treni kaçırmış paket: çalınacağı an geçti, tutmanın anlamı yok.
            return false;
        }
        if self.packets.contains_key(&seq) {
            return false;
        }
        self.packets.insert(seq, payload);

        // Aşırı birikme = aşırı gecikme. En eskileri atıp öne sarıyoruz.
        while self.packets.len() > self.max_depth {
            if let Some(&oldest) = self.packets.keys().next() {
                self.packets.remove(&oldest);
                self.next_seq = Some(oldest + 1);
            }
        }
        true
    }

    /// Ses kartına verilecek bir sonraki çerçeve.
    pub fn pop(&mut self) -> Frame {
        let Some(next) = self.next_seq else {
            // Akış duraklamış: yeniden başlamak için yeterince paket birikmeli.
            if self.packets.len() < self.target {
                return Frame::Silence;
            }
            let first = *self.packets.keys().next().expect("dolu olduğu kontrol edildi");
            self.next_seq = Some(first);
            return self.pop();
        };

        if let Some(payload) = self.packets.remove(&next) {
            self.next_seq = Some(next + 1);
            self.starved = 0;
            return Frame::Packet(payload);
        }

        // Beklenen paket yok. Sonrasında paket varsa gerçekten kaybolmuş demektir.
        if !self.packets.is_empty() {
            self.next_seq = Some(next + 1);
            self.starved = 0;
            return Frame::Lost;
        }

        // Tampon tamamen boş: ya ağ kesildi ya da karşı taraf susuyor.
        self.starved += 1;
        if self.starved >= STARVE_LIMIT {
            // Akışı duraklat; konuşma yeniden başladığında sıra numarası nereden
            // devam ederse etsin yeniden senkron olunur.
            self.next_seq = None;
            self.starved = 0;
        } else {
            self.next_seq = Some(next + 1);
        }
        Frame::Silence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(n: u8) -> Vec<u8> {
        vec![n; 4]
    }

    /// Tampon dolmadan çalmaya başlamamalı: hedef gecikme buna hizmet ediyor.
    #[test]
    fn waits_until_target_depth_before_playing() {
        let mut buffer = JitterBuffer::new(3);
        buffer.push(0, packet(0));
        assert_eq!(buffer.pop(), Frame::Silence, "tek paketle başlamamalı");
        buffer.push(1, packet(1));
        assert_eq!(buffer.pop(), Frame::Silence);
        buffer.push(2, packet(2));

        assert_eq!(buffer.pop(), Frame::Packet(packet(0)), "hedefe ulaşınca akmalı");
        assert_eq!(buffer.pop(), Frame::Packet(packet(1)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(2)));
    }

    /// Sırasız gelen paketler doğru sırayla çalınmalı.
    #[test]
    fn reorders_out_of_order_arrivals() {
        let mut buffer = JitterBuffer::new(3);
        buffer.push(2, packet(2));
        buffer.push(0, packet(0));
        buffer.push(1, packet(1));

        assert_eq!(buffer.pop(), Frame::Packet(packet(0)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(1)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(2)));
    }

    /// Kayıp paket, sessizlik değil örtme istemeli — aradaki fark duyulur.
    #[test]
    fn reports_loss_when_a_gap_is_surrounded_by_data() {
        let mut buffer = JitterBuffer::new(2);
        buffer.push(0, packet(0));
        buffer.push(2, packet(2));
        buffer.push(3, packet(3));

        assert_eq!(buffer.pop(), Frame::Packet(packet(0)));
        assert_eq!(buffer.pop(), Frame::Lost, "1 numaralı çerçeve kayıp");
        assert_eq!(buffer.pop(), Frame::Packet(packet(2)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(3)));
    }

    /// Karşı taraf sustuğunda kayıp raporlanmamalı — DTX sayesinde paket gelmemesi normal.
    #[test]
    fn silence_is_not_treated_as_loss() {
        let mut buffer = JitterBuffer::new(2);
        buffer.push(0, packet(0));
        buffer.push(1, packet(1));
        buffer.pop();
        buffer.pop();

        for _ in 0..20 {
            assert_eq!(buffer.pop(), Frame::Silence, "sessizlik kayıp sayılmamalı");
        }
    }

    /// Uzun sessizlikten sonra konuşma yeniden başlarsa akış yeniden yakalanmalı,
    /// sıra numarası nereden devam ederse etsin.
    #[test]
    fn resynchronises_after_a_long_pause() {
        let mut buffer = JitterBuffer::new(2);
        buffer.push(0, packet(0));
        buffer.push(1, packet(1));
        buffer.pop();
        buffer.pop();
        for _ in 0..STARVE_LIMIT + 2 {
            buffer.pop();
        }

        // Konuşma çok sonra, çok ileri bir sıra numarasıyla yeniden başlıyor.
        buffer.push(900, packet(9));
        buffer.push(901, packet(10));
        assert_eq!(buffer.pop(), Frame::Packet(packet(9)));
        assert_eq!(buffer.pop(), Frame::Packet(packet(10)));
    }

    /// Çalınma anı geçmiş paket kabul edilmemeli.
    #[test]
    fn rejects_packets_that_arrive_too_late() {
        let mut buffer = JitterBuffer::new(1);
        buffer.push(5, packet(5));
        buffer.push(6, packet(6));
        assert_eq!(buffer.pop(), Frame::Packet(packet(5)));

        assert!(!buffer.push(5, packet(5)), "geçmiş çerçeve geri alınmamalı");
        assert_eq!(buffer.pop(), Frame::Packet(packet(6)), "akış bozulmamalı");
    }

    #[test]
    fn rejects_duplicates() {
        let mut buffer = JitterBuffer::new(2);
        assert!(buffer.push(0, packet(0)));
        assert!(!buffer.push(0, packet(0)), "aynı çerçeve iki kez alınmamalı");
        assert_eq!(buffer.depth(), 1);
    }

    /// Ağ toparlanınca biriken yığın gecikmeye dönüşmemeli: tampon öne sarmalı.
    #[test]
    fn drops_backlog_instead_of_accumulating_delay() {
        let mut buffer = JitterBuffer::new(3);
        for seq in 0..100u32 {
            buffer.push(seq, packet(seq as u8));
        }

        assert!(
            buffer.depth() <= 12,
            "gecikme sınırsız büyümemeli, derinlik: {}",
            buffer.depth()
        );

        // Öne sarıldıktan sonra da düzgün akmaya devam etmeli.
        assert!(matches!(buffer.pop(), Frame::Packet(_)));
        assert!(matches!(buffer.pop(), Frame::Packet(_)));
    }
}
