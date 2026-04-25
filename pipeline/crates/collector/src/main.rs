// ─── AIS Collector ───────────────────────────────────────────────────────────
//
// Connects to the AISstream.io WebSocket API and writes every incoming message
// as-is (raw JSON) to JSONL files on the system disk.
//
// Files rotate every UTC minute so the mover can safely pick up completed
// ones without disturbing the currently-active file.
//
// JSONL file naming: ais_YYYYMMDD_HHMM_NNNN.jsonl
// ─────────────────────────────────────────────────────────────────────────────

mod writer;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use log::{error, info, warn};
use serde_json::json;
use tokio::signal::unix::{signal, SignalKind};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use barquitos_common::config::CollectorFile;
use barquitos_common::mount_check;

use crate::writer::RotatingFileWriter;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() {
    let args: Vec<String> = std::env::args().collect();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let cfg = CollectorFile::load_from_args_or_env(&args).unwrap_or_else(|e| {
        error!("Failed to load config: {e}");
        std::process::exit(1);
    });

    info!("═══════════════════════════════════════════════════════════════");
    info!("  AIS Collector v{}", env!("CARGO_PKG_VERSION"));
    info!("  WebSocket : {}", cfg.collector.ws_url);
    info!("  Output    : {}", cfg.collector.output_dir);
    if !cfg.collector.mount_point.is_empty() {
        info!("  Mount     : {}", cfg.collector.mount_point);
    }
    info!("═══════════════════════════════════════════════════════════════");

    // ── Pre-flight mount check (only if a mount_point is configured) ──
    if !cfg.collector.mount_point.is_empty() {
        if !mount_check::wait_for_mount(
            &cfg.collector.mount_point,
            cfg.mount.check_timeout_secs,
            cfg.mount.max_retries,
            cfg.mount.retry_interval_secs,
        ) {
            error!("💀 Mount {} not available. Aborting.", cfg.collector.mount_point);
            std::process::exit(1);
        }
        info!("✅ Mount {} healthy.", cfg.collector.mount_point);

        let _watchdog = mount_check::start_watchdog(
            cfg.collector.mount_point.clone(),
            cfg.mount.check_timeout_secs,
            cfg.mount.watchdog_interval_secs,
        );
    }

    // ── Tokio runtime ──
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("Failed to build Tokio runtime");

    rt.block_on(async_main(cfg));

    info!("Collector stopped.");
}

async fn async_main(cfg: CollectorFile) {
    // Signal handlers
    let mut sigint  = signal(SignalKind::interrupt()).expect("SIGINT handler failed");
    let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler failed");

    tokio::spawn(async move {
        tokio::select! {
            _ = sigint.recv()  => {},
            _ = sigterm.recv() => {},
        }
        info!("Shutdown signal received …");
        SHUTDOWN.store(true, Ordering::Relaxed);
    });

    // API key: env var overrides config
    let api_key = std::env::var("AISSTREAM_API_KEY")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.collector.api_key.clone());

    if api_key.is_empty() || api_key == "<YOUR_API_KEY>" {
        error!("No API key. Set AISSTREAM_API_KEY or put api_key in config.toml.");
        std::process::exit(1);
    }

    let mut file_writer = RotatingFileWriter::new(
        &cfg.collector.output_dir,
        0, // max_messages=0 means rotate on minute boundary only
        cfg.collector.mount_point.clone(),
        cfg.mount.check_timeout_secs,
    );

    let mut delay = cfg.collector.reconnect_delay_secs;

    while !SHUTDOWN.load(Ordering::Relaxed) {
        match collect(&api_key, &mut file_writer, &cfg).await {
            Ok(()) => {
                if SHUTDOWN.load(Ordering::Relaxed) { break; }
                delay = cfg.collector.reconnect_delay_secs; // reset on clean exit
            }
            Err(e) => {
                warn!("Connection lost ({e}). Reconnecting in {delay}s …");
                for _ in 0..delay {
                    if SHUTDOWN.load(Ordering::Relaxed) { break; }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                if SHUTDOWN.load(Ordering::Relaxed) { break; }
                delay = (delay * 2).min(cfg.collector.max_reconnect_delay_secs);
            }
        }
    }

    info!("Total messages this session: {}", file_writer.total_messages());
}

fn build_subscription(api_key: &str, cfg: &CollectorFile) -> String {
    let mut msg = json!({
        "APIKey": api_key,
        "BoundingBoxes": cfg.collector.bounding_boxes,
    });
    if !cfg.collector.filter_mmsi.is_empty() {
        msg["FiltersShipMMSI"] = json!(cfg.collector.filter_mmsi);
    }
    if !cfg.collector.filter_message_types.is_empty() {
        msg["FilterMessageTypes"] = json!(cfg.collector.filter_message_types);
    }
    msg.to_string()
}

async fn collect(
    api_key: &str,
    writer: &mut RotatingFileWriter,
    cfg: &CollectorFile,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Connecting to {} …", cfg.collector.ws_url);
    let (ws_stream, _) = connect_async(&cfg.collector.ws_url).await?;
    info!("Connected. Sending subscription …");

    let (mut sink, mut source) = ws_stream.split();
    sink.send(Message::Text(build_subscription(api_key, cfg))).await?;

    if cfg.collector.post_subscribe_delay_secs > 0 {
        tokio::time::sleep(Duration::from_secs(cfg.collector.post_subscribe_delay_secs)).await;
    }

    let mut total: u64 = 0;
    let mut mount_fails: u32 = 0;
    const MAX_MOUNT_FAILS: u32 = 5;

    // Keepalive ping — mirrors the Python fix (ping_interval=60, ping_timeout=60).
    // Also ensures any queued Pong responses from received Pings get flushed
    // (required because the stream is split: the sink doesn't drain automatically).
    let mut ping_timer = tokio::time::interval(Duration::from_secs(cfg.collector.ping_interval_secs));
    ping_timer.tick().await; // skip the immediate first tick

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) { break; }

        tokio::select! {
            biased;

            msg_result = source.next() => {
                let msg = match msg_result {
                    Some(Ok(m))  => m,
                    Some(Err(e)) => return Err(e.into()),
                    None         => { info!("Stream ended."); break; }
                };

                let text = match msg {
                    Message::Text(t)           => t,
                    Message::Binary(b)         => String::from_utf8_lossy(&b).to_string(),
                    Message::Ping(payload)     => {
                        // Explicit pong — split stream won't auto-flush from the read side.
                        sink.send(Message::Pong(payload)).await?;
                        continue;
                    }
                    Message::Pong(_)           => continue,
                    Message::Close(_)          => { info!("Server closed connection."); break; }
                    _                          => continue,
                };

                match writer.write(&text) {
                    Ok(true) => {
                        mount_fails = 0;
                        total += 1;
                        if total % cfg.collector.log_every_n_messages == 0 {
                            info!("Messages received: {total}");
                        }
                    }
                    Ok(false) => {
                        mount_fails += 1;
                        warn!("Mount unhealthy — dropped message ({mount_fails}/{MAX_MOUNT_FAILS})");
                        if mount_fails >= MAX_MOUNT_FAILS {
                            error!("Too many mount failures. Exiting.");
                            std::process::exit(1);
                        }
                    }
                    Err(e) => {
                        mount_fails += 1;
                        error!("Write error: {e} ({mount_fails}/{MAX_MOUNT_FAILS})");
                        if mount_fails >= MAX_MOUNT_FAILS {
                            std::process::exit(1);
                        }
                    }
                }
            }

            _ = ping_timer.tick() => {
                sink.send(Message::Ping(Default::default())).await?;
            }
        }
    }

    Ok(())
}
