//! The control plane: the coordinator server and the joining client.
//!
//! The crucial point of the design: **the host's own actions go through the same
//! function as actions arriving from the network** (`apply`). With a shortcut just for
//! the host, the room the host sees and the room everyone else sees would drift apart.
//!
//! The flow:
//! ```text
//! coordinator                            joiner
//!     │── Challenge{nonce} ───────────────▶│
//!     │◀── Hello{name, Argon2id(password)}─│
//!     │── Welcome{you, room} ─────────────▶│   (or Rejected)
//!     │── Roster / Chat / Notice ─────────▶│   (broadcast)
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

/// Depth of the broadcast channel. A slow client that falls this far behind is
/// resynchronised with a full roster to keep it consistent.
const BROADCAST_DEPTH: usize = 512;
/// The interface event queue.
const EVENT_DEPTH: usize = 256;

// ── Framing ─────────────────────────────────────────────────────────────────────

async fn write_msg<T: Serialize>(stream: &mut SendStream, message: &T) -> Result<()> {
    let framed = proto::encode(message)?;
    stream.write_all(&framed).await.context("could not write to the stream")?;
    Ok(())
}

async fn read_msg<T: DeserializeOwned>(stream: &mut RecvStream) -> Result<T> {
    let mut header = [0u8; 4];
    stream.read_exact(&mut header).await.context("the stream closed")?;
    let len = u32::from_le_bytes(header) as usize;
    if len > MAX_MESSAGE_BYTES {
        bail!("the other side announced a {len} byte message — over the limit");
    }
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await.context("the message was cut short")?;
    proto::decode(&body)
}

// ── Coordinator ────────────────────────────────────────────────────────────────

pub(crate) struct Shared {
    room: Mutex<Room>,
    /// The broadcast that reaches every connected peer **and** the host's own
    /// interface.
    broadcast: broadcast::Sender<ToPeer>,
    password: String,
}

impl Shared {
    /// The single point of truth: every action is applied to the room here and the
    /// result is broadcast.
    async fn apply(&self, from: PeerId, message: ToCoordinator) -> Result<()> {
        let mut room = self.room.lock().await;
        match message {
            ToCoordinator::Hello { .. } => bail!("the handshake is already complete"),

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
                        text: format!("{} left the room", peer.name),
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
    /// Opens the room and starts accepting incoming connections.
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
        room.join(me, host_name).context("the host nickname is invalid")?;

        let (broadcast_tx, _) = broadcast::channel(BROADCAST_DEPTH);
        let shared = Arc::new(Shared {
            room: Mutex::new(room),
            broadcast: broadcast_tx,
            password,
        });

        let (event_tx, event_rx) = mpsc::channel(EVENT_DEPTH);
        let (command_tx, command_rx) = mpsc::channel(EVENT_DEPTH);

        // The host's interface listens to the same broadcast as everyone else.
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

/// Handles the host's own commands through the same path as network ones.
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
                text: "the room is closing — the coordinator left".into(),
            });
            // A short breath so the broadcast reaches the clients.
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            endpoint.close().await;
            let _ = events.send(Event::Disconnected("the room was closed".into())).await;
            return;
        }
        if let Err(err) = shared.apply(me, into_wire(command)).await {
            // The host's own error is not broadcast; it lands on their screen only.
            let _ = events.send(Event::Notice(format!("that did not work: {err}"))).await;
        }
    }
}

/// Routes incoming connections by their ALPN.
///
/// The same endpoint accepts both control and voice connections. On peers that are not
/// the coordinator `control` is empty — they only answer voice connections.
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
                    debug!("could not accept an incoming connection: {err:#}");
                    return;
                }
            };
            let alpn = match accepting.alpn().await {
                Ok(alpn) => alpn,
                Err(err) => {
                    debug!("could not read the ALPN: {err:#}");
                    return;
                }
            };
            let conn = match accepting.await {
                Ok(conn) => conn,
                Err(err) => {
                    debug!("an incoming connection failed to establish: {err:#}");
                    return;
                }
            };

            if alpn == proto::VOICE_ALPN {
                match voice {
                    Some(mesh) => mesh.accept(conn),
                    None => debug!("a voice connection arrived but audio is not enabled"),
                }
                return;
            }

            let Some(shared) = control else {
                debug!("a control connection arrived but we are not the coordinator");
                return;
            };
            let peer = to_peer_id(conn.remote_id());
            if let Err(err) = serve_peer(shared.clone(), conn, peer).await {
                debug!("{} ile oturum bitti: {err:#}", peer.short());
            }
            // However the connection ends (gracefully or not), the roster is cleaned.
            let _ = shared.apply(peer, ToCoordinator::Leave).await;
        });
    }
}

/// Turns an applicant away, with a reason.
///
/// To **make sure the reason arrives**, the stream is finished and we wait for it to be
/// read; otherwise the message is lost as the connection closes and the user sees a
/// meaningless "connection dropped" instead of "wrong password".
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
    let (mut send, mut recv) = conn.open_bi().await.context("could not open the control stream")?;

    let nonce = auth::random_nonce();
    write_msg(&mut send, &ToPeer::Challenge { nonce }).await?;

    let hello: ToCoordinator = read_msg(&mut recv).await?;
    let ToCoordinator::Hello { name, proof } = hello else {
        bail!("a different message arrived instead of the handshake");
    };

    if !auth::verify(&shared.password, &nonce, &proof) {
        warn!("{} tried with the wrong password", peer.short());
        return reject(send, "wrong room password").await;
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
        text: format!("{display_name} joined the room"),
    });
    {
        let room = shared.room.lock().await;
        let _ = shared.broadcast.send(ToPeer::Roster { peers: room.roster() });
    }

    // The write side that carries the broadcast to this peer.
    let writer = tokio::spawn(async move {
        loop {
            match updates.recv().await {
                Ok(message) => {
                    if write_msg(&mut send, &message).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // Safer to make a lagging client rebuild its state than to send
                    // it an incomplete stream of messages.
                    debug!("{} fell {skipped} messages behind", peer.short());
                    let notice = ToPeer::Notice {
                        text: "your connection slowed down, some messages were skipped".into(),
                    };
                    if write_msg(&mut send, &notice).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });

    // The read side: handle commands until the peer goes away.
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

// ── Joining client ─────────────────────────────────────────────────────────────

pub struct Client;

impl Client {
    /// Connects to the coordinator named by the invite code and completes the
    /// handshake.
    ///
    /// In normal use the target is just an identity and discovery finds its address.
    /// Tests skip discovery by passing a full `EndpointAddr`.
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
            .context("could not connect to the room — the code may be wrong, or the room closed")?;

        let (mut send, mut recv) = conn.accept_bi().await.context("could not establish the control stream")?;

        let challenge: ToPeer = read_msg(&mut recv).await?;
        let ToPeer::Challenge { nonce } = challenge else {
            bail!("unexpected greeting message");
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
            ToPeer::Rejected { reason } => bail!("you were not let into the room: {reason}"),
            _ => bail!("unexpected reply"),
        };

        let (event_tx, event_rx) = mpsc::channel(EVENT_DEPTH);
        let (command_tx, command_rx) = mpsc::channel(EVENT_DEPTH);

        event_tx
            .send(Event::Welcome { me, room: snapshot })
            .await
            .ok();

        // The joining side must answer voice connections too: the mesh is two-way.
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
                // The QUIC-level reason for a drop tells the user nothing; in
                // practice it only ever means the room has closed.
                let _ = events
                    .send(Event::Disconnected(
                        "lost contact with the room — the coordinator may have left".into(),
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
                .send(Event::Disconnected("cannot reach the coordinator".into()))
                .await;
            return;
        }
        if quitting {
            let _ = send.finish();
            conn.close(0u32.into(), b"ayrildi");
            endpoint.close().await;
            let _ = events.send(Event::Disconnected("you left the room".into())).await;
            return;
        }
    }
}

// ── Conversions ────────────────────────────────────────────────────────────────

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
        // Welcome and Challenge are only meaningful during the handshake.
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
