//! Terminal arayüzü.

pub mod state;
mod view;

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::{mpsc, watch};

use crate::audio::device::{AudioDevices, AudioHealth};
use crate::net::voice::VoiceMesh;
use crate::net::{Command, Event, Session};
use crate::proto::PeerId;
use state::App;

/// Arayüzün ses tarafına tutunduğu yer. Ses açılamadıysa hiç kurulmaz.
pub struct VoiceControl {
    pub mesh: VoiceMesh,
    /// O anda konuşanlar.
    pub speaking: watch::Receiver<HashSet<PeerId>>,
    /// Mikrofon açık mı — arayüz hesaplar, ses motoru okur.
    pub mic_open: Arc<AtomicBool>,
    /// Karşı tarafları duyuyor muyuz.
    pub hearing: Arc<AtomicBool>,
    pub health: Arc<AudioHealth>,
    /// Hayatta tutulduğu sürece ses donanımı açık kalır.
    pub _devices: AudioDevices,
}

/// Oturumu ekrana bağlar ve kullanıcı çıkana kadar çalışır.
pub async fn run(
    mut session: Session,
    mut voice: Option<VoiceControl>,
    ptt_mode: bool,
) -> Result<()> {
    let mut app = App::new(session.me, session.invite_code.clone());
    app.voice_available = voice.is_some();
    app.ptt_mode = ptt_mode && app.voice_available;
    let mut terminal = ratatui::init();
    let mut keys = spawn_key_reader();
    // Bağlantı kalitesi anlık değil, gözle takip edilen bir bilgi: saniyede bir yeter.
    let mut quality_tick = tokio::time::interval(std::time::Duration::from_secs(1));

    let result = async {
        loop {
            terminal.draw(|frame| view::draw(frame, &app))?;

            tokio::select! {
                event = session.events.recv() => match event {
                    Some(event) => {
                        let membership_changed =
                            matches!(event, Event::Roster(_) | Event::Welcome { .. });
                        app.apply(event);
                        if membership_changed {
                            sync_voice(&app, voice.as_ref()).await;
                        }
                    }
                    None => break,
                },

                // Konuşma göstergesi ses motorundan gelir.
                changed = next_speakers(voice.as_mut().map(|v| &mut v.speaking)) => {
                    app.speaking = changed;
                }
                _ = quality_tick.tick() => {
                    if let Some(voice) = voice.as_ref() {
                        app.link = voice.mesh.link_status().await;
                        app.audio_dropouts = voice.health.underruns();
                    }
                }

                key = keys.recv() => match key {
                    Some(key) => {
                        if handle_key(&mut app, key, &session.commands).await? {
                            break;
                        }
                        apply_local_audio_state(&app, voice.as_ref());
                    }
                    None => break,
                },
            }

            if let Some(reason) = app.ended.clone() {
                // Kullanıcı son durumu görebilsin diye kapanmadan önce bir kare daha çiz.
                app.status = Some(reason);
                terminal.draw(|frame| view::draw(frame, &app))?;
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                break;
            }
        }
        anyhow::Ok(())
    }
    .await;

    ratatui::restore();
    drop(voice);
    if let Some(reason) = app.ended {
        println!("{reason}");
    }
    result
}

/// Tuş okuma bloklayıcı bir işlem; kendi thread'inde çalışıp kanala aktarılır.
fn spawn_key_reader() -> mpsc::Receiver<KeyEvent> {
    let (tx, rx) = mpsc::channel(64);
    std::thread::spawn(move || {
        loop {
            match crossterm::event::read() {
                Ok(TermEvent::Key(key)) if key.kind == KeyEventKind::Press => {
                    if tx.blocking_send(key).is_err() {
                        return;
                    }
                }
                Ok(_) => continue,
                Err(_) => return,
            }
        }
    });
    rx
}

/// Tuşu işler. Çıkılacaksa `true` döner.
async fn handle_key(
    app: &mut App,
    key: KeyEvent,
    commands: &mpsc::Sender<Command>,
) -> Result<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Ses kısayolları bilerek F-tuşları: terminalde Ctrl+M (0x0D) ve Ctrl+J (0x0A)
    // Enter'ın kendisidir, ondan ayırt edilemez. Onları kullansaydık "sustur" tuşu
    // sessizce mesaj gönderirdi. Ctrl+G / Ctrl+T çakışmasız alternatifler.
    let toggle_voice = key.code == KeyCode::F(2) || (ctrl && key.code == KeyCode::Char('g'));
    let toggle_mute = key.code == KeyCode::F(3) || (ctrl && key.code == KeyCode::Char('t'));
    let push_to_talk = key.code == KeyCode::F(4);
    let toggle_deafen = key.code == KeyCode::F(5);

    if push_to_talk && app.ptt_mode {
        // Terminaller tuş bırakma olayını genelde bildirmez; bu yüzden bas-konuş
        // burada "bas-aç / bas-kapat" olarak çalışır. Basılı tutma desteği için
        // terminalin klavye geliştirmelerini bildirmesi gerekir (bkz. run()).
        app.ptt_active = !app.ptt_active;
        return Ok(false);
    }
    if toggle_deafen {
        let deafened = !app.deafened;
        let _ = commands.send(Command::SetDeafened(deafened)).await;
        // Sağırlaştırırken mikrofonu da kapatmak beklenen davranış: duymadığın
        // bir sohbete konuşmaya devam etmek karşı tarafı yanıltır.
        if deafened && !app.muted {
            let _ = commands.send(Command::SetMuted(true)).await;
        }
        return Ok(false);
    }

    if toggle_voice {
        // Hedef, o an bakılan kanal: kullanıcı hangi kanalı görüyorsa oraya girer.
        let target = if app.voice == Some(app.viewing) {
            None
        } else {
            Some(app.viewing)
        };
        let _ = commands.send(Command::SwitchChannel(target)).await;
        return Ok(false);
    }
    if toggle_mute {
        let _ = commands.send(Command::SetMuted(!app.muted)).await;
        return Ok(false);
    }

    match key.code {
        KeyCode::Char('c') if ctrl => {
            let _ = commands.send(Command::Quit).await;
            return Ok(true);
        }

        KeyCode::Tab => app.view_next(true),
        KeyCode::BackTab => app.view_next(false),

        KeyCode::Enter => {
            if let Some(text) = app.take_input() {
                let channel = app.viewing;
                let _ = commands.send(Command::Chat { channel, text }).await;
            }
        }

        KeyCode::Backspace => {
            app.input.pop();
        }

        KeyCode::Char(c) if !ctrl => app.input.push(c),

        _ => {}
    }
    Ok(false)
}

/// Roster değiştiğinde ses mesh'ini yeni üyeliğe göre günceller ve susturma
/// durumunu koordinatörün söylediğiyle hizalar.
async fn sync_voice(app: &App, voice: Option<&VoiceControl>) {
    let Some(voice) = voice else {
        return;
    };
    apply_local_audio_state(app, Some(voice));

    let members = match app.voice {
        Some(channel) => app.peers_in(channel).iter().map(|p| p.id).collect(),
        None => Vec::new(),
    };
    voice.mesh.set_membership(app.voice, members).await;
}

/// Arayüzün hesapladığı mikrofon/kulaklık kararını ses motoruna aktarır.
fn apply_local_audio_state(app: &App, voice: Option<&VoiceControl>) {
    let Some(voice) = voice else {
        return;
    };
    voice.mic_open.store(app.mic_open(), Ordering::Relaxed);
    voice.hearing.store(!app.deafened, Ordering::Relaxed);
}

/// Konuşanlar listesindeki bir sonraki değişimi bekler. Ses kapalıysa asla dönmez —
/// `select!` içinde sonsuza kadar beklemesi istenen dal budur.
async fn next_speakers(
    speaking: Option<&mut watch::Receiver<HashSet<PeerId>>>,
) -> HashSet<PeerId> {
    match speaking {
        Some(speaking) => {
            // Receiver'ın kendisi ilerletilmeli. Kopyası üzerinden beklenirse asıl
            // receiver değişikliği "görmemiş" sayılır ve bir sonraki bekleme anında
            // döner — arayüz döngüsü boşa döner.
            if speaking.changed().await.is_ok() {
                speaking.borrow_and_update().clone()
            } else {
                std::future::pending().await
            }
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn speaker(seed: u8) -> HashSet<PeerId> {
        HashSet::from([PeerId([seed; 32])])
    }

    /// Konuşma durumu değiştiğinde arayüz uyanmalı.
    #[tokio::test]
    async fn wakes_up_when_someone_starts_speaking() {
        let (tx, mut rx) = watch::channel(HashSet::new());
        tx.send(speaker(1)).unwrap();

        let speakers = next_speakers(Some(&mut rx)).await;
        assert_eq!(speakers, speaker(1));
    }

    /// Ama yeni bir değişiklik yokken beklemeye devam etmeli.
    ///
    /// Aksi halde arayüz döngüsü boşa dönerek CPU'yu yakar ve ekranı durmadan
    /// yeniden çizer — kullanıcının fark edeceği tek şey ısınan bilgisayardır.
    #[tokio::test]
    async fn does_not_spin_when_nothing_changes() {
        let (tx, mut rx) = watch::channel(HashSet::new());
        tx.send(speaker(1)).unwrap();
        next_speakers(Some(&mut rx)).await;

        let again = tokio::time::timeout(Duration::from_millis(150), next_speakers(Some(&mut rx)));
        assert!(
            again.await.is_err(),
            "değişiklik yokken hemen dönmemeli — boş döngü oluşur"
        );
    }

    /// Ses kapalıyken bu dal hiç seçilmemeli.
    #[tokio::test]
    async fn never_returns_when_voice_is_off() {
        let result = tokio::time::timeout(Duration::from_millis(100), next_speakers(None));
        assert!(result.await.is_err());
    }
}
