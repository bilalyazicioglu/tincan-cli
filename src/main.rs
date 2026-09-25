//! tincan — serverless voice chat that runs in your terminal.

use std::io::{IsTerminal as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use clap::{CommandFactory, Parser, Subcommand};
use iroh::Endpoint;
use tincan::audio;
use tincan::auth::{Admission, Key, RoomSecret};
use tincan::clipboard;
use tincan::config::Config;
use tincan::invite;
use tincan::passphrase;
use tincan::audio::device::Wanted;
use tincan::net::Command;
use tincan::net::control::{Client, Coordinator};
use tincan::net::voice::VoiceMesh;
use tincan::net::endpoint;
use tincan::proto::PeerId;
use tincan::room::Room;
use tincan::ui::{self, VoiceControl};

/// Channels created by default when a room is opened.
const DEFAULT_CHANNELS: &str = "general,gaming,music";
/// What a room opened without a name is called on screen.
const UNNAMED_ROOM: &str = "tincan";

#[derive(Parser)]
#[command(
    name = "tincan",
    version,
    about = "Serverless voice chat in your terminal",
    before_help = tincan::logo::BANNER
)]
struct Cli {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Subcommand)]
enum Sub {
    /// Open a room: by name and passphrase, or without a name behind an invite code.
    Host {
        /// Room name. With the passphrase it is the room's address: whoever knows both can
        /// join by typing them. Leave it out to get an invite code instead.
        room: Option<String>,
        /// The nickname you appear under in the room.
        #[arg(long, short)]
        name: Option<String>,
        /// Passphrase. With a room name one is generated if you leave this out; without a
        /// room name and without this, anyone who has the code can walk in.
        #[arg(long, short)]
        password: Option<String>,
        /// Comma-separated list of channels.
        #[arg(long, default_value = DEFAULT_CHANNELS)]
        channels: String,
        #[command(flatten)]
        audio: AudioArgs,
    },
    /// Join a room by its name, or with an invite code.
    Join {
        /// The room name, or the invite code the host shared.
        room: String,
        /// The nickname you appear under in the room.
        #[arg(long, short)]
        name: Option<String>,
        /// Passphrase. Joining by room name without it asks for it instead, which keeps
        /// it out of the process list.
        #[arg(long, short)]
        password: Option<String>,
        #[command(flatten)]
        audio: AudioArgs,
    },
    /// List the audio devices tincan can see.
    Devices {
        /// Include what the device picker leaves out: ALSA plugins and each card's raw PCMs.
        #[arg(long)]
        all: bool,
    },
    /// Generate shell auto-completion scripts.
    Completions {
        /// Shell to generate completions for.
        shell: clap_complete::Shell,
    },
}

/// Flags shared by the commands that use audio.
#[derive(clap::Args, Clone)]
struct AudioArgs {
    /// Skip audio entirely; text chat only.
    #[arg(long)]
    no_voice: bool,
    /// Microphone to use (a distinctive part of its name is enough).
    #[arg(long)]
    input: Option<String>,
    /// Speaker to use (a distinctive part of its name is enough).
    #[arg(long)]
    output: Option<String>,
    /// Push-to-talk: the microphone only opens with F4.
    #[arg(long)]
    ptt: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parsed first: `--help` and a bad argument both exit here, and neither should
    // leave a log file behind.
    let command = Cli::parse().command;
    if let Sub::Completions { shell } = command {
        clap_complete::generate(shell, &mut Cli::command(), "tincan", &mut std::io::stdout());
        return Ok(());
    }
    let log = start_logging();

    let result = run(command).await;
    report_log(log);
    result
}

/// Sends this run's log somewhere it cannot land on top of the interface.
///
/// The interface draws on the terminal and the alternate screen does not capture
/// stderr, so one warning printed straight over the room — which is exactly what it
/// did. A redirected stderr is left alone: `2>tincan.log` has always meant "put the
/// log there", and it still does.
fn start_logging() -> Option<PathBuf> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "warn".into());

    if !std::io::stderr().is_terminal() {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(filter)
            .init();
        return None;
    }

    // Nowhere to write is a reason to stay quiet, not a reason to scribble on the
    // interface: without `init` the macros do nothing at all.
    let path = log_path()?;
    // Appending, because the interface points stderr at this same file while it is up
    // (see `tincan::stderr`), and two writers at their own offsets overwrite each other.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let _ = file.set_len(0);
    if let Ok(copy) = file.try_clone() {
        tincan::stderr::sink_into(copy);
    }
    tracing_subscriber::fmt()
        .with_writer(std::sync::Arc::new(file))
        .with_ansi(false)
        .with_env_filter(filter)
        .init();
    Some(path)
}

fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    let dir = base.join("tincan");
    std::fs::create_dir_all(&dir).ok()?;
    // One file per run. Two tincans on one machine is an ordinary thing to do while
    // testing, and they must not write over each other.
    Some(dir.join(format!("{}.log", std::process::id())))
}

/// Says where the log is, but only when there is something in it.
///
/// A clean run should end in silence, and a bad one in a single line — not in a wall
/// of text arriving at the moment you decided to stop reading. Whatever mattered to
/// the user was already said in the room while it happened.
fn report_log(path: Option<PathBuf>) {
    let Some(path) = path else {
        return;
    };
    let empty = std::fs::metadata(&path).map(|file| file.len() == 0).unwrap_or(true);
    if empty {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let lines = std::fs::read_to_string(&path)
        .map(|log| log.lines().count())
        .unwrap_or(0);
    let plural = if lines == 1 { "" } else { "s" };
    eprintln!("\n  {lines} log line{plural} from this session: {}", path.display());
}

async fn run(command: Sub) -> Result<()> {
    match command {
        Sub::Host {
            room,
            name,
            password,
            channels,
            audio,
        } => host(room, name, password, channels, audio).await,
        Sub::Join {
            room,
            name,
            password,
            audio,
        } => join(room, name, password, audio).await,
        Sub::Devices { all } => {
            println!("{}", audio::device::describe_devices(all)?);
            Ok(())
        }
        Sub::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "tincan", &mut std::io::stdout());
            Ok(())
        }
    }
}

async fn host(
    room_name: Option<String>,
    name: Option<String>,
    password: Option<String>,
    channels: String,
    audio: AudioArgs,
) -> Result<()> {
    tincan::logo::print_banner();
    let channels: Vec<String> = channels
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();

    // A room opened by name always has a passphrase, generated unless one was given: with
    // the name it is the room's address, and a guessable one is a room anyone can find.
    let generated = room_name.is_some() && password.is_none();
    let password = match password {
        Some(typed) => typed,
        None if generated => passphrase::generate(),
        None => String::new(),
    };
    let secret = match &room_name {
        Some(room) => {
            if !generated && passphrase::is_weak(&password) {
                println!("{}", tincan::logo::heading(
                    "  that passphrase is easy to guess, and here it is the room's address as well as its lock.\n  leave out -p and tincan makes one up.",
                ));
            }
            Some(RoomSecret::derive(room, &password)?)
        }
        None => None,
    };
    let room = Room::new(room_name.as_deref().unwrap_or(UNNAMED_ROOM).trim(), channels)?;

    println!("{}", tincan::logo::heading("  connecting to the network…"));
    let endpoint = endpoint::bind(secret.as_ref().map(RoomSecret::identity)).await?;
    let me = endpoint::to_peer_id(endpoint.id());
    let admission = match &secret {
        Some(secret) => Admission::room(secret, &password)?,
        None => Admission::invite(&password, &me)?,
    };
    let (mesh, control) = setup_voice(&endpoint, me, &audio);

    let mut session = Coordinator::spawn(endpoint, room, admission, &nickname(name), mesh).await?;

    match &room_name {
        Some(room) => {
            println!("\n{}", tincan::logo::heading("  the room is open. tell whoever you want in it:"));
            println!("\n    room:        {}", tincan::logo::code(room.trim()));
            println!("    passphrase:  {}\n", tincan::logo::code(&password));
            println!("{}", tincan::logo::heading(&format!("  they run:  tincan join {}", shell_word(room.trim()))));
        }
        None => {
            let copied = clipboard::copy(&session.invite_code);
            println!("\n{}", tincan::logo::heading("  the room is open. send this code to whoever you want in it:"));
            println!("\n    {}\n", tincan::logo::code(&session.invite_code));
            if copied {
                println!("{}", tincan::logo::heading("  it is on your clipboard already."));
            }
            println!("{}", tincan::logo::heading(&format!("  they run:  tincan join {}", session.invite_code)));
        }
    }

    // Wait for the user rather than a timer. The interface takes over the whole
    // screen, and a 63-character code is not something anyone can copy against a
    // countdown. F1 brings it back once the interface is up.
    print!("\n{}", tincan::logo::heading("  press enter to open the room. f1 brings the code back, f6 is audio."));
    std::io::stdout().flush().ok();

    if let Leaving::Interrupted = wait_at_the_prompt().await {
        // Leaving from the prompt is still leaving. Without this the process dies
        // holding an open endpoint, which iroh rightly complains about, and the room
        // is never told it closed.
        println!();
        let _ = session.commands.send(Command::Quit).await;
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), session.events.recv()).await;
        return Ok(());
    }

    ui::run(session, control, audio.ptt).await
}

async fn join(
    room: String,
    name: Option<String>,
    password: Option<String>,
    audio: AudioArgs,
) -> Result<()> {
    tincan::logo::print_banner();
    let (coordinator, key) = if invite::looks_like_code(&room) {
        let coordinator = PeerId(invite::decode(&room).context("could not read the invite code")?);
        let key = Key::for_invite(&password.unwrap_or_default(), &coordinator)?;
        (coordinator, key)
    } else {
        let passphrase = match password {
            Some(typed) => typed,
            None => ask_passphrase(&room)?,
        };
        let secret = RoomSecret::derive(&room, &passphrase)?;
        (secret.coordinator(), secret.key().clone())
    };

    println!("{}", tincan::logo::heading("  connecting to the room…"));
    // The joiner's own identity is always fresh: only the coordinator's is derived.
    let endpoint = endpoint::bind(None).await?;
    let me = endpoint::to_peer_id(endpoint.id());
    let (mesh, control) = setup_voice(&endpoint, me, &audio);

    let target = endpoint::to_endpoint_id(&coordinator)?;
    let session = Client::connect(endpoint, target, &key, &nickname(name), mesh).await?;

    ui::run(session, control, audio.ptt).await
}

/// Reads the passphrase from the terminal, or from stdin when it is piped.
///
/// `-p` still works, but anyone on the machine can read it in `ps` — and for a room opened
/// by name the passphrase is the room's address, not only its lock. It is not hidden while
/// typed: it is meant to be said out loud anyway.
fn ask_passphrase(room: &str) -> Result<String> {
    print!("{}", tincan::logo::heading(&format!("  passphrase for {}: ", room.trim())));
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("could not read the passphrase")?;
    let passphrase = line.trim().to_string();
    ensure!(!passphrase.is_empty(), "joining a room by name needs its passphrase");
    Ok(passphrase)
}

/// Quotes a room name for the "they run" line when the shell would split it.
fn shell_word(word: &str) -> String {
    if word.chars().all(|c| c.is_alphanumeric() || "-_.".contains(c)) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// How the wait at the prompt ended.
enum Leaving {
    Enter,
    Interrupted,
}

/// Waits for Enter, or for the user to give up on the room.
///
/// The line is read on a plain thread rather than through `tokio::io::stdin`, which is
/// backed by the runtime's blocking pool. A blocking read cannot be cancelled: dropping
/// the future on ctrl+c leaves the thread sitting on `read`, and the runtime will not
/// finish shutting down until it returns — so the program printed its goodbye and then
/// waited forever for a keypress that was never coming. A detached thread is something
/// the process is allowed to walk away from. `ui::spawn_key_reader` reads keys the same
/// way, for the same reason.
async fn wait_at_the_prompt() -> Leaving {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<()>(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = tx.blocking_send(());
    });

    tokio::select! {
        _ = rx.recv() => Leaving::Enter,
        _ = tokio::signal::ctrl_c() => Leaving::Interrupted,
    }
}

/// Brings up the audio hardware and the mesh.
///
/// If audio cannot start (no microphone permission, device is not 48 kHz) the app must
/// not die: text chat keeps working and the user sees the reason in the interface.
fn setup_voice(
    endpoint: &Endpoint,
    me: PeerId,
    args: &AudioArgs,
) -> (Option<VoiceMesh>, Option<VoiceControl>) {
    if args.no_voice {
        return (None, None);
    }
    let config = Config::load();
    let choice = audio::device::DeviceChoice {
        input: Wanted::pick(args.input.clone(), config.input_device),
        output: Wanted::pick(args.output.clone(), config.output_device),
    };
    match audio::start(me, &choice) {
        Ok(io) => {
            let mesh = VoiceMesh::start(endpoint.clone(), me, io.incoming.clone(), io.outgoing);
            let control = VoiceControl {
                mesh: mesh.clone(),
                speaking: io.speaking,
                mic_open: io.mic_open,
                hearing: io.hearing,
                mic_level: io.mic_level,
                peer_levels: io.peer_levels,
                peer_gains: io.peer_gains,
                mic_test: io.mic_test,
                gate: io.gate,
                denoise: io.denoise,
                health: io.health,
                blip_tx: io.blip_tx,
                devices: io.devices,
            };
            (Some(mesh), Some(control))
        }
        Err(err) => {
            eprintln!("\n  audio could not start, so this is a text-only session: {err:#}\n");
            std::thread::sleep(std::time::Duration::from_millis(2500));
            (None, None)
        }
    }
}

/// Falls back to the system username when no nickname is given.
fn nickname(explicit: Option<String>) -> String {
    explicit
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "guest".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_takes_the_room_name_as_its_argument() {
        let cli = Cli::try_parse_from(["tincan", "host", "lobby", "-p", "a-b-c-d"]).unwrap();
        let Sub::Host { room, password, .. } = cli.command else {
            panic!("expected host");
        };
        assert_eq!(room.as_deref(), Some("lobby"));
        assert_eq!(password.as_deref(), Some("a-b-c-d"));

        let cli = Cli::try_parse_from(["tincan", "host"]).unwrap();
        assert!(matches!(cli.command, Sub::Host { room: None, .. }), "the unnamed room stays");
    }

    #[test]
    fn join_takes_a_room_name_or_a_code() {
        for target in ["lobby", "n73w-kuqc-uog2"] {
            let cli = Cli::try_parse_from(["tincan", "join", target]).unwrap();
            assert!(matches!(cli.command, Sub::Join { ref room, .. } if room == target));
        }
    }

    #[test]
    fn room_names_are_quoted_only_when_the_shell_needs_it() {
        assert_eq!(shell_word("lobby"), "lobby");
        assert_eq!(shell_word("game-night_2"), "game-night_2");
        assert_eq!(shell_word("game night"), "'game night'");
        assert_eq!(shell_word("bob's"), "'bob'\\''s'");
    }

    #[test]
    fn completions_generate_for_supported_shells() {
        use clap_complete::Shell;

        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell] {
            let mut buf = Vec::new();
            clap_complete::generate(shell, &mut Cli::command(), "tincan", &mut buf);
            let script = String::from_utf8(buf).expect("completion script should be valid utf-8");
            assert!(!script.is_empty(), "completions for {shell:?} must not be empty");
            assert!(script.contains("host"), "must mention host command for {shell:?}");
            assert!(script.contains("join"), "must mention join command for {shell:?}");
            assert!(script.contains("completions"), "must mention completions command for {shell:?}");
        }
    }
}

