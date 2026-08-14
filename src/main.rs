//! tincan — terminalde çalışan, sunucusuz sesli sohbet.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tincan::invite;
use tincan::net::control::{Client, Coordinator};
use tincan::net::endpoint;
use tincan::proto::PeerId;
use tincan::room::Room;
use tincan::ui;

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
    },
    /// Davet koduyla var olan bir odaya katılır.
    Join {
        /// Host'un paylaştığı davet kodu.
        code: String,
        #[arg(long, short)]
        name: Option<String>,
        #[arg(long, short)]
        password: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Loglar arayüzü bozmasın diye dosyaya değil, yalnızca istenirse stderr'e gider.
    // (RUST_LOG=debug tincan ... 2>tincan.log)
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
        } => host(name, password, room, channels).await,
        Sub::Join {
            code,
            name,
            password,
        } => join(code, name, password).await,
    }
}

async fn host(
    name: Option<String>,
    password: Option<String>,
    room_name: String,
    channels: String,
) -> Result<()> {
    let channels: Vec<String> = channels
        .split(',')
        .map(|c| c.trim().to_string())
        .filter(|c| !c.is_empty())
        .collect();
    let room = Room::new(room_name, channels)?;

    println!("ağa bağlanılıyor...");
    let endpoint = endpoint::bind().await?;
    let session = Coordinator::spawn(endpoint, room, password.unwrap_or_default(), &nickname(name))
        .await?;

    println!("\n  Oda açıldı. Davet kodunu arkadaşlarınıza gönderin:\n");
    println!("      {}\n", session.invite_code);
    println!("  Onlar şunu çalıştıracak:  tincan join {}\n", session.invite_code);
    println!("  Arayüz açılıyor...");
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    ui::run(session).await
}

async fn join(code: String, name: Option<String>, password: Option<String>) -> Result<()> {
    let key = invite::decode(&code).context("davet kodu okunamadı")?;
    let coordinator = PeerId(key);

    println!("odaya bağlanılıyor...");
    let endpoint = endpoint::bind().await?;
    let target = endpoint::to_endpoint_id(&coordinator)?;
    let session = Client::connect(
        endpoint,
        target,
        &password.unwrap_or_default(),
        &nickname(name),
    )
    .await?;

    ui::run(session).await
}

/// Takma ad verilmediyse sistem kullanıcı adına düşer.
fn nickname(explicit: Option<String>) -> String {
    explicit
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "misafir".to_string())
}
