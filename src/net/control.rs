//! Kontrol düzlemi: koordinatör sunucusu ve katılan istemci.
//!
//! Tasarımın can alıcı noktası: **host'un kendi eylemleri de ağdan gelen eylemlerle
//! aynı fonksiyondan geçer** (`apply`). Host için ayrı bir kısayol olsaydı, host'un
//! gördüğü oda ile ötekilerin gördüğü oda zamanla ayrışırdı.
//!
//! Akış:
//! ```text
//! koordinatör                          katılan
//!     │── Challenge{nonce} ───────────────▶│
//!     │◀── Hello{ad, Argon2id(parola)} ────│
//!     │── Welcome{sen, oda} ──────────────▶│   (ya da Rejected)
//!     │── Roster / Chat / Notice ─────────▶│   (yayın)
//!     │◀── SwitchChannel / Chat / Leave ───│
//! ```

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use iroh::{Endpoint, EndpointAddr};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::{Mutex, broadcast, mpsc};
use tracing::{debug, warn};

use super::endpoint::to_peer_id;
use super::voice::VoiceMesh;
use super::{Command, Event, Session, now};
use crate::auth;
use crate::invite;
use crate::proto::{self, MAX_MESSAGE_BYTES, PeerId, ToCoordinator, ToPeer};
use crate::room::Room;

/// Yayın kanalının derinliği. Yavaş bir istemci bu kadar mesaj geriye düşerse
/// tutarlılığı korumak için tam roster'la senkronlanır.
const BROADCAST_DEPTH: usize = 512;
/// Arayüz olay kuyruğu.
const EVENT_DEPTH: usize = 256;

// ── Çerçeveleme ─────────────────────────────────────────────────────────────────

async fn write_msg<T: Serialize>(stream: &mut SendStream, message: &T) -> Result<()> {
    let framed = proto::encode(message)?;
    stream.write_all(&framed).await.context("akışa yazılamadı")?;
    Ok(())
}

async fn read_msg<T: DeserializeOwned>(stream: &mut RecvStream) -> Result<T> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await.context("akış kapandı")?;
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_MESSAGE_BYTES {
        bail!("karşı taraf {len} baytlık mesaj bildirdi — sınır aşıldı");
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await.context("mesaj yarıda kesildi")?;
    proto::decode(&body)
}

// ── Koordinatör ────────────────────────────────────────────────────────────────

pub(crate) struct Shared {
    room: Mutex<Room>,
    /// Tüm bağlı peer'lara **ve** host'un kendi arayüzüne giden yayın.
    broadcast: broadcast::Sender<ToPeer>,
    password: String,
}

impl Shared {
    /// Tek doğruluk noktası: her eylem odaya burada uygulanır ve sonucu yayınlanır.
    async fn apply(&self, from: PeerId, message: ToCoordinator) -> Result<()> {
        let mut room = self.room.lock().await;
        match message {
            ToCoordinator::Hello { .. } => bail!("el sıkışma zaten tamamlandı"),

            ToCoordinator::SwitchChannel { channel } => {
                room.switch_channel(&from, channel)?;
                let _ = self.broadcast.send(ToPeer::Roster { peers: room.roster() });
            }

            ToCoordinator::Chat { channel, text } => {
                let line = room.post_chat(&from, channel, &text, now())?;
                let _ = self.broadcast.send(ToPeer::Chat(line));
            }

            ToCoordinator::SetMuted { muted } => {
                room.set_muted(&from, muted)?;
                let _ = self.broadcast.send(ToPeer::Roster { peers: room.roster() });
            }

            ToCoordinator::SetDeafened { deafened } => {
                room.set_deafened(&from, deafened)?;
                let _ = self.broadcast.send(ToPeer::Roster { peers: room.roster() });
            }

            ToCoordinator::Leave => {
                if let Some(peer) = room.leave(&from) {
                    let _ = self.broadcast.send(ToPeer::Notice {
                        text: format!("{} odadan ayrıldı", peer.name),
                    });
                    let _ = self.broadcast.send(ToPeer::Roster { peers: room.roster() });
                }
            }
        }
        Ok(())
    }
}

pub struct Coordinator;

impl Coordinator {
    /// Odayı açar ve gelen bağlantıları kabul etmeye başlar.
    pub async fn spawn(
        endpoint: Endpoint,
        room: Room,
        password: String,
        host_name: &str,
        voice: Option<VoiceMesh>,
    ) -> Result<Session> {
        let me = to_peer_id(endpoint.id());
        let invite_code = invite::encode(&me.0);

        let mut room = room;
        room.join(me, host_name).context("host takma adı geçersiz")?;

        let (broadcast_tx, _) = broadcast::channel(BROADCAST_DEPTH);
        let shared = Arc::new(Shared {
            room: Mutex::new(room),
            broadcast: broadcast_tx,
            password,
        });

        let (event_tx, event_rx) = mpsc::channel(EVENT_DEPTH);
        let (command_tx, command_rx) = mpsc::channel(EVENT_DEPTH);

        // Host'un arayüzü de diğer herkesle aynı yayını dinler.
        let snapshot = shared.room.lock().await.snapshot();
        event_tx
            .send(Event::Welcome { me, room: snapshot })
            .await
            .ok();
        tokio::spawn(pump_broadcast_to_ui(
            shared.broadcast.subscribe(),
            event_tx.clone(),
        ));

        tokio::spawn(accept_loop(endpoint.clone(), Some(shared.clone()), voice));
        tokio::spawn(host_commands(shared, command_rx, event_tx, endpoint, me));

        Ok(Session {
            me,
            invite_code,
            commands: command_tx,
            events: event_rx,
        })
    }
}

/// Host'un kendi komutlarını ağdan gelenlerle aynı yoldan işler.
async fn host_commands(
    shared: Arc<Shared>,
    mut commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<Event>,
    endpoint: Endpoint,
    me: PeerId,
) {
    while let Some(command) = commands.recv().await {
        if matches!(command, Command::Quit) {
            let _ = shared.broadcast.send(ToPeer::Notice {
                text: "oda kapanıyor — koordinatör çıktı".into(),
            });
            // Yayının istemcilere ulaşması için kısa bir soluk.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            endpoint.close().await;
            let _ = events.send(Event::Disconnected("oda kapatıldı".into())).await;
            return;
        }
        if let Err(err) = shared.apply(me, into_wire(command)).await {
            // Host'un kendi hatası yayına çıkmaz, sadece kendi ekranına düşer.
            let _ = events.send(Event::Notice(format!("olmadı: {err}"))).await;
        }
    }
}

/// Gelen bağlantıları ALPN'e göre dağıtır.
///
/// Aynı endpoint hem kontrol hem ses bağlantısı kabul eder. Koordinatör olmayan
/// peer'larda `control` boştur — onlar yalnızca ses bağlantısı karşılar.
pub(crate) async fn accept_loop(
    endpoint: Endpoint,
    control: Option<Arc<Shared>>,
    voice: Option<VoiceMesh>,
) {
    while let Some(incoming) = endpoint.accept().await {
        let control = control.clone();
        let voice = voice.clone();
        tokio::spawn(async move {
            let mut accepting = match incoming.accept() {
                Ok(accepting) => accepting,
                Err(err) => {
                    debug!("gelen bağlantı kabul edilemedi: {err:#}");
                    return;
                }
            };
            let alpn = match accepting.alpn().await {
                Ok(alpn) => alpn,
                Err(err) => {
                    debug!("ALPN okunamadı: {err:#}");
                    return;
                }
            };
            let conn = match accepting.await {
                Ok(conn) => conn,
                Err(err) => {
                    debug!("gelen bağlantı kurulamadı: {err:#}");
                    return;
                }
            };

            if alpn == proto::VOICE_ALPN {
                match voice {
                    Some(mesh) => mesh.accept(conn),
                    None => debug!("ses bağlantısı geldi ama ses açık değil"),
                }
                return;
            }

            let Some(shared) = control else {
                debug!("kontrol bağlantısı geldi ama koordinatör değiliz");
                return;
            };
            let peer = to_peer_id(conn.remote_id());
            if let Err(err) = serve_peer(shared.clone(), conn, peer).await {
                debug!("{} ile oturum bitti: {err:#}", peer.short());
            }
            // Bağlantı nasıl biterse bitsin (zarif ya da kopma) roster temizlenir.
            let _ = shared.apply(peer, ToCoordinator::Leave).await;
        });
    }
}

/// Adayı gerekçesiyle geri çevirir.
///
/// Gerekçenin karşı tarafa **ulaştığından emin olmak** için akış bitirilip okunması
/// beklenir; aksi halde bağlantı kapanırken mesaj yolda kaybolur ve kullanıcı
/// "parola yanlış" yerine anlamsız bir "bağlantı koptu" görür.
async fn reject(mut send: SendStream, reason: &str) -> Result<()> {
    write_msg(
        &mut send,
        &ToPeer::Rejected {
            reason: reason.to_string(),
        },
    )
    .await?;
    send.finish().ok();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), send.stopped()).await;
    Ok(())
}

async fn serve_peer(shared: Arc<Shared>, conn: Connection, peer: PeerId) -> Result<()> {
    let (mut send, mut recv) = conn.open_bi().await.context("kontrol akışı açılamadı")?;

    let nonce = auth::random_nonce();
    write_msg(&mut send, &ToPeer::Challenge { nonce }).await?;

    let hello: ToCoordinator = read_msg(&mut recv).await?;
    let ToCoordinator::Hello { name, proof } = hello else {
        bail!("el sıkışma yerine başka mesaj geldi");
    };

    if !auth::verify(&shared.password, &nonce, &proof) {
        warn!("{} yanlış parola ile denedi", peer.short());
        return reject(send, "oda parolası yanlış").await;
    }

    let (display_name, snapshot) = {
        let mut room = shared.room.lock().await;
        match room.join(peer, &name) {
            Ok(display_name) => (display_name, room.snapshot()),
            Err(err) => {
                let reason = err.to_string();
                drop(room);
                return reject(send, &reason).await;
            }
        }
    };

    write_msg(&mut send, &ToPeer::Welcome { you: peer, room: snapshot }).await?;

    let mut updates = shared.broadcast.subscribe();
    let _ = shared.broadcast.send(ToPeer::Notice {
        text: format!("{display_name} odaya katıldı"),
    });
    {
        let room = shared.room.lock().await;
        let _ = shared.broadcast.send(ToPeer::Roster { peers: room.roster() });
    }

    // Yayını bu peer'a taşıyan yazma tarafı.
    let writer = tokio::spawn(async move {
        loop {
            match updates.recv().await {
                Ok(message) => {
                    if write_msg(&mut send, &message).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // Geride kalan istemciye eksik mesaj göndermektense durumu
                    // yeniden kurdurmak daha güvenli.
                    debug!("{} {skipped} mesaj geride kaldı", peer.short());
                    let notice = ToPeer::Notice {
                        text: "bağlantınız yavaşladı, bazı mesajlar atlandı".into(),
                    };
                    if write_msg(&mut send, &notice).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });

    // Okuma tarafı: peer kapanana kadar komutları işle.
    let result = async {
        loop {
            let message: ToCoordinator = read_msg(&mut recv).await?;
            let leaving = matches!(message, ToCoordinator::Leave);
            if let Err(err) = shared.apply(peer, message).await {
                debug!("{} komutu reddedildi: {err:#}", peer.short());
            }
            if leaving {
                return Ok(());
            }
        }
    }
    .await;

    writer.abort();
    result
}

// ── Katılan istemci ────────────────────────────────────────────────────────────

pub struct Client;

impl Client {
    /// Davet kodundaki koordinatöre bağlanır ve el sıkışmayı tamamlar.
    ///
    /// Normal kullanımda hedef sadece bir kimliktir; adresi keşif servisi bulur.
    /// Testler tam `EndpointAddr` vererek keşfi atlar.
    pub async fn connect(
        endpoint: Endpoint,
        target: impl Into<EndpointAddr>,
        password: &str,
        name: &str,
        voice: Option<VoiceMesh>,
    ) -> Result<Session> {
        let target: EndpointAddr = target.into();
        let coordinator = to_peer_id(target.id);
        let conn = endpoint
            .connect(target, proto::ALPN)
            .await
            .context("odaya bağlanılamadı — kod yanlış olabilir ya da oda kapalı")?;

        let (mut send, mut recv) = conn.accept_bi().await.context("kontrol akışı kurulamadı")?;

        let challenge: ToPeer = read_msg(&mut recv).await?;
        let ToPeer::Challenge { nonce } = challenge else {
            bail!("beklenmeyen karşılama mesajı");
        };

        let proof = auth::proof(password, &nonce)?;
        write_msg(
            &mut send,
            &ToCoordinator::Hello {
                name: name.to_string(),
                proof,
            },
        )
        .await?;

        let (me, snapshot) = match read_msg::<ToPeer>(&mut recv).await? {
            ToPeer::Welcome { you, room } => (you, room),
            ToPeer::Rejected { reason } => bail!("odaya alınmadınız: {reason}"),
            _ => bail!("beklenmeyen yanıt"),
        };

        let (event_tx, event_rx) = mpsc::channel(EVENT_DEPTH);
        let (command_tx, command_rx) = mpsc::channel(EVENT_DEPTH);

        event_tx
            .send(Event::Welcome { me, room: snapshot })
            .await
            .ok();

        // Katılan taraf da ses bağlantısı karşılamalı: mesh iki yönlüdür.
        if let Some(mesh) = voice {
            super::voice::spawn_accept(endpoint.clone(), mesh);
        }
        tokio::spawn(client_reader(recv, event_tx.clone()));
        tokio::spawn(client_writer(send, command_rx, conn, endpoint, event_tx));

        Ok(Session {
            me,
            invite_code: invite::encode(&coordinator.0),
            commands: command_tx,
            events: event_rx,
        })
    }
}

async fn client_reader(mut recv: RecvStream, events: mpsc::Sender<Event>) {
    loop {
        match read_msg::<ToPeer>(&mut recv).await {
            Ok(message) => {
                if let Some(event) = wire_to_event(message)
                    && events.send(event).await.is_err()
                {
                    return;
                }
            }
            Err(_) => {
                // Kopmanın QUIC seviyesindeki gerekçesi kullanıcıya bir şey anlatmaz;
                // pratikte tek anlamı odanın kapanmış olmasıdır.
                let _ = events
                    .send(Event::Disconnected(
                        "odayla bağlantı kesildi — koordinatör çıkmış olabilir".into(),
                    ))
                    .await;
                return;
            }
        }
    }
}

async fn client_writer(
    mut send: SendStream,
    mut commands: mpsc::Receiver<Command>,
    conn: Connection,
    endpoint: Endpoint,
    events: mpsc::Sender<Event>,
) {
    while let Some(command) = commands.recv().await {
        let quitting = matches!(command, Command::Quit);
        let wire = into_wire(command);
        if write_msg(&mut send, &wire).await.is_err() {
            let _ = events
                .send(Event::Disconnected("koordinatöre ulaşılamıyor".into()))
                .await;
            return;
        }
        if quitting {
            let _ = send.finish();
            conn.close(0u32.into(), b"ayrildi");
            endpoint.close().await;
            let _ = events.send(Event::Disconnected("odadan ayrıldınız".into())).await;
            return;
        }
    }
}

// ── Dönüşümler ─────────────────────────────────────────────────────────────────

fn into_wire(command: Command) -> ToCoordinator {
    match command {
        Command::SwitchChannel(channel) => ToCoordinator::SwitchChannel { channel },
        Command::Chat { channel, text } => ToCoordinator::Chat { channel, text },
        Command::SetMuted(muted) => ToCoordinator::SetMuted { muted },
        Command::SetDeafened(deafened) => ToCoordinator::SetDeafened { deafened },
        Command::Quit => ToCoordinator::Leave,
    }
}

fn wire_to_event(message: ToPeer) -> Option<Event> {
    match message {
        ToPeer::Roster { peers } => Some(Event::Roster(peers)),
        ToPeer::Chat(line) => Some(Event::Chat(line)),
        ToPeer::Notice { text } => Some(Event::Notice(text)),
        ToPeer::Rejected { reason } => Some(Event::Disconnected(reason)),
        // Welcome ve Challenge yalnızca el sıkışmada geçerli.
        ToPeer::Welcome { .. } | ToPeer::Challenge { .. } => None,
    }
}

async fn pump_broadcast_to_ui(
    mut updates: broadcast::Receiver<ToPeer>,
    events: mpsc::Sender<Event>,
) {
    loop {
        match updates.recv().await {
            Ok(message) => {
                if let Some(event) = wire_to_event(message)
                    && events.send(event).await.is_err()
                {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}
