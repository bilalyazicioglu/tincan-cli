//! End-to-end tests for the control plane: two real iroh endpoints, a real QUIC
//! connection, a real handshake — but with no relays and no discovery, entirely local.

use std::time::Duration;

use anyhow::{Result, bail};
use tincan::net::control::{Client, Coordinator};
use tincan::net::endpoint::bind_offline;
use tincan::net::{Command, Event, Session};
use tincan::proto::{ChannelId, PeerInfo};
use tincan::room::Room;

/// The upper bound used when waiting for events, so tests cannot hang.
const PATIENCE: Duration = Duration::from_secs(10);

fn test_room() -> Room {
    Room::new("test room", vec!["general".into(), "gaming".into()]).unwrap()
}

/// Waits for the first event matching a predicate, swallowing the others on the way.
async fn wait_for<T>(
    session: &mut Session,
    what: &str,
    mut matcher: impl FnMut(Event) -> Option<T>,
) -> Result<T> {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let event = match tokio::time::timeout_at(deadline, session.events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => bail!("the event channel closed while waiting for: {what}"),
            Err(_) => bail!("timed out waiting for: {what}"),
        };
        if let Event::Disconnected(reason) = &event {
            bail!("beklenmedik kopma ({reason}), beklenen: {what}");
        }
        if let Some(found) = matcher(event) {
            return Ok(found);
        }
    }
}

async fn wait_for_roster(session: &mut Session, count: usize) -> Result<Vec<PeerInfo>> {
    wait_for(session, &format!("a roster of {count}"), |event| match event {
        Event::Roster(peers) if peers.len() == count => Some(peers),
        _ => None,
    })
    .await
}

async fn wait_for_chat(session: &mut Session, text: &str) -> Result<()> {
    let text = text.to_string();
    wait_for(session, &format!("chat: {text}"), move |event| match event {
        Event::Chat(line) if line.text == text => Some(()),
        _ => None,
    })
    .await
}

/// The host opens a room and a guest connects: both sides must see the same room.
#[tokio::test]
async fn peer_joins_and_both_sides_converge() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), "password".into(), "alice", None).await?;

    let welcome = wait_for(&mut host, "host welcome", |e| match e {
        Event::Welcome { room, .. } => Some(room),
        _ => None,
    })
    .await?;
    assert_eq!(welcome.peers.len(), 1, "the host must see itself in the room");
    assert_eq!(welcome.channels, vec!["general", "gaming"]);

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "password", "bob", None).await?;

    let guest_welcome = wait_for(&mut guest, "guest welcome", |e| match e {
        Event::Welcome { room, .. } => Some(room),
        _ => None,
    })
    .await?;
    assert_eq!(guest_welcome.room_name, "test room");
    assert_eq!(
        guest_welcome.peers.len(),
        2,
        "the joiner must see everyone, itself included"
    );

    // The host side must see the newcomer too.
    let roster = wait_for_roster(&mut host, 2).await?;
    let names: Vec<&str> = roster.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"alice") && names.contains(&"bob"), "{names:?}");

    assert_ne!(host.me, guest.me, "the identities must differ");
    assert_eq!(host.invite_code, guest.invite_code, "the same room means the same code");
    Ok(())
}

/// An attempt with the wrong password must be rejected during the handshake.
#[tokio::test]
async fn wrong_password_is_refused() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let _host = Coordinator::spawn(host_ep, test_room(), "right-password".into(), "alice", None).await?;

    let guest_ep = bind_offline().await?;
    let result = Client::connect(guest_ep, host_addr, "wrong-password", "uninvited", None).await;

    let err = result.err().expect("a wrong password must not be accepted").to_string();
    assert!(err.contains("password"), "the error must point at the password: {err}");
    Ok(())
}

/// A chat message must reach the sender and everyone else in the same shape.
#[tokio::test]
async fn chat_reaches_everyone_including_the_sender() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "bob", None).await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    // From the joiner to the host.
    guest
        .commands
        .send(Command::Chat {
            channel: ChannelId(0),
            text: "merhaba herkese".into(),
        })
        .await?;
    wait_for_chat(&mut host, "merhaba herkese").await?;
    wait_for_chat(&mut guest, "merhaba herkese").await?;

    // From the host to the joiner — the host's own message takes the same path.
    host.commands
        .send(Command::Chat {
            channel: ChannelId(1),
            text: "welcome aboard".into(),
        })
        .await?;
    wait_for_chat(&mut guest, "welcome aboard").await?;
    Ok(())
}

/// A channel switch must show up in everyone's roster — this is what drives the
/// voice mesh.
#[tokio::test]
async fn channel_switch_is_visible_to_everyone() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "bob", None).await?;
    let guest_id = guest.me;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    guest
        .commands
        .send(Command::SwitchChannel(Some(ChannelId(1))))
        .await?;

    let in_channel = |peers: &[PeerInfo]| {
        peers
            .iter()
            .any(|p| p.id == guest_id && p.channel == Some(ChannelId(1)))
    };

    let host_view = wait_for(&mut host, "the channel switch in the host roster", |e| match e {
        Event::Roster(peers) if in_channel(&peers) => Some(peers),
        _ => None,
    })
    .await?;
    assert_eq!(host_view.len(), 2);

    wait_for(&mut guest, "the channel switch in the guest roster", |e| match e {
        Event::Roster(peers) if in_channel(&peers) => Some(()),
        _ => None,
    })
    .await?;
    Ok(())
}

/// Asking to switch to a channel that does not exist must neither corrupt the room
/// nor drop the connection.
#[tokio::test]
async fn invalid_channel_request_is_ignored_without_breaking_the_session() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "bob", None).await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    guest
        .commands
        .send(Command::SwitchChannel(Some(ChannelId(99))))
        .await?;

    // The session must survive: a chat sent afterwards must still work.
    guest
        .commands
        .send(Command::Chat {
            channel: ChannelId(0),
            text: "still here".into(),
        })
        .await?;
    wait_for_chat(&mut host, "still here").await?;
    Ok(())
}

/// When a joiner leaves it must drop out of the roster.
#[tokio::test]
async fn leaving_updates_the_roster() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "bob", None).await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    guest.commands.send(Command::Quit).await?;

    let roster = wait_for_roster(&mut host, 1).await?;
    assert_eq!(roster[0].name, "alice", "only the host may remain");
    Ok(())
}

/// A second person arriving under the same nickname must be disambiguated, not
/// turned away.
#[tokio::test]
async fn duplicate_nicknames_are_disambiguated_over_the_wire() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "alice", None).await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let roster = wait_for_roster(&mut host, 2).await?;
    let names: Vec<&str> = roster.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1], "the two 'alice's must be distinguishable: {names:?}");
    Ok(())
}

/// Three people: the coordinator must also relay messages between the joiners.
#[tokio::test]
async fn three_participants_stay_in_sync() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let mut first = Client::connect(bind_offline().await?, host_addr.clone(), "", "bob", None).await?;
    wait_for(&mut first, "welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    let mut second = Client::connect(bind_offline().await?, host_addr, "", "carol", None).await?;
    wait_for(&mut second, "welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 3).await?;

    // A message from one joiner must reach the other through the coordinator.
    first
        .commands
        .send(Command::Chat {
            channel: ChannelId(0),
            text: "carol can you hear me".into(),
        })
        .await?;
    wait_for_chat(&mut second, "carol can you hear me").await?;
    wait_for_chat(&mut host, "carol can you hear me").await?;
    Ok(())
}
