//! Rooms on a relay their host runs: a real iroh relay on localhost, endpoints with no
//! discovery at all, and nothing of n0's in between.

use std::time::Duration;

use anyhow::{Result, bail};
use iroh::tls::CaTlsConfig;
use iroh::{Endpoint, RelayUrl};
use tincan::auth::{Admission, RoomSecret};
use tincan::net::control::{Client, Coordinator};
use tincan::net::endpoint::{bind_on_relay, relay_builder, to_endpoint_id};
use tincan::net::{Event, Session};
use tincan::room::Room;

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
