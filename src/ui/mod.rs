//! The terminal interface.

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

/// Where the interface holds on to the audio side. Never built if audio failed to
/// start.
pub struct VoiceControl {
    pub mesh: VoiceMesh,
    /// Who is currently speaking.
    pub speaking: watch::Receiver<HashSet<PeerId>>,
    /// Whether the microphone is open — the interface computes it, the engine reads it.
    pub mic_open: Arc<AtomicBool>,
    /// Whether we can hear the others.
    pub hearing: Arc<AtomicBool>,
    pub health: Arc<AudioHealth>,
    pub blip_tx: mpsc::Sender<()>,
    /// The audio hardware stays open for as long as this is kept alive.
    pub _devices: AudioDevices,
}

impl VoiceControl {
    pub fn play_blip(&self) {
        let _ = self.blip_tx.try_send(());
    }
}

/// Wires the session to the screen and runs until the user quits.
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
    // Link quality is something you watch, not something instantaneous: once a
    // second is plenty.
    let mut quality_tick = tokio::time::interval(std::time::Duration::from_secs(1));

    let result = async {
        loop {
            terminal.draw(|frame| view::draw(frame, &app))?;

            tokio::select! {
                event = session.events.recv() => match event {
                    Some(event) => {
                        let membership_changed =
                            matches!(event, Event::Roster(_) | Event::Welcome { .. });
                        let prev_voice = app.voice;
                        app.apply(event);
                        if membership_changed {
                            sync_voice(&app, voice.as_ref()).await;
                            if app.voice.is_some() && app.voice != prev_voice {
                                if let Some(v) = voice.as_ref() {
                                    v.play_blip();
                                }
                            }
                        }
                    }
                    None => break,
                },

                // The speaking indicator comes from the audio engine.
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
                        if handle_key(&mut app, key, &session.commands, voice.as_ref()).await? {
                            break;
                        }
                        apply_local_audio_state(&app, voice.as_ref());
                    }
                    None => break,
                },
            }

            if let Some(reason) = app.ended.clone() {
                // Draw one more frame before closing so the user sees the final state.
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

/// Reading keys blocks, so it runs on its own thread and is piped into a channel.
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

/// Handles a key. Returns `true` if we should quit.
async fn handle_key(
    app: &mut App,
    key: KeyEvent,
    commands: &mpsc::Sender<Command>,
    voice: Option<&VoiceControl>,
) -> Result<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // The audio shortcuts are F-keys on purpose: in a terminal Ctrl+M (0x0D) and
    // Ctrl+J (0x0A) *are* Enter and cannot be told apart from it. Had we used those,
    // the "mute" key would have quietly sent a message. Ctrl+G / Ctrl+T are
    // conflict-free alternatives.
    let toggle_voice = key.code == KeyCode::F(2) || (ctrl && key.code == KeyCode::Char('g'));
    let toggle_mute = key.code == KeyCode::F(3) || (ctrl && key.code == KeyCode::Char('t'));
    let push_to_talk = key.code == KeyCode::F(4);
    let show_code = key.code == KeyCode::F(1);

    if show_code {
        let copied = crate::clipboard::copy(&app.invite_code);
        app.show_invite_code(copied);
        return Ok(false);
    }
    let toggle_deafen = key.code == KeyCode::F(5);

    if push_to_talk && app.ptt_mode {
        // Terminals usually do not report key-release events, so push-to-talk works
        // here as press-to-open / press-to-close. Real hold-to-talk would require the
        // terminal to advertise keyboard enhancements (see run()).
        app.ptt_active = !app.ptt_active;
        return Ok(false);
    }
    if toggle_deafen {
        let deafened = !app.deafened;
        let _ = commands.send(Command::SetDeafened(deafened)).await;
        // Closing the microphone along with deafening is the expected behaviour:
        // carrying on talking into a conversation you cannot hear misleads the others.
        if deafened && !app.muted {
            let _ = commands.send(Command::SetMuted(true)).await;
        }
        return Ok(false);
    }

    if toggle_voice {
        // The target is whichever channel is on screen: you join the one you see.
        let target = if app.voice == Some(app.viewing) {
            None
        } else {
            Some(app.viewing)
        };
        let _ = commands.send(Command::SwitchChannel(target)).await;
        if target.is_some() {
            if let Some(v) = voice {
                v.play_blip();
            }
        }
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

/// When the roster changes, updates the voice mesh to the new membership and aligns
/// the mute state with what the coordinator reports.
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

/// Passes the interface's microphone/headphone decision down to the audio engine.
fn apply_local_audio_state(app: &App, voice: Option<&VoiceControl>) {
    let Some(voice) = voice else {
        return;
    };
    voice.mic_open.store(app.mic_open(), Ordering::Relaxed);
    voice.hearing.store(!app.deafened, Ordering::Relaxed);
}

/// Waits for the next change in the speaker list. Never returns when audio is off —
/// this is the `select!` branch that is meant to wait forever.
async fn next_speakers(
    speaking: Option<&mut watch::Receiver<HashSet<PeerId>>>,
) -> HashSet<PeerId> {
    match speaking {
        Some(speaking) => {
            // The receiver itself has to be advanced. Waiting on a clone leaves the
            // real receiver counted as not having seen the change, so the next wait
            // returns immediately and the interface loop spins.
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

    /// The interface must wake when the speaking state changes.
    #[tokio::test]
    async fn wakes_up_when_someone_starts_speaking() {
        let (tx, mut rx) = watch::channel(HashSet::new());
        tx.send(speaker(1)).unwrap();

        let speakers = next_speakers(Some(&mut rx)).await;
        assert_eq!(speakers, speaker(1));
    }

    /// But with no new change it must keep waiting.
    ///
    /// Otherwise the interface loop spins, burns CPU and redraws the screen endlessly —
    /// and the only thing the user notices is a hot laptop.
    #[tokio::test]
    async fn does_not_spin_when_nothing_changes() {
        let (tx, mut rx) = watch::channel(HashSet::new());
        tx.send(speaker(1)).unwrap();
        next_speakers(Some(&mut rx)).await;

        let again = tokio::time::timeout(Duration::from_millis(150), next_speakers(Some(&mut rx)));
        assert!(
            again.await.is_err(),
            "must not return immediately with no change — that is a spin loop"
        );
    }

    /// With audio off this branch must never be selected.
    #[tokio::test]
    async fn never_returns_when_voice_is_off() {
        let result = tokio::time::timeout(Duration::from_millis(100), next_speakers(None));
        assert!(result.await.is_err());
    }
}
