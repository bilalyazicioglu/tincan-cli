//! Arayüzün gördüğü durum ve olayların ona nasıl işlendiği.
//!
//! Terminalden ve ağdan bağımsız tutuldu: burası saf bir durum makinesi olduğu için
//! "kim nerede görünüyor, hangi mesaj hangi panele düşüyor" soruları test edilebiliyor.

use std::collections::HashMap;

use crate::net::Event;
use crate::proto::{ChannelId, ChatLine, PeerId, PeerInfo};

/// Sohbet panelinde tutulan en fazla satır.
const VISIBLE_HISTORY: usize = 500;

/// Sohbet panelindeki bir satır: ya bir kişinin mesajı ya da sistem bildirimi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Chat(ChatLine),
    Notice { text: String, at: u64 },
}

pub struct App {
    pub me: PeerId,
    pub room_name: String,
    pub invite_code: String,
    pub channels: Vec<String>,
    pub peers: Vec<PeerInfo>,
    /// Bir kez görülen her kimliğin adı. Roster küçülse de burası küçülmez:
    /// sohbet geçmişindeki eski mesajlar sahiplerinin adını korusun diye.
    names: HashMap<PeerId, String>,
    pub lines: Vec<Line>,
    /// Ekranda açık olan kanal — yazılan mesaj buraya gider.
    pub viewing: ChannelId,
    /// Sesle bağlı olunan kanal. Görüntülenen kanaldan bağımsızdır:
    /// kullanıcı "oyun"da konuşurken "genel"deki yazışmayı okuyabilmeli.
    pub voice: Option<ChannelId>,
    pub muted: bool,
    pub input: String,
    pub status: Option<String>,
    /// Oturum bittiğinde dolan gerekçe; dolduğunda arayüz kapanır.
    pub ended: Option<String>,
}

impl App {
    pub fn new(me: PeerId, invite_code: String) -> Self {
        Self {
            me,
            room_name: String::new(),
            invite_code,
            channels: Vec::new(),
            peers: Vec::new(),
            names: HashMap::new(),
            lines: Vec::new(),
            viewing: ChannelId(0),
            voice: None,
            muted: false,
            input: String::new(),
            status: None,
            ended: None,
        }
    }

    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Welcome { me, room } => {
                self.me = me;
                self.room_name = room.room_name;
                self.channels = room.channels;
                self.peers = room.peers;
                self.lines = room.recent_chat.into_iter().map(Line::Chat).collect();
                self.remember_names();
                // Kendi durumumuz sunucudan gelen listede — yeniden bağlanmada da doğru olsun.
                self.sync_self_from_roster();
            }
            Event::Roster(peers) => {
                self.peers = peers;
                self.remember_names();
                self.sync_self_from_roster();
            }
            Event::Chat(line) => self.push(Line::Chat(line)),
            Event::Notice(text) => {
                let at = crate::net::now();
                self.push(Line::Notice { text, at });
            }
            Event::Disconnected(reason) => self.ended = Some(reason),
        }
    }

    fn push(&mut self, line: Line) {
        self.lines.push(line);
        if self.lines.len() > VISIBLE_HISTORY {
            self.lines.drain(..self.lines.len() - VISIBLE_HISTORY);
        }
    }

    fn remember_names(&mut self) {
        for peer in &self.peers {
            self.names.insert(peer.id, peer.name.clone());
        }
    }

    /// Ses/sustur durumunu koordinatörün söylediğiyle hizalar.
    fn sync_self_from_roster(&mut self) {
        if let Some(me) = self.peers.iter().find(|p| p.id == self.me) {
            self.voice = me.channel;
            self.muted = me.muted;
        }
    }

    /// Görüntülenen kanala ait satırlar. Bildirimler her kanalda görünür —
    /// "X odaya katıldı" bilgisi kanala bağlı değil.
    pub fn visible_lines(&self) -> Vec<&Line> {
        self.lines
            .iter()
            .filter(|line| match line {
                Line::Chat(chat) => chat.channel == self.viewing,
                Line::Notice { .. } => true,
            })
            .collect()
    }

    pub fn peers_in(&self, channel: ChannelId) -> Vec<&PeerInfo> {
        self.peers
            .iter()
            .filter(|p| p.channel == Some(channel))
            .collect()
    }

    pub fn name_of(&self, id: PeerId) -> String {
        self.names.get(&id).cloned().unwrap_or_else(|| id.short())
    }

    pub fn channel_name(&self, channel: ChannelId) -> &str {
        self.channels
            .get(channel.0 as usize)
            .map(String::as_str)
            .unwrap_or("?")
    }

    /// Bir sonraki kanala geçer (görüntüleme).
    pub fn view_next(&mut self, forward: bool) {
        if self.channels.is_empty() {
            return;
        }
        let count = self.channels.len() as u8;
        self.viewing = ChannelId(if forward {
            (self.viewing.0 + 1) % count
        } else {
            (self.viewing.0 + count - 1) % count
        });
    }

    /// Yazılan mesajı alır ve girdi alanını temizler; gönderilecek bir şey yoksa `None`.
    pub fn take_input(&mut self) -> Option<String> {
        let text = self.input.trim().to_string();
        self.input.clear();
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::RoomSnapshot;

    fn peer(seed: u8, channel: Option<ChannelId>) -> PeerInfo {
        PeerInfo {
            id: PeerId([seed; 32]),
            name: format!("kişi{seed}"),
            channel,
            muted: false,
        }
    }

    fn welcomed() -> App {
        let mut app = App::new(PeerId([1; 32]), "kod".into());
        app.apply(Event::Welcome {
            me: PeerId([1; 32]),
            room: RoomSnapshot {
                room_name: "oda".into(),
                channels: vec!["genel".into(), "oyun".into(), "müzik".into()],
                peers: vec![peer(1, None), peer(2, Some(ChannelId(1)))],
                recent_chat: vec![],
            },
        });
        app
    }

    #[test]
    fn welcome_populates_the_room() {
        let app = welcomed();
        assert_eq!(app.room_name, "oda");
        assert_eq!(app.channels.len(), 3);
        assert_eq!(app.peers.len(), 2);
        assert_eq!(app.viewing, ChannelId(0));
        assert_eq!(app.voice, None, "sese otomatik girilmemeli");
    }

    /// Kullanıcı bir kanalda konuşurken başka bir kanalın yazışmasını okuyabilmeli.
    #[test]
    fn viewing_and_voice_channels_are_independent() {
        let mut app = welcomed();
        app.apply(Event::Roster(vec![
            peer(1, Some(ChannelId(2))),
            peer(2, Some(ChannelId(1))),
        ]));

        assert_eq!(app.voice, Some(ChannelId(2)), "ses kanalı roster'dan gelmeli");
        assert_eq!(app.viewing, ChannelId(0), "görüntülenen kanal değişmemeli");

        app.view_next(true);
        assert_eq!(app.viewing, ChannelId(1));
        assert_eq!(app.voice, Some(ChannelId(2)), "gezinmek sesi taşımamalı");
    }

    #[test]
    fn channel_view_wraps_in_both_directions() {
        let mut app = welcomed();
        app.view_next(false);
        assert_eq!(app.viewing, ChannelId(2), "geriye sarmalı");
        app.view_next(true);
        assert_eq!(app.viewing, ChannelId(0), "ileri sarmalı");
    }

    #[test]
    fn chat_is_filtered_by_channel_but_notices_are_not() {
        let mut app = welcomed();
        app.apply(Event::Chat(ChatLine {
            channel: ChannelId(0),
            from: PeerId([2; 32]),
            text: "genel mesaj".into(),
            at: 1,
        }));
        app.apply(Event::Chat(ChatLine {
            channel: ChannelId(1),
            from: PeerId([2; 32]),
            text: "oyun mesajı".into(),
            at: 2,
        }));
        app.apply(Event::Notice("kişi2 odaya katıldı".into()));

        let visible: Vec<String> = app
            .visible_lines()
            .iter()
            .map(|line| match line {
                Line::Chat(c) => c.text.clone(),
                Line::Notice { text, .. } => text.clone(),
            })
            .collect();
        assert_eq!(visible, vec!["genel mesaj", "kişi2 odaya katıldı"]);

        app.view_next(true);
        let visible: Vec<String> = app
            .visible_lines()
            .iter()
            .map(|line| match line {
                Line::Chat(c) => c.text.clone(),
                Line::Notice { text, .. } => text.clone(),
            })
            .collect();
        assert_eq!(visible, vec!["oyun mesajı", "kişi2 odaya katıldı"]);
    }

    #[test]
    fn roster_drives_channel_membership_display() {
        let mut app = welcomed();
        assert_eq!(app.peers_in(ChannelId(1)).len(), 1);
        assert_eq!(app.peers_in(ChannelId(0)).len(), 0);

        app.apply(Event::Roster(vec![peer(1, Some(ChannelId(0))), peer(2, None)]));
        assert_eq!(app.peers_in(ChannelId(0)).len(), 1);
        assert_eq!(app.peers_in(ChannelId(1)).len(), 0);
    }

    #[test]
    fn input_is_trimmed_and_blank_input_sends_nothing() {
        let mut app = welcomed();
        app.input = "   ".into();
        assert_eq!(app.take_input(), None);
        assert!(app.input.is_empty(), "geçersiz girdi de temizlenmeli");

        app.input = "  selam  ".into();
        assert_eq!(app.take_input().as_deref(), Some("selam"));
    }

    #[test]
    fn history_is_bounded() {
        let mut app = welcomed();
        for i in 0..VISIBLE_HISTORY + 100 {
            app.apply(Event::Notice(format!("bildirim {i}")));
        }
        assert_eq!(app.lines.len(), VISIBLE_HISTORY);
    }

    #[test]
    fn disconnect_ends_the_session() {
        let mut app = welcomed();
        assert!(app.ended.is_none());
        app.apply(Event::Disconnected("oda kapandı".into()));
        assert_eq!(app.ended.as_deref(), Some("oda kapandı"));
    }

    #[test]
    fn unknown_sender_falls_back_to_short_id() {
        let app = welcomed();
        assert_eq!(app.name_of(PeerId([2; 32])), "kişi2");
        // Hiç görülmemiş bir kimlik için elimizde isim yok.
        assert_eq!(app.name_of(PeerId([9; 32])), PeerId([9; 32]).short());
    }

    /// Biri odadan ayrıldığında, ekranda kalan eski mesajları hâlâ onun adıyla
    /// görünmeli — yoksa sohbet geçmişi ayrılmalarla birlikte okunamaz hale gelir.
    #[test]
    fn names_survive_after_a_peer_leaves() {
        let mut app = welcomed();
        app.apply(Event::Chat(ChatLine {
            channel: ChannelId(0),
            from: PeerId([2; 32]),
            text: "ben gidiyorum".into(),
            at: 1,
        }));
        assert_eq!(app.name_of(PeerId([2; 32])), "kişi2");

        // kişi2 ayrıldı: roster'da artık yok.
        app.apply(Event::Roster(vec![peer(1, None)]));

        assert_eq!(
            app.name_of(PeerId([2; 32])),
            "kişi2",
            "ayrılan kişinin eski mesajı kimliğe dönüşmemeli"
        );
        assert_eq!(app.peers.len(), 1, "kişi listesi yine de güncellenmeli");
    }
}
