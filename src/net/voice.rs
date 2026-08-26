//! The voice mesh: direct datagram transport between peers in the same channel.
//!
//! It runs entirely apart from the control plane. The coordinator says who is in which
//! channel; the peers handle the connections and the voice packets among themselves.
//! Voice traffic never passes through the coordinator.
//!
//! Two subtleties:
//!
//! * **Duplicate connections.** If two peers try to connect to each other at the same
//!   moment, two separate connections form and the audio is heard twice. The rule: the
//!   lower identity dials, the higher one waits. Since identities are public keys, this
//!   is a deterministic division of labour that needs no negotiation.
//! * **Channel filter.** An incoming packet whose channel does not match ours is not
//!   played. This is what stops audio leaking in from the wrong channel during the
//!   brief gap between a channel switch and the roster update.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

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
    /// Where decoded frames go (the audio engine).
    incoming: mpsc::Sender<Incoming>,
    /// The established voice connections.
    connections: Mutex<HashMap<PeerId, Connection>>,
    /// Our own voice channel; written into outgoing headers and used to filter
    /// incoming ones.
    channel: Mutex<Option<ChannelId>>,
    /// Outgoing frame counter.
    seq: AtomicU32,
}

/// The current state of the voice connections — feeds the quality indicator in the
/// interface.
///
/// The only thing a user wants to know is "is my voice getting through cleanly", and
/// that has two parts: whether it goes direct or detours through a relay, and latency.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinkStatus {
    /// Peers reached over a direct P2P connection.
    pub direct: usize,
    /// Peers whose audio flows through a relay (hole punching did not succeed).
    pub relayed: usize,
    /// The worst round-trip time across the connections.
    pub worst_rtt: Option<Duration>,
}

impl LinkStatus {
    pub fn peers(&self) -> usize {
        self.direct + self.relayed
    }
}

/// The voice mesh's public face.
#[derive(Clone)]
pub struct VoiceMesh {
    shared: Arc<Shared>,
    endpoint: Endpoint,
}

impl VoiceMesh {
    /// Builds the mesh and starts the loop that distributes outgoing voice frames.
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
                    continue; // Not in a voice channel; there is nowhere to speak.
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
                    // Datagram loss is normal for audio; failures pass quietly.
                    if let Err(err) = conn.send_datagram(frame.clone()) {
                        debug!("could not send audio to peer {}: {err}", peer.short());
                    }
                }
            }
        });

        Self { shared, endpoint }
    }

    /// Adds an incoming voice connection to the mesh.
    pub fn accept(&self, conn: Connection) {
        let peer = to_peer_id(conn.remote_id());
        let shared = self.shared.clone();
        tokio::spawn(async move {
            debug!("voice connection established with {} (incoming)", peer.short());
            shared.connections.lock().await.insert(peer, conn.clone());
            read_loop(shared.clone(), conn, peer).await;
            shared.connections.lock().await.remove(&peer);
        });
    }

    /// Updates the mesh from the coordinator's roster: connects to the peers in our
    /// channel and closes connections to those who left it.
    pub async fn set_membership(&self, channel: Option<ChannelId>, mut members: Vec<PeerId>) {
        *self.shared.channel.lock().await = channel;
        members.retain(|peer| *peer != self.shared.me);
        let wanted: HashSet<PeerId> = if channel.is_some() {
            members.into_iter().collect()
        } else {
            HashSet::new()
        };

        // Drop everyone who is no longer in the same channel.
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
            // Only the lower identity dials; the other side waits.
            if self.shared.me.0 > peer.0 {
                continue;
            }
            self.dial(*peer);
        }
    }

    /// Summarises the quality of the established voice connections.
    pub async fn link_status(&self) -> LinkStatus {
        let connections = self.shared.connections.lock().await;
        let mut status = LinkStatus::default();

        for conn in connections.values() {
            let paths = conn.paths();
            // Several paths can be open at once; the selected one carries the traffic.
            let Some(selected) = paths.iter().find(|path| path.is_selected()) else {
                continue;
            };
            if selected.is_relay() {
                status.relayed += 1;
            } else {
                status.direct += 1;
            }
            let rtt = selected.rtt();
            status.worst_rtt = Some(status.worst_rtt.map_or(rtt, |worst| worst.max(rtt)));
        }
        status
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
                    debug!("voice connection established with {} (outgoing)", peer.short());
                    shared.connections.lock().await.insert(peer, conn.clone());
                    read_loop(shared.clone(), conn, peer).await;
                    shared.connections.lock().await.remove(&peer);
                }
                Err(err) => debug!("could not open a voice connection to peer {}: {err}", peer.short()),
            }
        });
    }
}

/// The loop that answers incoming voice connections on peers that are not the
/// coordinator.
///
/// On the coordinator the control plane's accept loop does this instead: two loops
/// cannot listen on a single endpoint.
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
                    debug!("a non-voice connection arrived, ignoring it");
                    return;
                }
                match accepting.await {
                    Ok(conn) => mesh.accept(conn),
                    Err(err) => debug!("a voice connection failed to establish: {err:#}"),
                }
            });
        }
    });
}

/// Reads voice datagrams from one peer and passes them to the audio engine.
async fn read_loop(shared: Arc<Shared>, conn: Connection, peer: PeerId) {
    while let Ok(datagram) = conn.read_datagram().await {
        let Some((header, payload)) = VoiceHeader::parse(&datagram) else {
            continue; // Corrupt packet: drop it, do not tear the connection down.
        };

        // If it is not from our channel, it is not played.
        if *shared.channel.lock().await != Some(header.channel) {
            continue;
        }

        let frame = Incoming {
            from: peer,
            seq: header.seq,
            payload: payload.to_vec(),
        };
        // If the audio engine cannot keep up, dropping the frame beats queueing it
        // and growing the latency.
        if shared.incoming.try_send(frame).is_err() {
            debug!("the audio queue is full, dropped a frame");
        }
    }
}
