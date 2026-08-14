//! Faz 0 probe — ATILABILIR KOD.
//!
//! Projenin en büyük riskini test eder: farklı ağlardaki (NAT arkasındaki) iki makine
//! sadece bir kodla birbirine bağlanabiliyor mu, ve ses temposunda (20ms'de bir paket)
//! datagram akışı sağlıklı mı?
//!
//! Ölçülenler: bağlantı doğrudan mı yoksa relay üzerinden mi kuruldu, RTT dağılımı,
//! paket kaybı, ve `max_datagram_size` bir Opus çerçevesine yetiyor mu.
//!
//! Kullanım:
//!     Makine A:  cargo run --example ping -- host
//!     Makine B:  cargo run --example ping -- join <A-nin-bastigi-kod>

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use iroh::{Endpoint, EndpointId, RelayMode, endpoint::presets};

const ALPN: &[u8] = b"tincan/probe/0";

/// Ses temposu: 48kHz mono, 20ms çerçeve.
const FRAME_INTERVAL: Duration = Duration::from_millis(20);
/// ~32kbps Opus'ta 20ms'lik bir çerçeve kabaca bu boyutta olur.
const FRAME_BYTES: usize = 120;
/// 10 saniyelik akış.
const FRAME_COUNT: u64 = 500;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "host" => host().await,
        "join" => {
            let code = std::env::args()
                .nth(2)
                .context("kullanım: cargo run --example ping -- join <kod>")?;
            join(&code).await
        }
        _ => bail!("kullanım: cargo run --example ping -- [host | join <kod>]"),
    }
}

async fn bind() -> Result<Endpoint> {
    let ep = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Default)
        .bind()
        .await?;
    ep.online().await;
    Ok(ep)
}

/// Yankı tarafı: gelen her datagramı olduğu gibi geri gönderir.
async fn host() -> Result<()> {
    let ep = bind().await?;

    println!("\n  Davet kodu:  {}\n", ep.id());
    println!("  Diğer makinede:  cargo run --example ping -- join {}\n", ep.id());
    println!("  Bağlantı bekleniyor...");

    while let Some(incoming) = ep.accept().await {
        let conn = match incoming.await {
            Ok(conn) => conn,
            Err(err) => {
                eprintln!("  gelen bağlantı başarısız: {err:#}");
                continue;
            }
        };
        println!("  ✓ bağlandı: {}", conn.remote_id().fmt_short());
        report_path(&conn, "host");

        let mut echoed = 0u64;
        while let Ok(datagram) = conn.read_datagram().await {
            conn.send_datagram(datagram)?;
            echoed += 1;
        }
        println!("  bağlantı kapandı — {echoed} datagram yankılandı");
        report_path(&conn, "host/son");
    }
    Ok(())
}

/// Ölçüm tarafı: ses temposunda datagram gönderir, yankıları eşleştirir, istatistik basar.
async fn join(code: &str) -> Result<()> {
    let ep = bind().await?;
    let peer: EndpointId = code.trim().parse().context("geçersiz davet kodu")?;

    println!("\n  Bağlanılıyor: {peer}");
    let started = Instant::now();
    let conn = ep.connect(peer, ALPN).await.context("bağlantı kurulamadı")?;
    println!("  ✓ bağlantı {:?} içinde kuruldu", started.elapsed());

    match conn.max_datagram_size() {
        Some(size) if size >= FRAME_BYTES => {
            println!("  ✓ max_datagram_size = {size} bayt (Opus çerçevesi için fazlasıyla yeterli)")
        }
        Some(size) => println!("  ⚠ max_datagram_size = {size} bayt — Opus çerçevesi ({FRAME_BYTES}) sığmıyor!"),
        None => bail!("karşı taraf datagram desteklemiyor — mimari bu olmadan çalışmaz"),
    }

    // Gönderim zamanları: seq -> gönderildiği an.
    let sent_at: Arc<Mutex<HashMap<u64, Instant>>> = Arc::default();
    let rtts: Arc<Mutex<Vec<Duration>>> = Arc::default();

    let recv_task = tokio::spawn({
        let conn = conn.clone();
        let sent_at = sent_at.clone();
        let rtts = rtts.clone();
        async move {
            while let Ok(datagram) = conn.read_datagram().await {
                let Ok(bytes) = <[u8; 8]>::try_from(&datagram[..8]) else {
                    continue;
                };
                let seq = u64::from_le_bytes(bytes);
                let sent = sent_at.lock().unwrap().remove(&seq);
                if let Some(sent) = sent {
                    rtts.lock().unwrap().push(sent.elapsed());
                }
            }
        }
    });

    println!("\n  {FRAME_COUNT} çerçeve gönderiliyor (20ms aralık, ~10 saniye)...");
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    for seq in 0..FRAME_COUNT {
        ticker.tick().await;
        let mut frame = vec![0u8; FRAME_BYTES];
        frame[..8].copy_from_slice(&seq.to_le_bytes());
        sent_at.lock().unwrap().insert(seq, Instant::now());
        if let Err(err) = conn.send_datagram(Bytes::from(frame)) {
            eprintln!("  gönderim hatası (seq {seq}): {err}");
        }
    }

    // Yoldaki son yankıların gelmesi için bekle.
    tokio::time::sleep(Duration::from_millis(500)).await;
    conn.close(0u32.into(), b"probe bitti");
    recv_task.abort();

    report_path(&conn, "join");
    report_stats(&rtts.lock().unwrap());
    Ok(())
}

/// Bağlantının doğrudan mı yoksa relay üzerinden mi aktığını gösterir —
/// projenin asıl iddiasının kanıtı bu satırda.
fn report_path(conn: &iroh::endpoint::Connection, etiket: &str) {
    println!("  [{etiket}] aktif yollar: {:?}", conn.paths());
}

fn report_stats(rtts: &[Duration]) {
    let received = rtts.len() as u64;
    let lost = FRAME_COUNT.saturating_sub(received);
    let loss_pct = lost as f64 / FRAME_COUNT as f64 * 100.0;

    println!("\n  ── Sonuçlar ──");
    println!("  gönderilen: {FRAME_COUNT}   dönen: {received}   kayıp: {lost} (%{loss_pct:.1})");

    if rtts.is_empty() {
        println!("  ⚠ hiç yankı dönmedi — bağlantı kurulmuş görünse de veri akmıyor");
        return;
    }

    let mut sorted: Vec<_> = rtts.to_vec();
    sorted.sort_unstable();
    let pick = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
    let mean = sorted.iter().sum::<Duration>() / sorted.len() as u32;

    println!(
        "  RTT  ort={:?}  p50={:?}  p95={:?}  p99={:?}  max={:?}",
        mean,
        pick(0.50),
        pick(0.95),
        pick(0.99),
        sorted[sorted.len() - 1],
    );
    println!(
        "  → tek yön gecikme kabaca {:?}; ses için hedefimiz <100ms",
        mean / 2
    );
}
