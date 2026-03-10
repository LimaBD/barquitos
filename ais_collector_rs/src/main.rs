// ─── AIS Data Collector (Rust) ───────────────────────────────────────────────
//
// High-performance AIS data collector.  Connects to the AISstream.io
// WebSocket API and writes every incoming message as-is (raw JSON) into
// JSONL files, rotating by UTC minute.
//
// Features over the Python version:
//   • Zero-copy message forwarding (no JSON parse on write path)
//   • Async I/O via Tokio for minimal latency
//   • Mount-point health verification before every write
//   • Watchdog thread that exits the process if /mnt/wdblack hangs
//   • Exponential backoff reconnection
//   • Graceful shutdown on SIGINT / SIGTERM
//
// Runs 24/7 on a desktop i7 (16 GB RAM).
// ────────────────────────────────────────────────────────────────────────────

mod config;
mod mount_check;
mod writer;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use serde_json::json;
use tokio::signal::unix::{signal, SignalKind};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::writer::RotatingFileWriter;

/// Shared shutdown flag.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    info!("═══════════════════════════════════════════════════════════════");
    info!("  AIS Collector (Rust) v{}", env!("CARGO_PKG_VERSION"));
    info!("  WebSocket: {}", config::WS_URL);
    info!("  Output:    {}", config::DATA_OUTPUT_DIR);
    info!("  Mount:     {}", config::MOUNT_POINT);
    info!("═══════════════════════════════════════════════════════════════");

    // ── Pre-flight: verify mount is healthy ──
    if !mount_check::wait_for_mount(config::MOUNT_POINT, config::MOUNT_CHECK_TIMEOUT_SECS, 3, 10)
    {
        error!("💀 Mount {} not available at startup. Aborting.", config::MOUNT_POINT);
        std::process::exit(1);
    }
    info!("✅ Mount {} is healthy.", config::MOUNT_POINT);

    // ── Start watchdog thread ──
    // If mount becomes unresponsive at any point, the watchdog kills the process
    // to prevent indefinite hangs on blocked I/O.
    let _watchdog = mount_check::start_watchdog(
        config::MOUNT_POINT,
        config::MOUNT_CHECK_TIMEOUT_SECS,
        config::WATCHDOG_INTERVAL_SECS,
    );

    // ── Run async main loop ──
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("Failed to build Tokio runtime");

    rt.block_on(async_main());

    info!("Collector stopped. Goodbye!");
}

async fn async_main() {
    // Set up signal handlers
    let mut sigint = signal(SignalKind::interrupt()).expect("Failed to register SIGINT");
    let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM");

    tokio::spawn(async move {
        tokio::select! {
            _ = sigint.recv() => {},
            _ = sigterm.recv() => {},
        }
        info!("Shutdown signal received …");
        SHUTDOWN.store(true, Ordering::Relaxed);
    });

    // Resolve API key: env var takes precedence
    let api_key = std::env::var("AISSTREAM_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| config::API_KEY.to_string());

    if api_key.is_empty() || api_key == "<YOUR_API_KEY>" {
        error!("No API key configured. Set AISSTREAM_API_KEY env var or edit config.rs.");
        std::process::exit(1);
    }

    let mut file_writer =
        RotatingFileWriter::new(config::DATA_OUTPUT_DIR, config::MAX_MESSAGES_PER_FILE);

    let mut delay = config::RECONNECT_DELAY_SECS;

    while !SHUTDOWN.load(Ordering::Relaxed) {
        match collect(&api_key, &mut file_writer).await {
            Ok(()) => {
                // Clean exit (shutdown requested)
                if SHUTDOWN.load(Ordering::Relaxed) {
                    break;
                }
                delay = config::RECONNECT_DELAY_SECS; // reset on clean close
            }
            Err(e) => {
                warn!("Connection lost ({e}). Reconnecting in {delay}s …");
                // Sleep interruptibly
                for _ in 0..delay {
                    if SHUTDOWN.load(Ordering::Relaxed) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                if SHUTDOWN.load(Ordering::Relaxed) {
                    break;
                }
                delay = (delay * 2).min(config::MAX_RECONNECT_DELAY_SECS);
            }
        }
    }

    info!(
        "Total messages collected this session: {}",
        file_writer.total_messages()
    );
}

/// Build the JSON subscription message.
fn build_subscription(api_key: &str) -> String {
    let mut msg = json!({
        "APIKey": api_key,
        "BoundingBoxes": config::BOUNDING_BOXES,
    });

    if !config::FILTER_MMSI.is_empty() {
        msg["FiltersShipMMSI"] = json!(config::FILTER_MMSI);
    }
    if !config::FILTER_MESSAGE_TYPES.is_empty() {
        msg["FilterMessageTypes"] = json!(config::FILTER_MESSAGE_TYPES);
    }

    msg.to_string()
}

/// Connect, subscribe, and stream messages to disk.
async fn collect(
    api_key: &str,
    writer: &mut RotatingFileWriter,
) -> Result<(), Box<dyn std::error::Error>> {
    let subscription = build_subscription(api_key);

    info!("Connecting to {} …", config::WS_URL);

    let (ws_stream, _response) = connect_async(config::WS_URL).await?;
    info!("Connected. Sending subscription …");

    let (mut ws_sink, mut ws_source) = ws_stream.split();
    ws_sink.send(Message::Text(subscription)).await?;

    let mut total: u64 = 0;
    let mut mount_fail_count: u32 = 0;
    const MAX_MOUNT_FAILS: u32 = 5;

    while let Some(msg_result) = ws_source.next().await {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }

        let msg = msg_result?;
        let text = match msg {
            Message::Text(t) => t,
            Message::Binary(b) => String::from_utf8_lossy(&b).to_string(),
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => {
                info!("Server sent Close frame.");
                break;
            }
            _ => continue,
        };

        match writer.write(&text) {
            Ok(true) => {
                mount_fail_count = 0;
                total += 1;
                if total % config::LOG_EVERY_N_MESSAGES == 0 {
                    info!("Messages received so far: {total}");
                }
            }
            Ok(false) => {
                // Mount unhealthy — buffer messages briefly and retry
                mount_fail_count += 1;
                warn!(
                    "Mount unhealthy, message dropped ({}/{})",
                    mount_fail_count, MAX_MOUNT_FAILS
                );
                if mount_fail_count >= MAX_MOUNT_FAILS {
                    error!(
                        "💀 Mount failed {} consecutive times. Exiting to prevent data loss.",
                        MAX_MOUNT_FAILS
                    );
                    std::process::exit(1);
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Err(e) => {
                error!("Write error: {e}");
                mount_fail_count += 1;
                if mount_fail_count >= MAX_MOUNT_FAILS {
                    error!("💀 Too many write errors. Exiting.");
                    std::process::exit(1);
                }
            }
        }
    }

    info!("WebSocket closed. Total messages this session: {total}");
    Ok(())
}
