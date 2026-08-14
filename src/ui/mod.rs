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
    /// Mikrofonun kapalı olup olmadığı; koordinatörden gelen duruma göre ayarlanır.
    pub muted: Arc<AtomicBool>,
    pub health: Arc<AudioHealth>,
    /// Hayatta tutulduğu sürece ses donanımı açık kalır.
    pub _devices: AudioDevices,
}

/// Oturumu ekrana bağlar ve kullanıcı çıkana kadar çalışır.
pub async fn run(mut session: Session, voice: Option<VoiceControl>) -> Result<()> {
    let mut app = App::new(session.me, session.invite_code.clone());
    app.voice_available = voice.is_some();
    let mut terminal = ratatui::init();
    let mut keys = spawn_key_reader();

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
                changed = wait_for_speakers(voice.as_ref()) => {
                    app.speaking = changed;
                }
                key = keys.recv() => match key {
                    Some(key) => {
                        if handle_key(&mut app, key, &session.commands).await? {
                            break;
                        }
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
    voice.muted.store(app.muted, Ordering::Relaxed);

    let members = match app.voice {
        Some(channel) => app.peers_in(channel).iter().map(|p| p.id).collect(),
        None => Vec::new(),
    };
    voice.mesh.set_membership(app.voice, members).await;
}

/// Konuşanlar listesindeki değişimi bekler. Ses kapalıysa asla dönmez —
/// `select!` içinde sonsuza kadar beklemesi istenen dal budur.
async fn wait_for_speakers(voice: Option<&VoiceControl>) -> HashSet<PeerId> {
    match voice {
        Some(voice) => {
            let mut speaking = voice.speaking.clone();
            if speaking.changed().await.is_ok() {
                speaking.borrow().clone()
            } else {
                std::future::pending().await
            }
        }
        None => std::future::pending().await,
    }
}
