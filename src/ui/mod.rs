//! Terminal arayüzü.

pub mod state;
mod view;

use anyhow::Result;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;

use crate::net::{Command, Session};
use state::App;

/// Oturumu ekrana bağlar ve kullanıcı çıkana kadar çalışır.
pub async fn run(mut session: Session) -> Result<()> {
    let mut app = App::new(session.me, session.invite_code.clone());
    let mut terminal = ratatui::init();
    let mut keys = spawn_key_reader();

    let result = async {
        loop {
            terminal.draw(|frame| view::draw(frame, &app))?;

            tokio::select! {
                event = session.events.recv() => match event {
                    Some(event) => app.apply(event),
                    None => break,
                },
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

    match key.code {
        KeyCode::Char('c') if ctrl => {
            let _ = commands.send(Command::Quit).await;
            return Ok(true);
        }

        // Ses kanalına gir / çık: görüntülenen kanal hedef alınır.
        KeyCode::Char('j') if ctrl => {
            let target = if app.voice == Some(app.viewing) {
                None
            } else {
                Some(app.viewing)
            };
            let _ = commands.send(Command::SwitchChannel(target)).await;
        }

        KeyCode::Char('m') if ctrl => {
            let _ = commands.send(Command::SetMuted(!app.muted)).await;
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
