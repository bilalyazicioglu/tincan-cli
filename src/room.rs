//! Odanın otoriter durumu — koordinatörün elindeki tek gerçek kaynak.
//!
//! Burası bilerek saf: ağ, zaman ve ses yok. Koordinatör mantığının tamamı burada
//! test edilir, `net/control.rs` sadece bu tipi mesajlara bağlar.

use std::collections::{BTreeMap, VecDeque};

use anyhow::{Result, bail, ensure};

use crate::proto::{ChannelId, ChatLine, MAX_CHAT_CHARS, MAX_NAME_CHARS, PeerId, PeerInfo, RoomSnapshot};

/// Yeni katılana gönderilen ve bellekte tutulan chat satırı sayısı.
const CHAT_HISTORY: usize = 200;

pub struct Room {
    name: String,
    channels: Vec<String>,
    /// BTreeMap: roster sırası her istemcide aynı olsun diye (kimliğe göre deterministik).
    peers: BTreeMap<PeerId, PeerInfo>,
    chat: VecDeque<ChatLine>,
}

impl Room {
    pub fn new(name: impl Into<String>, channels: Vec<String>) -> Result<Self> {
        let channels: Vec<String> = channels.into_iter().map(|c| c.trim().to_string()).collect();
        ensure!(!channels.is_empty(), "oda en az bir kanal içermeli");
        ensure!(channels.len() <= u8::MAX as usize, "çok fazla kanal");
        ensure!(channels.iter().all(|c| !c.is_empty()), "kanal adı boş olamaz");
        Ok(Self {
            name: name.into(),
            channels,
            peers: BTreeMap::new(),
            chat: VecDeque::new(),
        })
    }

    pub fn channels(&self) -> &[String] {
        &self.channels
    }

    pub fn roster(&self) -> Vec<PeerInfo> {
        self.peers.values().cloned().collect()
    }

    pub fn snapshot(&self) -> RoomSnapshot {
        RoomSnapshot {
            room_name: self.name.clone(),
            channels: self.channels.clone(),
            peers: self.roster(),
            recent_chat: self.chat.iter().cloned().collect(),
        }
    }

    /// Aynı ses kanalındaki peer'lar — Faz 2'de mesh'in kimlerle kurulacağını bu belirler.
    pub fn peers_in_channel(&self, channel: ChannelId) -> Vec<PeerId> {
        self.peers
            .values()
            .filter(|p| p.channel == Some(channel))
            .map(|p| p.id)
            .collect()
    }

    pub fn get(&self, id: &PeerId) -> Option<&PeerInfo> {
        self.peers.get(id)
    }

    /// Odaya katılır. Aynı kimlik yeniden katılırsa kaydı tazelenir (yeniden bağlanma).
    ///
    /// Kullanıcıya gösterilecek nihai adı döndürür — çakışma varsa değiştirilmiş olabilir.
    pub fn join(&mut self, id: PeerId, requested_name: &str) -> Result<String> {
        let name = self.sanitize_name(&id, requested_name)?;
        self.peers.insert(
            id,
            PeerInfo {
                id,
                name: name.clone(),
                channel: None,
                muted: false,
            },
        );
        Ok(name)
    }

    pub fn leave(&mut self, id: &PeerId) -> Option<PeerInfo> {
        self.peers.remove(id)
    }

    pub fn switch_channel(&mut self, id: &PeerId, channel: Option<ChannelId>) -> Result<()> {
        if let Some(ChannelId(index)) = channel {
            ensure!(
                (index as usize) < self.channels.len(),
                "kanal {index} yok"
            );
        }
        let peer = self.peers.get_mut(id).ok_or_else(|| anyhow::anyhow!("odada değilsiniz"))?;
        peer.channel = channel;
        Ok(())
    }

    pub fn set_muted(&mut self, id: &PeerId, muted: bool) -> Result<()> {
        let peer = self.peers.get_mut(id).ok_or_else(|| anyhow::anyhow!("odada değilsiniz"))?;
        peer.muted = muted;
        Ok(())
    }

    /// Chat mesajını doğrular, geçmişe ekler ve dağıtılacak satırı döndürür.
    ///
    /// Zaman damgası dışarıdan verilir: sıralama koordinatörün saatinden gelmeli,
    /// istemcilerin saatlerine güvenilmez.
    pub fn post_chat(&mut self, id: &PeerId, channel: ChannelId, text: &str, at: u64) -> Result<ChatLine> {
        ensure!(self.peers.contains_key(id), "odada değilsiniz");
        ensure!(
            (channel.0 as usize) < self.channels.len(),
            "kanal {} yok",
            channel.0
        );

        let text = text.trim();
        ensure!(!text.is_empty(), "boş mesaj gönderilemez");
        ensure!(
            text.chars().count() <= MAX_CHAT_CHARS,
            "mesaj çok uzun ({} karakter, sınır {})",
            text.chars().count(),
            MAX_CHAT_CHARS
        );

        let line = ChatLine {
            channel,
            from: *id,
            text: text.to_string(),
            at,
        };
        self.chat.push_back(line.clone());
        while self.chat.len() > CHAT_HISTORY {
            self.chat.pop_front();
        }
        Ok(line)
    }

    /// Takma adı temizler ve odada benzersiz kılar.
    fn sanitize_name(&self, id: &PeerId, requested: &str) -> Result<String> {
        let trimmed: String = requested
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_NAME_CHARS)
            .collect();
        let trimmed = trimmed.trim().to_string();
        if trimmed.is_empty() {
            bail!("takma ad boş olamaz");
        }

        // Aynı adı kullanan başka biri varsa kısa kimlikle ayırt et — kimse atılmaz.
        let taken = self
            .peers
            .values()
            .any(|p| p.id != *id && p.name.eq_ignore_ascii_case(&trimmed));
        Ok(if taken {
            format!("{trimmed}#{}", &id.short()[..4])
        } else {
            trimmed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> Room {
        Room::new("test odası", vec!["genel".into(), "oyun".into()]).unwrap()
    }

    fn id(seed: u8) -> PeerId {
        PeerId([seed; 32])
    }

    #[test]
    fn rejects_room_without_channels() {
        assert!(Room::new("x", vec![]).is_err());
        assert!(Room::new("x", vec!["  ".into()]).is_err());
    }

    #[test]
    fn joining_peer_starts_outside_voice_channels() {
        let mut room = room();
        let name = room.join(id(1), "ahmet").unwrap();
        assert_eq!(name, "ahmet");

        let roster = room.roster();
        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].channel, None, "yeni gelen sese otomatik girmemeli");
        assert!(!roster[0].muted);
    }

    #[test]
    fn duplicate_names_are_disambiguated_not_rejected() {
        let mut room = room();
        room.join(id(1), "ahmet").unwrap();
        let second = room.join(id(2), "Ahmet").unwrap();

        assert_ne!(second, "ahmet", "ikinci ahmet ayırt edilebilmeli");
        assert!(second.starts_with("Ahmet#"));
        assert_eq!(room.roster().len(), 2, "kimse dışlanmamalı");
    }

    #[test]
    fn rejoining_same_identity_refreshes_instead_of_duplicating() {
        let mut room = room();
        room.join(id(1), "ahmet").unwrap();
        room.switch_channel(&id(1), Some(ChannelId(1))).unwrap();

        // Bağlantı koptu, aynı kimlik yeni adla döndü.
        let name = room.join(id(1), "ahmet2").unwrap();
        assert_eq!(name, "ahmet2");
        assert_eq!(room.roster().len(), 1, "aynı kimlik iki kez listelenmemeli");
        assert_eq!(room.get(&id(1)).unwrap().channel, None, "durum sıfırlanmalı");
    }

    #[test]
    fn name_is_trimmed_and_capped() {
        let mut room = room();
        let long = "a".repeat(MAX_NAME_CHARS + 20);
        let name = room.join(id(1), &format!("  {long}  ")).unwrap();
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);

        assert!(room.join(id(2), "   ").is_err(), "boş ad reddedilmeli");
        assert!(room.join(id(3), "\u{7}\u{7}").is_err(), "kontrol karakteri ad sayılmaz");
    }

    #[test]
    fn switching_to_unknown_channel_fails_without_changing_state() {
        let mut room = room();
        room.join(id(1), "ahmet").unwrap();
        room.switch_channel(&id(1), Some(ChannelId(0))).unwrap();

        assert!(room.switch_channel(&id(1), Some(ChannelId(9))).is_err());
        assert_eq!(
            room.get(&id(1)).unwrap().channel,
            Some(ChannelId(0)),
            "başarısız geçiş mevcut kanalı bozmamalı"
        );
    }

    #[test]
    fn channel_membership_drives_the_voice_mesh() {
        let mut room = room();
        room.join(id(1), "a").unwrap();
        room.join(id(2), "b").unwrap();
        room.join(id(3), "c").unwrap();
        room.switch_channel(&id(1), Some(ChannelId(0))).unwrap();
        room.switch_channel(&id(2), Some(ChannelId(0))).unwrap();
        room.switch_channel(&id(3), Some(ChannelId(1))).unwrap();

        assert_eq!(room.peers_in_channel(ChannelId(0)), vec![id(1), id(2)]);
        assert_eq!(room.peers_in_channel(ChannelId(1)), vec![id(3)]);

        // Kanaldan çıkan peer mesh'ten düşmeli.
        room.switch_channel(&id(2), None).unwrap();
        assert_eq!(room.peers_in_channel(ChannelId(0)), vec![id(1)]);
    }

    #[test]
    fn leaving_removes_peer_from_roster() {
        let mut room = room();
        room.join(id(1), "ahmet").unwrap();
        assert!(room.leave(&id(1)).is_some());
        assert!(room.roster().is_empty());
        assert!(room.leave(&id(1)).is_none(), "ikinci ayrılma sessizce yoksayılmalı");
    }

    #[test]
    fn strangers_cannot_act() {
        let mut room = room();
        assert!(room.post_chat(&id(99), ChannelId(0), "merhaba", 1).is_err());
        assert!(room.switch_channel(&id(99), Some(ChannelId(0))).is_err());
        assert!(room.set_muted(&id(99), true).is_err());
    }

    #[test]
    fn chat_is_validated() {
        let mut room = room();
        room.join(id(1), "ahmet").unwrap();

        let line = room.post_chat(&id(1), ChannelId(0), "  selam  ", 100).unwrap();
        assert_eq!(line.text, "selam", "baştaki/sondaki boşluk kırpılmalı");
        assert_eq!(line.at, 100, "zaman damgası koordinatörden gelmeli");

        assert!(room.post_chat(&id(1), ChannelId(0), "   ", 1).is_err(), "boş mesaj");
        assert!(room.post_chat(&id(1), ChannelId(5), "x", 1).is_err(), "olmayan kanal");
        let long = "ş".repeat(MAX_CHAT_CHARS + 1);
        assert!(room.post_chat(&id(1), ChannelId(0), &long, 1).is_err(), "uzun mesaj");
    }

    #[test]
    fn chat_history_is_bounded_and_ordered() {
        let mut room = room();
        room.join(id(1), "ahmet").unwrap();
        for i in 0..CHAT_HISTORY + 50 {
            room.post_chat(&id(1), ChannelId(0), &format!("mesaj {i}"), i as u64).unwrap();
        }

        let history = room.snapshot().recent_chat;
        assert_eq!(history.len(), CHAT_HISTORY, "geçmiş sınırsız büyümemeli");
        assert_eq!(history.first().unwrap().text, "mesaj 50", "en eskiler düşmeli");
        assert_eq!(history.last().unwrap().text, format!("mesaj {}", CHAT_HISTORY + 49));
    }

    #[test]
    fn snapshot_carries_everything_a_newcomer_needs() {
        let mut room = room();
        room.join(id(1), "ahmet").unwrap();
        room.switch_channel(&id(1), Some(ChannelId(1))).unwrap();
        room.post_chat(&id(1), ChannelId(0), "merhaba", 5).unwrap();

        let snapshot = room.snapshot();
        assert_eq!(snapshot.room_name, "test odası");
        assert_eq!(snapshot.channels, vec!["genel", "oyun"]);
        assert_eq!(snapshot.peers.len(), 1);
        assert_eq!(snapshot.peers[0].channel, Some(ChannelId(1)));
        assert_eq!(snapshot.recent_chat.len(), 1);
    }
}
