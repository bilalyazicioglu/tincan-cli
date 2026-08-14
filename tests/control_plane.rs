//! Kontrol düzleminin uçtan uca testleri: iki gerçek iroh endpoint'i, gerçek QUIC
//! bağlantısı, gerçek el sıkışma — ama relay ve keşif olmadan, tamamen yerel.

use std::time::Duration;

use anyhow::{Result, bail};
use tincan::net::control::{Client, Coordinator};
use tincan::net::endpoint::bind_offline;
use tincan::net::{Command, Event, Session};
use tincan::proto::{ChannelId, PeerInfo};
use tincan::room::Room;

/// Olayları beklerken kullanılan üst sınır — testler asılı kalmasın.
const PATIENCE: Duration = Duration::from_secs(10);

fn test_room() -> Room {
    Room::new("test odası", vec!["genel".into(), "oyun".into()]).unwrap()
}

/// Belirtilen koşulu sağlayan ilk olayı bekler, yolda gelen diğerlerini yutar.
async fn wait_for<T>(
    session: &mut Session,
    what: &str,
    mut matcher: impl FnMut(Event) -> Option<T>,
) -> Result<T> {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let event = match tokio::time::timeout_at(deadline, session.events.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => bail!("olay kanalı kapandı, beklenen: {what}"),
            Err(_) => bail!("zaman aşımı, beklenen: {what}"),
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
    wait_for(session, &format!("{count} kişilik roster"), |event| match event {
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

/// Host odayı açar, katılan bağlanır: her iki taraf da aynı odayı görmeli.
#[tokio::test]
async fn peer_joins_and_both_sides_converge() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), "parola".into(), "ahmet").await?;

    let welcome = wait_for(&mut host, "host welcome", |e| match e {
        Event::Welcome { room, .. } => Some(room),
        _ => None,
    })
    .await?;
    assert_eq!(welcome.peers.len(), 1, "host kendini odada görmeli");
    assert_eq!(welcome.channels, vec!["genel", "oyun"]);

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "parola", "mehmet").await?;

    let guest_welcome = wait_for(&mut guest, "guest welcome", |e| match e {
        Event::Welcome { room, .. } => Some(room),
        _ => None,
    })
    .await?;
    assert_eq!(guest_welcome.room_name, "test odası");
    assert_eq!(
        guest_welcome.peers.len(),
        2,
        "katılan, kendisi dahil herkesi görmeli"
    );

    // Host tarafı da yeni geleni görmeli.
    let roster = wait_for_roster(&mut host, 2).await?;
    let names: Vec<&str> = roster.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"ahmet") && names.contains(&"mehmet"), "{names:?}");

    assert_ne!(host.me, guest.me, "kimlikler ayrı olmalı");
    assert_eq!(host.invite_code, guest.invite_code, "aynı odanın kodu");
    Ok(())
}

/// Yanlış parola ile bağlanma denemesi el sıkışmada reddedilmeli.
#[tokio::test]
async fn wrong_password_is_refused() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let _host = Coordinator::spawn(host_ep, test_room(), "doğru-parola".into(), "ahmet").await?;

    let guest_ep = bind_offline().await?;
    let result = Client::connect(guest_ep, host_addr, "yanlış-parola", "davetsiz").await;

    let err = result.err().expect("yanlış parola kabul edilmemeli").to_string();
    assert!(err.contains("parola"), "hata parolayı işaret etmeli: {err}");
    Ok(())
}

/// Chat mesajı gönderene de, diğerlerine de aynı biçimde ulaşmalı.
#[tokio::test]
async fn chat_reaches_everyone_including_the_sender() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "ahmet").await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "mehmet").await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    // Katılandan host'a.
    guest
        .commands
        .send(Command::Chat {
            channel: ChannelId(0),
            text: "merhaba herkese".into(),
        })
        .await?;
    wait_for_chat(&mut host, "merhaba herkese").await?;
    wait_for_chat(&mut guest, "merhaba herkese").await?;

    // Host'tan katılana — host'un kendi mesajı da aynı yoldan geçmeli.
    host.commands
        .send(Command::Chat {
            channel: ChannelId(1),
            text: "hoş geldin".into(),
        })
        .await?;
    wait_for_chat(&mut guest, "hoş geldin").await?;
    Ok(())
}

/// Kanal değişimi herkesin roster'ına yansımalı — Faz 2'de ses mesh'ini bu belirleyecek.
#[tokio::test]
async fn channel_switch_is_visible_to_everyone() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "ahmet").await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "mehmet").await?;
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

    let host_view = wait_for(&mut host, "host roster'ında kanal değişimi", |e| match e {
        Event::Roster(peers) if in_channel(&peers) => Some(peers),
        _ => None,
    })
    .await?;
    assert_eq!(host_view.len(), 2);

    wait_for(&mut guest, "guest roster'ında kanal değişimi", |e| match e {
        Event::Roster(peers) if in_channel(&peers) => Some(()),
        _ => None,
    })
    .await?;
    Ok(())
}

/// Olmayan bir kanala geçme isteği odayı bozmamalı, bağlantı da kopmamalı.
#[tokio::test]
async fn invalid_channel_request_is_ignored_without_breaking_the_session() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "ahmet").await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "mehmet").await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    guest
        .commands
        .send(Command::SwitchChannel(Some(ChannelId(99))))
        .await?;

    // Oturum ayakta kalmalı: sonrasında gönderilen chat hâlâ çalışıyor olmalı.
    guest
        .commands
        .send(Command::Chat {
            channel: ChannelId(0),
            text: "hâlâ buradayım".into(),
        })
        .await?;
    wait_for_chat(&mut host, "hâlâ buradayım").await?;
    Ok(())
}

/// Katılan ayrılınca roster'dan düşmeli.
#[tokio::test]
async fn leaving_updates_the_roster() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "ahmet").await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "mehmet").await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    guest.commands.send(Command::Quit).await?;

    let roster = wait_for_roster(&mut host, 1).await?;
    assert_eq!(roster[0].name, "ahmet", "geriye sadece host kalmalı");
    Ok(())
}

/// Aynı takma adla gelen ikinci kişi dışlanmamalı, ayırt edilmeli.
#[tokio::test]
async fn duplicate_nicknames_are_disambiguated_over_the_wire() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "ahmet").await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let guest_ep = bind_offline().await?;
    let mut guest = Client::connect(guest_ep, host_addr, "", "ahmet").await?;
    wait_for(&mut guest, "guest welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let roster = wait_for_roster(&mut host, 2).await?;
    let names: Vec<&str> = roster.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names.len(), 2);
    assert_ne!(names[0], names[1], "iki 'ahmet' ayırt edilebilmeli: {names:?}");
    Ok(())
}

/// Üç kişi: koordinatör, katılanlar arasındaki mesajları da doğru dağıtmalı.
#[tokio::test]
async fn three_participants_stay_in_sync() -> Result<()> {
    let host_ep = bind_offline().await?;
    let host_addr = host_ep.addr();
    let mut host = Coordinator::spawn(host_ep, test_room(), String::new(), "ahmet").await?;
    wait_for(&mut host, "host welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;

    let mut first = Client::connect(bind_offline().await?, host_addr.clone(), "", "mehmet").await?;
    wait_for(&mut first, "welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 2).await?;

    let mut second = Client::connect(bind_offline().await?, host_addr, "", "zeynep").await?;
    wait_for(&mut second, "welcome", |e| matches!(e, Event::Welcome { .. }).then_some(())).await?;
    wait_for_roster(&mut host, 3).await?;

    // Bir katılandan gönderilen mesaj, koordinatör üzerinden diğer katılana ulaşmalı.
    first
        .commands
        .send(Command::Chat {
            channel: ChannelId(0),
            text: "zeynep duyuyor musun".into(),
        })
        .await?;
    wait_for_chat(&mut second, "zeynep duyuyor musun").await?;
    wait_for_chat(&mut host, "zeynep duyuyor musun").await?;
    Ok(())
}
