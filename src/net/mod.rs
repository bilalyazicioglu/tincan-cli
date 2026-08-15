//! Ağ katmanı: iroh endpoint'i, kontrol düzlemi ve (Faz 2'de) ses mesh'i.

pub mod control;
pub mod endpoint;

use tokio::sync::mpsc;

use crate::proto::{ChannelId, ChatLine, PeerId, PeerInfo, RoomSnapshot};

/// Arayüzden gelen kullanıcı eylemleri.
///
/// Host da, katılan da aynı komutları gönderir; farkı `Session`'ın hangi kurucuyla
/// yaratıldığı belirler. Arayüz kimin host olduğunu bilmek zorunda kalmaz.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SwitchChannel(Option<ChannelId>),
    /// Kanal açıkça taşınır: kullanıcı mesajı yazarken kanal değiştirebilir,
    /// mesaj yazıldığı kanala düşmeli.
    Chat { channel: ChannelId, text: String },
    SetMuted(bool),
    SetDeafened(bool),
    Quit,
}

/// Arayüze giden olaylar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Welcome { me: PeerId, room: RoomSnapshot },
    Roster(Vec<PeerInfo>),
    Chat(ChatLine),
    Notice(String),
    /// Oturum bitti — koordinatör kapandı, bağlantı koptu ya da reddedildik.
    Disconnected(String),
}

/// Arayüzün kontrol düzlemine tutunduğu yer.
pub struct Session {
    pub me: PeerId,
    /// Davet kodu — host modunda kendi kodumuz, katılan modda bağlandığımız oda.
    pub invite_code: String,
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
}

/// Koordinatörün chat sıralaması için kullandığı saat.
pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
pub mod voice;
