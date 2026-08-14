//! Kontrol düzleminin tel üzerindeki tipleri.
//!
//! Bu modül bilerek iroh'tan bağımsız: kimlikler ham `[u8; 32]` olarak taşınır, böylece
//! protokol ağ katmanı olmadan test edilebilir. iroh tiplerine dönüşüm `net` katmanında.
//!
//! Çerçeveleme: her mesaj `u32` (little-endian) uzunluk öneki + postcard gövdesi.

use serde::{Deserialize, Serialize};

/// Kontrol akışında kullanılan ALPN.
pub const ALPN: &[u8] = b"tincan/control/0";

/// Tek bir kontrol mesajının kabul edilen en büyük boyutu.
/// Chat satırları küçük; bu sınır bozuk/kötü niyetli uzunluk öneklerine karşı korumadır.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Bir chat mesajının en fazla karakter sayısı.
pub const MAX_CHAT_CHARS: usize = 2000;

/// Takma adın en fazla karakter sayısı.
pub const MAX_NAME_CHARS: usize = 24;

/// Peer kimliği — iroh public key'inin ham hali.
///
/// Kimlik ve kriptografik doğrulama aynı şey olduğu için ayrı bir kullanıcı hesabı yok:
/// bir peer'ın kim olduğunu QUIC el sıkışması zaten kanıtlar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    /// Kullanıcı arayüzünde gösterilecek kısa biçim.
    pub fn short(&self) -> String {
        self.0[..5].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl std::fmt::Display for PeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Kanal kimliği — oda oluşturulurken sabitlenen listedeki sıra numarası.
/// MVP'de kanallar dinamik olarak eklenip silinmez.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChannelId(pub u8);

/// Bir peer'ın herkese açık durumu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: PeerId,
    pub name: String,
    /// `None` ise peer odada ama hiçbir ses kanalında değil (sadece metin).
    pub channel: Option<ChannelId>,
    pub muted: bool,
}

/// Odaya yeni katılan bir peer'ın devraldığı tam durum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSnapshot {
    pub room_name: String,
    pub channels: Vec<String>,
    pub peers: Vec<PeerInfo>,
    /// Kanal başına son mesajlar (sıralı, eski → yeni).
    pub recent_chat: Vec<ChatLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatLine {
    pub channel: ChannelId,
    pub from: PeerId,
    pub text: String,
    /// Unix epoch saniyesi — koordinatörün saatine göre, sıralama tek noktadan gelsin diye.
    pub at: u64,
}

/// İstemci → koordinatör.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToCoordinator {
    /// Challenge'a yanıt: takma ad + parola kanıtı.
    Hello { name: String, proof: [u8; 32] },
    /// Ses kanalı değiştir; `None` = ses kanalından tamamen çık.
    SwitchChannel { channel: Option<ChannelId> },
    Chat { channel: ChannelId, text: String },
    SetMuted { muted: bool },
    /// Zarif ayrılma. Bu gelmezse koordinatör bağlantı kopmasından anlar.
    Leave,
}

/// Koordinatör → istemci.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToPeer {
    /// Bağlantı kurulur kurulmaz gönderilir; parola kanıtının tuzu.
    Challenge { nonce: [u8; 16] },
    Welcome { you: PeerId, room: RoomSnapshot },
    Rejected { reason: String },
    /// Üye listesinde herhangi bir değişiklik — tam liste gönderilir.
    ///
    /// Fark (delta) göndermek yerine tam liste: 6 kişilik bir odada liste birkaç yüz
    /// bayt, buna karşılık istemcilerin durumu asla ayrışamaz.
    Roster { peers: Vec<PeerInfo> },
    Chat(ChatLine),
    /// "X odaya katıldı" gibi sistem satırları.
    Notice { text: String },
}

/// Uzunluk önekli çerçeveyi kodlar.
pub fn encode<T: Serialize>(message: &T) -> anyhow::Result<Vec<u8>> {
    let body = postcard::to_stdvec(message)?;
    anyhow::ensure!(
        body.len() <= MAX_MESSAGE_BYTES,
        "mesaj çok büyük: {} bayt",
        body.len()
    );
    let mut framed = Vec::with_capacity(4 + body.len());
    framed.extend_from_slice(&(body.len() as u32).to_le_bytes());
    framed.extend_from_slice(&body);
    Ok(framed)
}

/// Gövdeyi çözer (uzunluk öneki `net` katmanında okunmuş olur).
pub fn decode<T: for<'de> Deserialize<'de>>(body: &[u8]) -> anyhow::Result<T> {
    Ok(postcard::from_bytes(body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_peer(seed: u8) -> PeerInfo {
        PeerInfo {
            id: PeerId([seed; 32]),
            name: format!("kullanıcı{seed}"),
            channel: Some(ChannelId(1)),
            muted: false,
        }
    }

    /// Çerçeveleme + postcard round-trip'i, protokolün her iki yönü için.
    #[test]
    fn frames_round_trip() {
        let messages = vec![
            ToPeer::Challenge { nonce: [7; 16] },
            ToPeer::Welcome {
                you: PeerId([1; 32]),
                room: RoomSnapshot {
                    room_name: "İstanbul".into(),
                    channels: vec!["genel".into(), "oyun".into()],
                    peers: vec![sample_peer(1), sample_peer(2)],
                    recent_chat: vec![ChatLine {
                        channel: ChannelId(0),
                        from: PeerId([2; 32]),
                        text: "merhaba dünya".into(),
                        at: 1_700_000_000,
                    }],
                },
            },
            ToPeer::Roster { peers: vec![sample_peer(3)] },
            ToPeer::Rejected { reason: "parola yanlış".into() },
        ];

        for message in messages {
            let framed = encode(&message).unwrap();
            let len = u32::from_le_bytes(framed[..4].try_into().unwrap()) as usize;
            assert_eq!(len, framed.len() - 4, "uzunluk öneki gövdeyle uyuşmalı");
            let decoded: ToPeer = decode(&framed[4..]).unwrap();
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn client_messages_round_trip() {
        let messages = vec![
            ToCoordinator::Hello { name: "ahmet".into(), proof: [9; 32] },
            ToCoordinator::SwitchChannel { channel: Some(ChannelId(2)) },
            ToCoordinator::SwitchChannel { channel: None },
            ToCoordinator::Chat { channel: ChannelId(0), text: "çok güzel".into() },
            ToCoordinator::SetMuted { muted: true },
            ToCoordinator::Leave,
        ];

        for message in messages {
            let framed = encode(&message).unwrap();
            let decoded: ToCoordinator = decode(&framed[4..]).unwrap();
            assert_eq!(decoded, message);
        }
    }

    /// Türkçe karakterler ve emoji tel üzerinde bozulmamalı.
    #[test]
    fn preserves_non_ascii_text() {
        let line = ChatLine {
            channel: ChannelId(0),
            from: PeerId([1; 32]),
            text: "şşğüöçİ 🎧 çalıyor".into(),
            at: 42,
        };
        let framed = encode(&line).unwrap();
        let decoded: ChatLine = decode(&framed[4..]).unwrap();
        assert_eq!(decoded.text, "şşğüöçİ 🎧 çalıyor");
    }

    #[test]
    fn rejects_oversized_message() {
        let huge = ToCoordinator::Chat {
            channel: ChannelId(0),
            text: "a".repeat(MAX_MESSAGE_BYTES + 1),
        };
        assert!(encode(&huge).is_err());
    }

    #[test]
    fn short_id_is_stable_and_readable() {
        let id = PeerId([0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(id.short(), "abcdef0123");
        assert_eq!(id.to_string().len(), 64);
    }
}
