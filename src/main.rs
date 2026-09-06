//! tincan — serverless voice chat that runs in your terminal.

use std::io::{IsTerminal as _, Write as _};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iroh::Endpoint;
use tincan::audio;
use tincan::clipboard;
use tincan::config::Config;
use tincan::invite;
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
    /// Open a new room and print its invite code.
    Host {
        /// The nickname you appear under in the room.
        #[arg(long, short)]
        name: Option<String>,
        /// Room password. Without one, anyone who has the code can walk in.
        #[arg(long, short)]
        password: Option<String>,
        /// Room name.
        #[arg(long, default_value = "tincan")]
        room: String,
        /// Comma-separated list of channels.
        #[arg(long, default_value = DEFAULT_CHANNELS)]
        channels: String,
        #[command(flatten)]
        audio: AudioArgs,
    },
    /// Join an existing room with an invite code.
    Join {
        /// The invite code the host shared.
        code: String,
        #[arg(long, short)]
        name: Option<String>,
        #[arg(long, short)]
        password: Option<String>,
        #[command(flatten)]
        audio: AudioArgs,
    },
    /// List the audio devices tincan can see.
    Devices,
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
    let file = std::fs::File::create(&path).ok()?;
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
            name,
            password,
            room,
            channels,
            audio,
        } => host(name, password, room, channels, audio).await,
        Sub::Join {
            code,
            name,
            password,
            audio,
        } => join(code, name, password, audio).await,
        Sub::Devices => {
            println!("{}", audio::device::describe_devices()?);
            Ok(())
        }
    }
}

async fn host(
    name: Option<String>,
    password: Option<String>,
    room_name: String,
    channels: String,
    audio: AudioArgs,
) -> Result<()> {
    tincan::logo::print_banner();
    let channels: Vec<String> = channels
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    let room = Room::new(room_name, channels)?;

    println!("{}", tincan::logo::heading("  connecting to the network…"));
    let endpoint = endpoint::bind().await?;
    let me = endpoint::to_peer_id(endpoint.id());
    let (mesh, control) = setup_voice(&endpoint, me, &audio);

    let mut session = Coordinator::spawn(
        endpoint,
        room,
        password.unwrap_or_default(),
        &nickname(name),
        mesh,
    )
    .await?;

    let copied = clipboard::copy(&session.invite_code);
    println!("\n{}", tincan::logo::heading("  the room is open. send this code to whoever you want in it:"));
    println!("\n    {}\n", tincan::logo::code(&session.invite_code));
    if copied {
        println!("{}", tincan::logo::heading("  it is on your clipboard already."));
    }
    println!("{}", tincan::logo::heading(&format!("  they run:  tincan join {}", session.invite_code)));

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
    code: String,
    name: Option<String>,
    password: Option<String>,
    audio: AudioArgs,
) -> Result<()> {
    tincan::logo::print_banner();
    let key = invite::decode(&code).context("could not read the invite code")?;
    let coordinator = PeerId(key);

    println!("{}", tincan::logo::heading("  connecting to the room…"));
    let endpoint = endpoint::bind().await?;
    let me = endpoint::to_peer_id(endpoint.id());
    let (mesh, control) = setup_voice(&endpoint, me, &audio);

    let target = endpoint::to_endpoint_id(&coordinator)?;
    let session = Client::connect(
        endpoint,
        target,
        &password.unwrap_or_default(),
        &nickname(name),
        mesh,
    )
    .await?;

    ui::run(session, control, audio.ptt).await
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
