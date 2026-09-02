//! tincan — serverless voice chat that runs in your terminal.

use std::io::Write as _;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iroh::Endpoint;
use tokio::io::{AsyncBufReadExt, BufReader};
use tincan::audio;
use tincan::clipboard;
use tincan::config::Config;
use tincan::invite;
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
    // Logs only go to stderr when asked for, so they cannot scramble the interface:
    //   RUST_LOG=debug tincan host 2>tincan.log
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn".into()),
        )
        .init();

    match Cli::parse().command {
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

    let session = Coordinator::spawn(
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
    let mut answer = String::new();
    BufReader::new(tokio::io::stdin()).read_line(&mut answer).await.ok();

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
        input: args.input.clone().or(config.input_device),
        output: args.output.clone().or(config.output_device),
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
                mic_loopback: io.mic_loopback,
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
