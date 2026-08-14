//! Ses mesh'i: aynı kanaldaki peer'lar arasında doğrudan datagram taşıma.
//!
//! Kontrol düzleminden tamamen ayrı çalışır. Koordinatör kimin hangi kanalda olduğunu
//! söyler; bağlantıları ve ses paketlerini peer'lar kendi aralarında halleder. Ses
//! trafiği hiçbir zaman koordinatör üzerinden geçmez.
//!
//! İki incelik:
//!
//! * **Çift bağlantı.** İki peer aynı anda birbirine bağlanmaya kalkarsa iki ayrı
//!   bağlantı oluşur ve ses iki kez duyulur. Kural: kimliği küçük olan bağlanır,
//!   büyük olan bekler. Kimlikler public key olduğu için bu, ek anlaşma gerektirmeyen
//!   deterministik bir görev paylaşımıdır.
//! * **Kanal filtresi.** Gelen paketin kanalı bizimkiyle eşleşmiyorsa çalınmaz.
//!   Kanal değişimi ile roster güncellemesi arasındaki kısa boşlukta yanlış kanaldan
//!   ses sızmasını bu engeller.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use bytes::Bytes;
use iroh::Endpoint;
use iroh::endpoint::Connection;
use tokio::sync::{Mutex, mpsc};
use tracing::debug;

use super::endpoint::{to_endpoint_id, to_peer_id};
use crate::audio::Incoming;
use crate::proto::{self, ChannelId, PeerId, VoiceHeader};

struct Shared {
    me: PeerId,
    /// Çözülmüş çerçevelerin gideceği yer (ses motoru).
    incoming: mpsc::Sender<Incoming>,
    /// Kurulu ses bağlantıları.
    connections: Mutex<HashMap<PeerId, Connection>>,
    /// Kendi ses kanalımız; hem gönderilen başlığa yazılır hem gelen filtrelenir.
    channel: Mutex<Option<ChannelId>>,
    /// Giden çerçeve sayacı.
    seq: AtomicU32,
}

/// Ses mesh'inin dışarıya açılan yüzü.
#[derive(Clone)]
pub struct VoiceMesh {
    shared: Arc<Shared>,
    endpoint: Endpoint,
}

impl VoiceMesh {
    /// Mesh'i kurar ve giden ses çerçevelerini dağıtan döngüyü başlatır.
    pub fn start(
        endpoint: Endpoint,
        me: PeerId,
        incoming: mpsc::Sender<Incoming>,
        mut outgoing: mpsc::Receiver<Vec<u8>>,
    ) -> Self {
        let shared = Arc::new(Shared {
            me,
            incoming,
            connections: Mutex::new(HashMap::new()),
            channel: Mutex::new(None),
            seq: AtomicU32::new(0),
        });

        let sender = shared.clone();
        tokio::spawn(async move {
            let mut datagram = vec![0u8; VoiceHeader::SIZE + 1500];
            while let Some(payload) = outgoing.recv().await {
                let Some(channel) = *sender.channel.lock().await else {
                    continue; // Ses kanalında değiliz; konuşulan bir yer yok.
                };

                let seq = sender.seq.fetch_add(1, Ordering::Relaxed);
                let header = VoiceHeader { seq, channel };
                let total = VoiceHeader::SIZE + payload.len();
                if total > datagram.len() {
                    continue;
                }
                header.write_into(&mut datagram);
                datagram[VoiceHeader::SIZE..total].copy_from_slice(&payload);
                let frame = Bytes::copy_from_slice(&datagram[..total]);

                let connections = sender.connections.lock().await;
                for (peer, conn) in connections.iter() {
                    // Datagram kaybı ses için normaldir; başarısızlık sessizce geçilir.
                    if let Err(err) = conn.send_datagram(frame.clone()) {
                        debug!("{} peer'ına ses gönderilemedi: {err}", peer.short());
                    }
                }
            }
        });

        Self { shared, endpoint }
    }

    /// Gelen bir ses bağlantısını mesh'e katar.
    pub fn accept(&self, conn: Connection) {
        let peer = to_peer_id(conn.remote_id());
        let shared = self.shared.clone();
        tokio::spawn(async move {
            debug!("{} ile ses bağlantısı kuruldu (gelen)", peer.short());
            shared.connections.lock().await.insert(peer, conn.clone());
            read_loop(shared.clone(), conn, peer).await;
            shared.connections.lock().await.remove(&peer);
        });
    }

    /// Koordinatörden gelen roster'a göre mesh'i günceller: kendi kanalımızdaki
    /// peer'larla bağlantı kurar, kanaldan çıkanlarla olan bağlantıyı kapatır.
    pub async fn set_membership(&self, channel: Option<ChannelId>, mut members: Vec<PeerId>) {
        *self.shared.channel.lock().await = channel;
        members.retain(|peer| *peer != self.shared.me);
        let wanted: HashSet<PeerId> = if channel.is_some() {
            members.into_iter().collect()
        } else {
            HashSet::new()
        };

        // Artık aynı kanalda olmayanları bırak.
        let mut connections = self.shared.connections.lock().await;
        connections.retain(|peer, conn| {
            let keep = wanted.contains(peer);
            if !keep {
                conn.close(0u32.into(), b"kanal degisti");
            }
            keep
        });
        let existing: HashSet<PeerId> = connections.keys().copied().collect();
        drop(connections);

        for peer in wanted.difference(&existing) {
            // Yalnızca kimliği küçük olan taraf bağlantıyı başlatır; diğeri bekler.
            if self.shared.me.0 > peer.0 {
                continue;
            }
            self.dial(*peer);
        }
    }

    fn dial(&self, peer: PeerId) {
        let shared = self.shared.clone();
        let endpoint = self.endpoint.clone();
        tokio::spawn(async move {
            let Ok(target) = to_endpoint_id(&peer) else {
                return;
            };
            match endpoint.connect(target, proto::VOICE_ALPN).await {
                Ok(conn) => {
                    debug!("{} ile ses bağlantısı kuruldu (giden)", peer.short());
                    shared.connections.lock().await.insert(peer, conn.clone());
                    read_loop(shared.clone(), conn, peer).await;
                    shared.connections.lock().await.remove(&peer);
                }
                Err(err) => debug!("{} peer'ına ses bağlantısı kurulamadı: {err}", peer.short()),
            }
        });
    }
}

/// Koordinatör olmayan peer'lar için gelen ses bağlantılarını karşılayan döngü.
///
/// Koordinatörde bu işi kontrol düzleminin accept döngüsü yapar: tek bir endpoint'i
/// iki döngü birden dinleyemez.
pub fn spawn_accept(endpoint: Endpoint, mesh: VoiceMesh) {
    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            let mesh = mesh.clone();
            tokio::spawn(async move {
                let Ok(mut accepting) = incoming.accept() else {
                    return;
                };
                let Ok(alpn) = accepting.alpn().await else {
                    return;
                };
                if alpn != proto::VOICE_ALPN {
                    debug!("ses dışı bağlantı geldi, yoksayılıyor");
                    return;
                }
                match accepting.await {
                    Ok(conn) => mesh.accept(conn),
                    Err(err) => debug!("ses bağlantısı kurulamadı: {err:#}"),
                }
            });
        }
    });
}

/// Bir peer'dan gelen ses datagramlarını okur ve ses motoruna iletir.
async fn read_loop(shared: Arc<Shared>, conn: Connection, peer: PeerId) {
    while let Ok(datagram) = conn.read_datagram().await {
        let Some((header, payload)) = VoiceHeader::parse(&datagram) else {
            continue; // Bozuk paket: at, bağlantıyı koparma.
        };

        // Bizim kanalımızdan değilse çalınmaz.
        if *shared.channel.lock().await != Some(header.channel) {
            continue;
        }

        let frame = Incoming {
            from: peer,
            seq: header.seq,
            payload: payload.to_vec(),
        };
        // Ses motoru yetişemiyorsa çerçeveyi düşürmek, kuyruk biriktirip
        // gecikmeyi büyütmekten iyidir.
        if shared.incoming.try_send(frame).is_err() {
            debug!("ses kuyruğu dolu, çerçeve düştü");
        }
    }
}
