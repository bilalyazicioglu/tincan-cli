//! Ses mesh'inin uçtan uca testleri.
//!
//! Ses donanımı yok: mesh'in giriş/çıkış uçlarına doğrudan bağlanıp gerçek QUIC
//! datagramlarının doğru peer'a, doğru kanal filtresiyle ulaştığını ölçüyoruz.

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

/// Test için bir mesh düğümü: endpoint, mesh ve iki ucu.
struct Node {
    id: PeerId,
    endpoint: Endpoint,
    lookup: MemoryLookup,
    mesh: VoiceMesh,
    /// Bu düğümün ses motoruna ulaşan çerçeveler.
    received: mpsc::Receiver<Incoming>,
    /// Bu düğümün "mikrofonu": buraya yazılan çerçeve mesh'e dağıtılır.
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

/// Keşif servisi yerine düğümlere birbirlerinin adresini elle tanıtır.
fn introduce(a: &Node, b: &Node) {
    a.lookup.add_endpoint_info(b.endpoint.addr());
    b.lookup.add_endpoint_info(a.endpoint.addr());
}

async fn expect_frame(node: &mut Node) -> Result<Incoming> {
    match tokio::time::timeout(PATIENCE, node.received.recv()).await {
        Ok(Some(frame)) => Ok(frame),
        Ok(None) => bail!("ses kanalı kapandı"),
        Err(_) => bail!("ses çerçevesi gelmedi"),
    }
}

/// Datagram kaybı normaldir; mesh kurulana kadar tekrar tekrar gönderiyoruz.
async fn send_until_received(from: &Node, to: &mut Node, payload: &[u8]) -> Result<Incoming> {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if tokio::time::Instant::now() > deadline {
            bail!("ses çerçevesi hiç ulaşmadı");
        }
        from.microphone.send(payload.to_vec()).await?;
        match tokio::time::timeout(Duration::from_millis(200), to.received.recv()).await {
            Ok(Some(frame)) => return Ok(frame),
            Ok(None) => bail!("ses kanalı kapandı"),
            Err(_) => continue,
        }
    }
}

/// İki peer aynı kanaldayken ses doğrudan birbirlerine ulaşmalı.
#[tokio::test]
async fn voice_flows_directly_between_peers_in_the_same_channel() -> Result<()> {
    let mut a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    let channel = Some(ChannelId(0));
    a.mesh.set_membership(channel, vec![b.id]).await;
    b.mesh.set_membership(channel, vec![a.id]).await;

    let frame = send_until_received(&a, &mut b, b"merhaba-ses").await?;
    assert_eq!(frame.from, a.id, "gönderen bağlantıdan tanınmalı");
    assert_eq!(frame.payload, b"merhaba-ses");

    // Ters yön de çalışmalı: mesh çift yönlüdür.
    let back = send_until_received(&b, &mut a, b"cevap").await?;
    assert_eq!(back.from, b.id);
    assert_eq!(back.payload, b"cevap");
    Ok(())
}

/// Sıra numaraları artmalı — jitter tamponu buna dayanıyor.
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
        "sıra numarası artmalı: {} → {}",
        first.seq,
        second.seq
    );
    Ok(())
}

/// Farklı kanaldaki peer'ın sesi duyulmamalı.
#[tokio::test]
async fn voice_does_not_leak_across_channels() -> Result<()> {
    let a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    // Önce aynı kanalda buluşup bağlantıyı kuruyoruz.
    a.mesh.set_membership(Some(ChannelId(0)), vec![b.id]).await;
    b.mesh.set_membership(Some(ChannelId(0)), vec![a.id]).await;
    send_until_received(&a, &mut b, b"ayni-kanal").await?;

    // b başka kanala geçiyor; a hâlâ eski kanalda konuşuyor.
    b.mesh.set_membership(Some(ChannelId(1)), vec![]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    for _ in 0..10 {
        a.microphone.send(b"duyulmamali".to_vec()).await?;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Kanal değişiminden önce yolda olan çerçeveler olabilir; yeni kanaldan
    // sonra gelen "duyulmamali" içeriği hiç görülmemeli.
    while let Ok(frame) = b.received.try_recv() {
        assert_ne!(
            frame.payload, b"duyulmamali",
            "başka kanaldaki ses sızmamalı"
        );
    }
    Ok(())
}

/// Ses kanalından tamamen çıkan peer artık ses almamalı.
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
        "sesten çıkan peer akış almamalı"
    );
    Ok(())
}

/// Ses kanalında değilken konuşmak bir yere gitmemeli — ve çökmemeli.
#[tokio::test]
async fn speaking_outside_a_channel_is_a_no_op() -> Result<()> {
    let a = node().await?;
    let mut b = node().await?;
    introduce(&a, &b);

    // a hiçbir kanalda değil, b dinliyor.
    b.mesh.set_membership(Some(ChannelId(0)), vec![a.id]).await;
    for _ in 0..5 {
        a.microphone.send(b"bosluga".to_vec()).await?;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(b.received.try_recv().is_err(), "kanalsız ses gitmemeli");
    Ok(())
}

/// Üç kişilik mesh: herkes herkesi duymalı, koordinatör aracılık etmeden.
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

    // a konuşuyor: hem b hem c duymalı.
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
    assert!(heard_b, "b, a'yı duymalı");
    assert!(heard_c, "c, a'yı duymalı");

    // b'nin sesi de c'ye doğrudan gitmeli (ikisi de koordinatör değil).
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let mut heard = false;
    while !heard && tokio::time::Instant::now() < deadline {
        b.microphone.send(b"c-duyuyor-mu".to_vec()).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
        while let Ok(frame) = c.received.try_recv() {
            heard |= frame.from == b.id;
        }
    }
    assert!(heard, "katılanlar arası ses doğrudan akmalı");
    Ok(())
}
