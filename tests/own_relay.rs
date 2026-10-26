//! Rooms on a relay their host runs: a real iroh relay on localhost, endpoints with no
//! discovery at all, and nothing of n0's in between.

use std::time::Duration;

use anyhow::{Result, bail};
use iroh::tls::CaTlsConfig;
use iroh::{Endpoint, RelayUrl};
use tincan::auth::{Admission, RoomSecret};
use tincan::net::control::{Client, Coordinator};
use tincan::net::endpoint::{bind_on_relay, relay_builder, to_endpoint_id, to_peer_id};
use tincan::net::voice::{VoiceMesh, spawn_accept};
use tincan::net::{Event, Session};
use tincan::proto::ChannelId;
use tincan::room::Room;
use tokio::sync::mpsc;

const PATIENCE: Duration = Duration::from_secs(10);

/// An endpoint on the test relay. The relay's certificate is self-signed, so it is not
/// checked here; that is the only difference from `bind_on_relay`.
async fn on_relay(identity: Option<iroh::SecretKey>, relay: &RelayUrl) -> Result<Endpoint> {
    let endpoint = relay_builder(identity, relay)
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    tokio::time::timeout(PATIENCE, endpoint.online()).await?;
    Ok(endpoint)
}

async fn wait_for_roster(session: &mut Session, count: usize) -> Result<()> {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        match tokio::time::timeout_at(deadline, session.events.recv()).await {
            Ok(Some(Event::Roster(peers))) if peers.len() == count => return Ok(()),
            Ok(Some(Event::Disconnected(reason))) => bail!("disconnected: {reason}"),
            Ok(Some(_)) => {}
            Ok(None) => bail!("the event channel closed"),
            Err(_) => bail!("timed out waiting for a roster of {count}"),
        }
    }
}

#[tokio::test]
async fn a_named_room_on_a_relay_is_joined_by_id_alone() -> Result<()> {
    let (_map, relay, _server) = iroh::test_utils::run_relay_server().await?;
    let secret = RoomSecret::derive("lobby", "pass")?;

    let host_ep = on_relay(Some(secret.identity()), &relay).await?;
    let admission = Admission::room(&secret, "pass")?;
    let room = Room::new("lobby", vec!["general".into()])?;
    let mut host = Coordinator::spawn(host_ep, room, admission, "alice", None).await?;

    let guest_ep = on_relay(None, &relay).await?;
    let joiner = RoomSecret::derive("lobby", "pass")?;
    let _guest = Client::connect(
        guest_ep,
        to_endpoint_id(&joiner.coordinator())?,
        joiner.key(),
        "bob",
        None,
    )
    .await?;

    wait_for_roster(&mut host, 2).await
}

#[tokio::test]
async fn a_relay_endpoint_never_asks_n0() -> Result<()> {
    let (_map, relay, _server) = iroh::test_utils::run_relay_server().await?;
    let endpoint = on_relay(None, &relay).await?;
    let relays: Vec<RelayUrl> = endpoint.addr().relay_urls().cloned().collect();
    assert_eq!(relays, vec![relay]);
    endpoint.close().await;
    Ok(())
}

#[tokio::test]
async fn an_unreachable_relay_fails_in_bounded_time() -> Result<()> {
    let nowhere: RelayUrl = "https://127.0.0.1:9".parse()?;
    let started = std::time::Instant::now();
    let Err(err) = bind_on_relay(None, &nowhere).await else {
        bail!("a relay nobody runs must not count as reached");
    };
    assert!(started.elapsed() < Duration::from_secs(15));
    assert!(
        format!("{err:#}").contains("could not reach the relay"),
        "{err:#}"
    );
    Ok(())
}

/// The voice mesh dials peers by identity; on a relay of your own that has to resolve
/// through the relay, since there is no discovery service to ask.
#[tokio::test]
async fn voice_reaches_a_peer_through_the_shared_relay() -> Result<()> {
    let (_map, relay, _server) = iroh::test_utils::run_relay_server().await?;

    let a_ep = on_relay(None, &relay).await?;
    let b_ep = on_relay(None, &relay).await?;
    let (a_id, b_id) = (to_peer_id(a_ep.id()), to_peer_id(b_ep.id()));

    let (a_in, _a_received) = mpsc::channel(64);
    let (a_mic, a_out) = mpsc::channel::<Vec<u8>>(64);
    let a = VoiceMesh::start(a_ep.clone(), a_id, a_in, a_out);
    spawn_accept(a_ep, a.clone());

    let (b_in, mut b_received) = mpsc::channel(64);
    let (_b_mic, b_out) = mpsc::channel::<Vec<u8>>(64);
    let b = VoiceMesh::start(b_ep.clone(), b_id, b_in, b_out);
    spawn_accept(b_ep, b.clone());

    let channel = Some(ChannelId(0));
    a.set_membership(channel, vec![b_id]).await;
    b.set_membership(channel, vec![a_id]).await;

    // Datagrams can be lost while the link comes up, so keep speaking until one lands.
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        if tokio::time::Instant::now() > deadline {
            bail!("no audio crossed the relay");
        }
        a_mic.send(b"merhaba".to_vec()).await?;
        if let Ok(Some(frame)) =
            tokio::time::timeout(Duration::from_millis(200), b_received.recv()).await
        {
            assert_eq!(frame.from, a_id);
            assert_eq!(frame.payload, b"merhaba");
            return Ok(());
        }
    }
}

/// A wrong passphrase derives a key nobody holds, so on a relay it is a room nobody
/// answers for, the same as on n0's network.
#[tokio::test]
async fn a_wrong_passphrase_on_a_relay_looks_like_a_closed_room() -> Result<()> {
    let (_map, relay, _server) = iroh::test_utils::run_relay_server().await?;
    let secret = RoomSecret::derive("lobby", "pass")?;
    let host_ep = on_relay(Some(secret.identity()), &relay).await?;
    let room = Room::new("lobby", vec!["general".into()])?;
    let _host = Coordinator::spawn(
        host_ep,
        room,
        Admission::room(&secret, "pass")?,
        "alice",
        None,
    )
    .await?;

    let wrong = RoomSecret::derive("lobby", "wrong")?;
    let refused = Client::connect_patiently(
        on_relay(None, &relay).await?,
        to_endpoint_id(&wrong.coordinator())?,
        wrong.key(),
        "mallory",
        None,
        Duration::from_secs(3),
        |_| {},
    )
    .await;
    let Err(err) = refused else {
        bail!("a wrong passphrase must not reach the room");
    };
    assert!(
        format!("{err:#}").contains("could not reach the room"),
        "{err:#}"
    );
    Ok(())
}
