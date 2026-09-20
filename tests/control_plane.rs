//! End-to-end tests for the control plane: two real iroh endpoints, a real QUIC
//! connection, a real handshake — but with no relays and no discovery, entirely local.

use std::time::Duration;

use anyhow::{Result, bail};
use iroh::EndpointAddr;
use tincan::auth::{Admission, Key, RoomSecret};
use tincan::net::control::{Client, Coordinator};
use tincan::net::endpoint::{bind, bind_offline, bind_offline_as, to_endpoint_id, to_peer_id};
use tincan::net::{Command, Event, Session};
use tincan::proto::{ChannelId, PeerInfo};
use tincan::room::Room;

/// The upper bound used when waiting for events, so tests cannot hang.
const PATIENCE: Duration = Duration::from_secs(10);

fn test_room() -> Room {
    Room::new("test room", vec!["general".into(), "gaming".into()]).unwrap()
}

/// What a coordinator at `addr` lets in, for a room reached by its invite code.
fn admits(addr: &EndpointAddr, password: &str) -> Admission {
    Admission::invite(password, &to_peer_id(addr.id)).unwrap()
}

/// The key a joiner holding the invite code for `addr` proves itself with.
fn key_for(addr: &EndpointAddr, password: &str) -> Key {
    Key::for_invite(password, &to_peer_id(addr.id)).unwrap()
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
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, "password"), "alice", None).await?;

    let welcome = wait_for(&mut host, "host welcome", |e| match e {
        Event::Welcome { room, .. } => Some(room),
        _ => None,
    })
    .await?;
    assert_eq!(welcome.peers.len(), 1, "the host must see itself in the room");
    assert_eq!(welcome.channels, vec!["general", "gaming"]);

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, "password"), "bob", None).await?;

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

/// A room opened by name: the joiner derives the host's address from the name and the
/// passphrase alone, and the invite code still gets in too.
#[tokio::test]
async fn a_room_opened_by_name_is_reached_by_name_and_by_code() -> Result<()> {
    let passphrase = "chestnut-ferry-lens-moss";
    let secret = RoomSecret::derive("lobby", passphrase)?;
    let host_ep = bind_offline_as(secret.identity()).await?;
    let host_addr = host_ep.addr();
    let admission = Admission::room(&secret, passphrase)?;
    let mut host = Coordinator::spawn(host_ep, test_room(), admission, "alice", None).await?;

    // Typed differently, derived on the other machine: the same address.
    let joiner = RoomSecret::derive("Lobby", "Chestnut Ferry Lens Moss")?;
    assert_eq!(to_peer_id(host_addr.id), joiner.coordinator());

    let mut by_name = Client::connect(bind_offline().await?, host_addr.clone(), joiner.key(), "bob", None).await?;
    wait_for(&mut by_name, "welcome by name", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let by_code_key = key_for(&host_addr, passphrase);
    let mut by_code = Client::connect(bind_offline().await?, host_addr.clone(), &by_code_key, "carol", None).await?;
    wait_for(&mut by_code, "welcome by code", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    wait_for_roster(&mut host, 3).await?;

    let wrong = RoomSecret::derive("lobby", "chestnut-ferry-lens-mess")?;
    assert_ne!(wrong.coordinator(), secret.coordinator(), "a wrong passphrase is a different address");
    let refused = Client::connect(bind_offline().await?, host_addr, wrong.key(), "mallory", None).await;
    let err = refused.err().expect("a wrong passphrase must not be accepted").to_string();
    assert!(err.contains("password"), "the error must point at the password: {err}");
    Ok(())
}

/// Over the real network: a room opened by name is found from the name and passphrase
/// alone, through the address records its derived key publishes.
#[tokio::test]
#[ignore = "needs the internet and n0's discovery servers"]
async fn a_named_room_is_found_over_the_network() -> Result<()> {
    // A made-up name, so the test never lands in somebody's real room.
    let room = format!("test-{}", tincan::passphrase::generate());
    let passphrase = tincan::passphrase::generate();

    let secret = RoomSecret::derive(&room, &passphrase)?;
    let admission = Admission::room(&secret, &passphrase)?;
    let mut host = Coordinator::spawn(bind(Some(secret.identity())).await?, test_room(), admission, "alice", None).await?;

    let joiner = RoomSecret::derive(&room, &passphrase)?;
    let target = to_endpoint_id(&joiner.coordinator())?;
    // Publishing the host's addresses takes a moment; until then the lookup finds nothing.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut guest = loop {
        match Client::connect(bind(None).await?, target, joiner.key(), "bob", None).await {
            Ok(session) => break session,
            Err(err) if tokio::time::Instant::now() < deadline => {
                eprintln!("not found yet, retrying: {err:#}");
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(err) => return Err(err),
        }
    };
    wait_for(&mut guest, "welcome by name", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;
    Ok(())
}

/// An attempt with the wrong password must be rejected during the handshake.
#[tokio::test]
async fn wrong_password_is_refused() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let _host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, "right-password"), "alice", None).await?;

    let guest_ep = bind_offline().await?;
    let result = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, "wrong-password"), "uninvited", None).await;

    let err = result.err().expect("a wrong password must not be accepted").to_string();
    assert!(err.contains("password"), "the error must point at the password: {err}");
    Ok(())
}

/// A chat message must reach the sender and everyone else in the same shape.
#[tokio::test]
async fn chat_reaches_everyone_including_the_sender() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, ""), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, ""), "bob", None).await?;
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
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, ""), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, ""), "bob", None).await?;
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
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, ""), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, ""), "bob", None).await?;
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
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, ""), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, ""), "bob", None).await?;
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
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, ""), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, ""), "alice", None).await?;
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
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, ""), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let mut first = Client::connect(bind_offline().await?, host_addr.clone(), &key_for(&host_addr, ""), "bob", None).await?;
    wait_for(&mut first, "welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    let mut second = Client::connect(bind_offline().await?, host_addr.clone(), &key_for(&host_addr, ""), "carol", None).await?;
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

/// When the coordinator leaves, clients receive a disconnection notice and
/// their endpoints close gracefully without dropping unclosed.
#[tokio::test]
async fn host_quitting_notifies_and_gracefully_disconnects_guest() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, ""), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, ""), "bob", None).await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    // Host quits.
    host.commands.send(Command::Quit).await?;

    // Guest should receive the notice and disconnection.
    let deadline = tokio::time::Instant::now() + PATIENCE;
    let mut saw_notice = false;
    let mut saw_disconnect = false;
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, guest.events.recv()).await {
        match event {
            Event::Notice(text) if text.contains("the coordinator left") => saw_notice = true,
            Event::Disconnected(_) => {
                saw_disconnect = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_notice, "guest should have received the closing notice");
    assert!(saw_disconnect, "guest should have received Event::Disconnected");

    // Dropping guest's commands should close client_writer and terminate cleanly.
    drop(guest.commands);
    tokio::time::sleep(Duration::from_millis(100)).await;
    Ok(())
}

/// Setting AFK status must broadcast the updated roster to all peers in the room.
#[tokio::test]
async fn afk_status_is_visible_to_everyone() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), admits(&host_addr, ""), "alice", None).await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr.clone(), &key_for(&host_addr, ""), "bob", None).await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    // Guest marks itself as AFK.
    guest.commands.send(Command::SetAfk(true)).await?;

    // Host should see Bob as AFK in the roster.
    wait_for(&mut host, "bob marked afk in host roster", |e| match e {
        Event::Roster(peers) => {
            let bob = peers.iter().find(|p| p.name == "bob")?;
            bob.afk.then_some(())
        }
        _ => None,
    })
    .await?;

    // Guest clears AFK.
    guest.commands.send(Command::SetAfk(false)).await?;

    // Host should see Bob as no longer AFK.
    wait_for(&mut host, "bob un-afk in host roster", |e| match e {
        Event::Roster(peers) => {
            let bob = peers.iter().find(|p| p.name == "bob")?;
            (!bob.afk).then_some(())
        }
        _ => None,
    })
    .await?;

    Ok(())
}

