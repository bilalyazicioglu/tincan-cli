//! tincan — terminalde çalışan, sunucusuz sesli sohbet.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iroh::Endpoint;
use tincan::audio;
use tincan::invite;
use tincan::net::control::{Client, Coordinator};
use tincan::net::voice::VoiceMesh;
use tincan::net::endpoint;
use tincan::proto::PeerId;
use tincan::room::Room;
use tincan::ui::{self, VoiceControl};

/// Oda açılırken oluşturulan varsayılan kanallar.
const DEFAULT_CHANNELS: &str = "genel,oyun,müzik";

#[derive(Parser)]
#[command(name = "tincan", version, about = "Terminalde sunucusuz sesli sohbet")]
struct Cli {
    #[command(subcommand)]
    command: Sub,
}

#[derive(Subcommand)]
enum Sub {
    /// Yeni bir oda açar ve davet kodunu basar.
    Host {
        /// Odada görünecek takma adınız.
        #[arg(long, short)]
        name: Option<String>,
        /// Oda parolası. Verilmezse kodu bilen herkes girebilir.
        #[arg(long, short)]
        password: Option<String>,
        /// Oda adı.
        #[arg(long, default_value = "tincan")]
        room: String,
        /// Virgülle ayrılmış kanal listesi.
        #[arg(long, default_value = DEFAULT_CHANNELS)]
        channels: String,
        /// Sesi hiç açma; yalnızca yazışma.
        #[arg(long)]
        no_voice: bool,
    },
    /// Davet koduyla var olan bir odaya katılır.
    Join {
        /// Host'un paylaştığı davet kodu.
        code: String,
        #[arg(long, short)]
        name: Option<String>,
        #[arg(long, short)]
        password: Option<String>,
        #[arg(long)]
        no_voice: bool,
    },
    /// Ses cihazlarını listeler.
    Devices,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Loglar arayüzü bozmasın diye yalnızca istenirse stderr'e gider:
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
            no_voice,
        } => host(name, password, room, channels, no_voice).await,
        Sub::Join {
            code,
            name,
            password,
            no_voice,
        } => join(code, name, password, no_voice).await,
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
    no_voice: bool,
) -> Result<()> {
    let channels: Vec<String> = channels
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    let room = Room::new(room_name, channels)?;

    println!("ağa bağlanılıyor...");
    let endpoint = endpoint::bind().await?;
    let me = endpoint::to_peer_id(endpoint.id());
    let (mesh, control) = setup_voice(&endpoint, me, no_voice);

    let session = Coordinator::spawn(
        endpoint,
        room,
        password.unwrap_or_default(),
        &nickname(name),
        mesh,
    )
    .await?;

    println!("\n  Oda açıldı. Davet kodunu arkadaşlarınıza gönderin:\n");
    println!("      {}\n", session.invite_code);
    println!("  Onlar şunu çalıştıracak:  tincan join {}\n", session.invite_code);
    println!("  Arayüz açılıyor...");
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    ui::run(session, control).await
}

async fn join(
    code: String,
    name: Option<String>,
    password: Option<String>,
    no_voice: bool,
) -> Result<()> {
    let key = invite::decode(&code).context("davet kodu okunamadı")?;
    let coordinator = PeerId(key);

    println!("odaya bağlanılıyor...");
    let endpoint = endpoint::bind().await?;
    let me = endpoint::to_peer_id(endpoint.id());
    let (mesh, control) = setup_voice(&endpoint, me, no_voice);

    let target = endpoint::to_endpoint_id(&coordinator)?;
    let session = Client::connect(
        endpoint,
        target,
        &password.unwrap_or_default(),
        &nickname(name),
        mesh,
    )
    .await?;

    ui::run(session, control).await
}

/// Ses donanımını ve mesh'i kurar.
///
/// Ses açılamazsa (mikrofon izni yok, cihaz 48kHz değil) uygulama çökmemeli:
/// yazışma tarafı çalışmaya devam eder, kullanıcı arayüzde nedeni görür.
fn setup_voice(
    endpoint: &Endpoint,
    me: PeerId,
    disabled: bool,
) -> (Option<VoiceMesh>, Option<VoiceControl>) {
    if disabled {
        return (None, None);
    }
    match audio::start(me) {
        Ok(io) => {
            let mesh = VoiceMesh::start(endpoint.clone(), me, io.incoming.clone(), io.outgoing);
            let control = VoiceControl {
                mesh: mesh.clone(),
                speaking: io.speaking,
                muted: io.muted,
                health: io.health,
                _devices: io.devices,
            };
            (Some(mesh), Some(control))
        }
        Err(err) => {
            eprintln!("\n  ⚠ ses açılamadı, yalnızca yazışma: {err:#}\n");
            std::thread::sleep(std::time::Duration::from_millis(2500));
            (None, None)
        }
    }
}

/// Takma ad verilmediyse sistem kullanıcı adına düşer.
fn nickname(explicit: Option<String>) -> String {
    explicit
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "misafir".to_string())
}
