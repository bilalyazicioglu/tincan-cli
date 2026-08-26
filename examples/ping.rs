//! Phase 0 probe — THROWAWAY CODE.
//!
//! Tests the project's biggest risk: can two machines on different networks (behind
//! NAT) reach each other from a code alone, and is the datagram flow healthy at speech
//! tempo (one packet every 20 ms)?
//!
//! What it measures: whether the connection is direct or relayed, the RTT
//! distribution, packet loss, and whether `max_datagram_size` is enough for an Opus
//! frame.
//!
//! Usage:
//!     Machine A:  cargo run --example ping -- host
//!     Machine B:  cargo run --example ping -- join <the-code-A-printed>

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use bytes::Bytes;
use iroh::{Endpoint, EndpointId, RelayMode, endpoint::presets};

const ALPN: &[u8] = b"tincan/probe/0";

/// Speech tempo: 48 kHz mono, 20 ms frames.
const FRAME_INTERVAL: Duration = Duration::from_millis(20);
/// At ~32 kbps, an Opus frame of 20 ms comes out roughly this size.
const FRAME_BYTES: usize = 120;
/// Ten seconds of stream.
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
                .context("usage: cargo run --example ping -- join <code>")?;
            join(&code).await
        }
        _ => bail!("usage: cargo run --example ping -- [host | join <code>]"),
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

/// The echo side: sends every incoming datagram straight back.
async fn host() -> Result<()> {
    let ep = bind().await?;

    println!("\n  Davet kodu:  {}\n", ep.id());
    println!("  On the other machine:  cargo run --example ping -- join {}\n", ep.id());
    println!("  Waiting for a connection...");

    while let Some(incoming) = ep.accept().await {
        let conn = match incoming.await {
            Ok(conn) => conn,
            Err(err) => {
                eprintln!("  incoming connection failed: {err:#}");
                continue;
            }
        };
        println!("  ✓ connected: {}", conn.remote_id().fmt_short());
        report_path(&conn, "host");

        let mut echoed = 0u64;
        while let Ok(datagram) = conn.read_datagram().await {
            conn.send_datagram(datagram)?;
            echoed += 1;
        }
        println!("  connection closed — {echoed} datagrams echoed");
        report_path(&conn, "host/son");
    }
    Ok(())
}

/// The measuring side: sends datagrams at speech tempo, matches the echoes, prints
/// the statistics.
async fn join(code: &str) -> Result<()> {
    let ep = bind().await?;
    let peer: EndpointId = code.trim().parse().context("invalid invite code")?;

    println!("\n  Connecting to: {peer}");
    let started = Instant::now();
    let conn = ep.connect(peer, ALPN).await.context("could not establish the connection")?;
    println!("  ✓ connected in {:?}", started.elapsed());

    match conn.max_datagram_size() {
        Some(size) if size >= FRAME_BYTES => {
            println!("  ✓ max_datagram_size = {size} bytes (ample for an Opus frame)")
        }
        Some(size) => println!("  ⚠ max_datagram_size = {size} bytes — an Opus frame ({FRAME_BYTES}) does not fit!"),
        None => bail!("the other side does not support datagrams — the architecture needs them"),
    }

    // Send times: seq -> the instant it went out.
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

    println!("\n  Sending {FRAME_COUNT} frames (20 ms apart, ~10 seconds)...");
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    for seq in 0..FRAME_COUNT {
        ticker.tick().await;
        let mut frame = vec![0u8; FRAME_BYTES];
        frame[..8].copy_from_slice(&seq.to_le_bytes());
        sent_at.lock().unwrap().insert(seq, Instant::now());
        if let Err(err) = conn.send_datagram(Bytes::from(frame)) {
            eprintln!("  send error (seq {seq}): {err}");
        }
    }

    // Wait for the last echoes still in flight.
    tokio::time::sleep(Duration::from_millis(500)).await;
    conn.close(0u32.into(), b"probe bitti");
    recv_task.abort();

    report_path(&conn, "join");
    report_stats(&rtts.lock().unwrap());
    Ok(())
}

/// Shows whether the connection flows directly or through a relay — this line is the
/// evidence for the project's central claim.
fn report_path(conn: &iroh::endpoint::Connection, label: &str) {
    println!("  [{label}] active paths: {:?}", conn.paths());
}

fn report_stats(rtts: &[Duration]) {
    let received = rtts.len() as u64;
    let lost = FRAME_COUNT.saturating_sub(received);
    let loss_pct = lost as f64 / FRAME_COUNT as f64 * 100.0;

    println!("\n  ── Results ──");
    println!("  sent: {FRAME_COUNT}   returned: {received}   lost: {lost} ({loss_pct:.1}%)");

    if rtts.is_empty() {
        println!("  ⚠ no echoes came back — the link looks up but no data is flowing");
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
        "  → one-way latency is roughly {:?}; our target for voice is <100 ms",
        mean / 2
    );
}
