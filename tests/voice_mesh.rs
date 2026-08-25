//! End-to-end tests for the voice mesh.
//!
//! No audio hardware involved: we attach directly to the mesh's input and output ends
//! and measure that real QUIC datagrams reach the right peer through the right channel
//! filter.

use std::time::Duration;

use anyhow::{Result, bail};
use iroh::Endpoint;
use iroh::address_lookup::MemoryLookup;
use tincan::audio::Incoming;
use tincan::net::endpoint::{bind_offline_with_lookup, to_peer_id};
use tincan::net::voice::{VoiceMesh, spawn_accept};
use tincan::proto::{ChannelId, PeerId};
use tokio::sync::mpsc;

const PATIENCE: Duration = Duration::from_secs(10);

/// A mesh node for tests: the endpoint, the mesh and both of its ends.
struct Node {
    id: PeerId,
    endpoint: Endpoint,
    lookup: MemoryLookup,
    mesh: VoiceMesh,
    /// The frames that reach this node's audio engine.
    received: mpsc::Receiver<Incoming>,
    /// This node's "microphone": a frame written here is distributed to the mesh.
    microphone: mpsc::Sender<Vec<u8>>,
}

async fn node() -> Result<Node> {
    let (endpoint, lookup) = bind_offline_with_lookup().await?;
    let id = to_peer_id(endpoint.id());

    let (incoming_tx, received) = mpsc::channel(64);
    let (microphone, outgoing_rx) = mpsc::channel(64);

    let mesh = VoiceMesh::start(endpoint.clone(), id, incoming_tx, outgoing_rx);
    spawn_accept(endpoint.clone(), mesh.clone());

    Ok(Node {
        id,
        endpoint,
        lookup,
        mesh,
        received,
        microphone,
    })
}

/// Introduces the nodes to each other's addresses by hand, instead of via discovery.
fn introduce(a: &Node, b: &Node) {
    a.lookup.add_endpoint_info(b.endpoint.addr());
    b.lookup.add_endpoint_info(a.endpoint.addr());
}

async fn expect_frame(node: &mut Node) -> Result<Incoming> {
    match tokio::time::timeout(PATIENCE, node.received.recv()).await {
        Ok(Some(frame)) => Ok(frame),
        Ok(None) => bail!("the audio channel closed"),
        Err(_) => bail!("no audio frame arrived"),
    }
}

/// Datagram loss is normal, so we keep resending until the mesh is up.
async fn send_until_received(from: &Node, to: &mut Node, payload: &[u8]) -> Result<Incoming> {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if tokio::time::Instant::now() > deadline {
            bail!("the audio frame never arrived");
        }
        from.microphone.send(payload.to_vec()).await?;
        match tokio::time::timeout(Duration::from_millis(200), to.received.recv()).await {
            Ok(Some(frame)) => return Ok(frame),
            Ok(None) => bail!("the audio channel closed"),
            Err(_) => continue,
        }
    }
}

/// While two peers share a channel, audio must reach each of them directly.
#[tokio::test]
async fn voice_flows_directly_between_peers_in_the_same_channel() -> Result<()> {
    let mut a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    let channel = Some(ChannelId(0));
    a.mesh.set_membership(channel, vec![b.id]).await;
    b.mesh.set_membership(channel, vec![a.id]).await;

    let frame = send_until_received(&a, &mut b, b"merhaba-ses").await?;
    assert_eq!(frame.from, a.id, "the sender must be identified from the connection");
    assert_eq!(frame.payload, b"merhaba-ses");

    // The reverse direction must work too: the mesh is two-way.
    let back = send_until_received(&b, &mut a, b"cevap").await?;
    assert_eq!(back.from, b.id);
    assert_eq!(back.payload, b"cevap");
    Ok(())
}

/// Sequence numbers must increase — the jitter buffer relies on it.
#[tokio::test]
async fn sequence_numbers_increase() -> Result<()> {
    let a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    let channel = Some(ChannelId(0));
    a.mesh.set_membership(channel, vec![b.id]).await;
    b.mesh.set_membership(channel, vec![a.id]).await;

    let first = send_until_received(&a, &mut b, b"bir").await?;
    a.microphone.send(b"iki".to_vec()).await?;
    let second = expect_frame(&mut b).await?;

    assert!(
        second.seq > first.seq,
        "the sequence number must increase: {} → {}",
        first.seq,
        second.seq
    );
    Ok(())
}

/// A peer in a different channel must not be heard.
#[tokio::test]
async fn voice_does_not_leak_across_channels() -> Result<()> {
    let a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    // First meet in the same channel so the connection gets established.
    a.mesh.set_membership(Some(ChannelId(0)), vec![b.id]).await;
    b.mesh.set_membership(Some(ChannelId(0)), vec![a.id]).await;
    send_until_received(&a, &mut b, b"ayni-kanal").await?;

    // b moves to another channel; a is still talking in the old one.
    b.mesh.set_membership(Some(ChannelId(1)), vec![]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    for _ in 0..10 {
        a.microphone.send(b"unheard".to_vec()).await?;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Frames sent before the channel switch may still be in flight; the "unheard"
    // content sent after the switch must never show up.
    while let Ok(frame) = b.received.try_recv() {
        assert_ne!(
            frame.payload, b"unheard",
            "audio from another channel must not leak through"
        );
    }
    Ok(())
}

/// A peer that leaves voice entirely must stop receiving audio.
#[tokio::test]
async fn leaving_voice_stops_the_stream() -> Result<()> {
    let a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    a.mesh.set_membership(Some(ChannelId(0)), vec![b.id]).await;
    b.mesh.set_membership(Some(ChannelId(0)), vec![a.id]).await;
    send_until_received(&a, &mut b, b"once").await?;

    b.mesh.set_membership(None, vec![]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    while b.received.try_recv().is_ok() {}

    for _ in 0..10 {
        a.microphone.send(b"sonra".to_vec()).await?;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(
        b.received.try_recv().is_err(),
        "a peer that left voice must receive no stream"
    );
    Ok(())
}

/// Talking while in no voice channel must go nowhere — and must not crash.
#[tokio::test]
async fn speaking_outside_a_channel_is_a_no_op() -> Result<()> {
    let a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    // a is in no channel, b is listening.
    b.mesh.set_membership(Some(ChannelId(0)), vec![a.id]).await;
    for _ in 0..5 {
        a.microphone.send(b"bosluga".to_vec()).await?;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(b.received.try_recv().is_err(), "audio with no channel must go nowhere");
    Ok(())
}

/// The link quality report must classify an established connection correctly.
#[tokio::test]
async fn link_status_reports_direct_connections() -> Result<()> {
    let a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    assert_eq!(a.mesh.link_status().await.peers(), 0, "no connections to begin with");

    let channel = Some(ChannelId(0));
    a.mesh.set_membership(channel, vec![b.id]).await;
    b.mesh.set_membership(channel, vec![a.id]).await;
    send_until_received(&a, &mut b, b"kalite").await?;

    let status = a.mesh.link_status().await;
    assert_eq!(status.peers(), 1);
    assert_eq!(status.direct, 1, "on a local network the link must be direct");
    assert_eq!(status.relayed, 0, "no relay may be reported while relays are off");
    assert!(status.worst_rtt.is_some(), "the RTT must be measured");
    Ok(())
}

/// A three-person mesh: everyone must hear everyone, with no coordinator in between.
#[tokio::test]
async fn three_peers_form_a_full_mesh() -> Result<()> {
    let a = node().await?;
    let mut b = node().await?;
    let mut c = node().await?;
    introduce(&a, &b);
    introduce(&a, &c);
    introduce(&b, &c);

    let channel = Some(ChannelId(0));
    a.mesh.set_membership(channel, vec![b.id, c.id]).await;
    b.mesh.set_membership(channel, vec![a.id, c.id]).await;
    c.mesh.set_membership(channel, vec![a.id, b.id]).await;

    // a is talking: both b and c must hear it.
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let (mut heard_b, mut heard_c) = (false, false);
    while (!heard_b || !heard_c) && tokio::time::Instant::now() < deadline {
        a.microphone.send(b"herkese".to_vec()).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        while let Ok(frame) = b.received.try_recv() {
            heard_b |= frame.from == a.id;
        }
        while let Ok(frame) = c.received.try_recv() {
            heard_c |= frame.from == a.id;
        }
    }
    assert!(heard_b, "b must hear a");
    assert!(heard_c, "c must hear a");

    // b's audio must reach c directly too (neither of them is the coordinator).
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let mut heard = false;
    while !heard && tokio::time::Instant::now() < deadline {
        b.microphone.send(b"c-duyuyor-mu".to_vec()).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        while let Ok(frame) = c.received.try_recv() {
            heard |= frame.from == b.id;
        }
    }
    assert!(heard, "audio between joiners must flow directly");
    Ok(())
}
