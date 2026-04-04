// ─── AIS Data Lake Ingestor ──────────────────────────────────────────────────
//
// File-based ingestion daemon.  Scans /mnt/wdblack/ais/ for JSONL files
// written by the Python AIS collector, parses them, applies edge-filtering,
// writes Parquet to the Bronze layer, then moves files to ais_processed/.
//
// Runs 24/7 on a desktop i7 (16 GB RAM, mechanical HDD).
// ────────────────────────────────────────────────────────────────────────────

mod config;
mod edge_state;
mod mount_check;
mod parser;
mod schema;
mod writer;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use std::io::{BufRead, BufReader};

use chrono::Utc;
use log::{debug, error, info, warn};

use crate::edge_state::EdgeState;
use crate::parser::{ParsedMessage, StaticRecord};
use crate::writer::*;

/// Shared shutdown flag set by Ctrl-C / SIGTERM.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() {
    // ── Check for --debug-file <path> argument ──
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--debug-file") {
        let file_path = args.get(pos + 1).unwrap_or_else(|| {
            eprintln!("Usage: ais_ingestor --debug-file <path_to_jsonl>");
            std::process::exit(1);
        });

        // Force verbose logging for debug mode
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug"))
            .format_timestamp_secs()
            .init();

        run_debug_single_file(file_path);
        return;
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    info!("═══════════════════════════════════════════════════════════════");
    info!("  AIS Data Lake Ingestor v{}", env!("CARGO_PKG_VERSION"));
    info!("  Input:     {}", config::AIS_INPUT_DIR);
    info!("  Processed: {}", config::AIS_PROCESSED_DIR);
    info!("  Lake root: {}", config::LAKE_ROOT);
    info!("  Mount:     {}", config::MOUNT_POINT);
    info!("  Scan interval: {}s", config::SCAN_INTERVAL_SECS);
    info!("  Files per batch: {}", config::FILES_PER_BATCH);
    info!("  Throttle: rest {}s every {} files",
        config::THROTTLE_REST_SECS, config::THROTTLE_FILE_COUNT);
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

    // Graceful shutdown on SIGINT / SIGTERM
    ctrlc::set_handler(|| {
        info!("Shutdown signal received …");
        SHUTDOWN.store(true, Ordering::Relaxed);
    })
    .expect("Failed to set Ctrl-C handler");

    // Ensure directories exist
    fs::create_dir_all(config::AIS_INPUT_DIR).ok();
    fs::create_dir_all(config::AIS_PROCESSED_DIR).ok();
    fs::create_dir_all(config::LAKE_ROOT).ok();

    // Edge state persists across batches to correctly detect pivots
    let mut edge = EdgeState::new();

    // Cumulative file counter for throttling (resets after rest)
    let mut files_since_rest: u64 = 0;

    // Main loop: scan → ingest batch → COOLDOWN → repeat
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            info!("Shutting down gracefully.");
            break;
        }

        // ── Mount health check before each batch ──
        if !mount_check::ensure_mount_healthy(
            config::MOUNT_POINT,
            config::MOUNT_CHECK_TIMEOUT_SECS,
        ) {
            error!("Mount unhealthy before batch. Waiting for recovery …");
            if !mount_check::wait_for_mount(
                config::MOUNT_POINT,
                config::MOUNT_CHECK_TIMEOUT_SECS,
                10,
                30,
            ) {
                error!("💀 Mount did not recover. Exiting.");
                std::process::exit(1);
            }
        }

        match scan_and_ingest(&mut edge) {
            Ok(0) => {
                // No files found — sleep before next scan
                sleep_interruptible(config::SCAN_INTERVAL_SECS);
            }
            Ok(n) => {
                files_since_rest += n as u64;
                info!("Batch complete: processed {n} files ({files_since_rest} since last rest).");

                // ── File-count throttling ──
                if files_since_rest >= config::THROTTLE_FILE_COUNT {
                    info!(
                        "🛏️  Throttle: processed {} files — resting for {}s ({}min) …",
                        files_since_rest,
                        config::THROTTLE_REST_SECS,
                        config::THROTTLE_REST_SECS / 60,
                    );
                    sleep_interruptible(config::THROTTLE_REST_SECS);
                    files_since_rest = 0;
                    info!("🔄 Throttle rest complete. Resuming ingestion.");
                } else {
                    // ⭐ MANDATORY COOLDOWN — even when more files are waiting.
                    info!(
                        "Cooldown: {}s before next batch (lets collector & OS use the disk) …",
                        config::COOLDOWN_AFTER_BATCH_SECS
                    );
                    sleep_interruptible(config::COOLDOWN_AFTER_BATCH_SECS);
                }
            }
            Err(e) => {
                error!("Batch error: {e}");
                sleep_interruptible(config::SCAN_INTERVAL_SECS);
            }
        }
    }

    info!("Ingestor stopped.  Edge state tracked {} vessels.", edge.vessel_count());
}

// ─── Debug: single-file mode ────────────────────────────────────────────────
//
// Processes ONE file with verbose output on a single thread.
// No mount checks, no watchdog, no throttling, no file moves.
// The file is left in place (not moved to ais_processed/).
// ────────────────────────────────────────────────────────────────────────────

fn run_debug_single_file(path: &str) {
    let file_path = PathBuf::from(path);
    if !file_path.exists() {
        error!("File not found: {}", file_path.display());
        std::process::exit(1);
    }

    info!("═══════════════════════════════════════════════════════════════");
    info!("  DEBUG MODE — single file, 1 thread, verbose");
    info!("  File: {}", file_path.display());
    info!("═══════════════════════════════════════════════════════════════");

    let mut edge = EdgeState::new();
    let mut pos_buf = PositionBuf::default();
    let mut draught_buf = DraughtBuf::default();
    let mut nav_buf = NavStatusBuf::default();
    let mut dest_buf = DestinationBuf::default();
    let mut static_buf = VesselStaticBuf::default();
    let mut name_buf = VesselNameHistoryBuf::default();
    let mut health = HealthCounters::default();

    let file = fs::File::open(&file_path).unwrap_or_else(|e| {
        error!("Cannot open file: {e}");
        std::process::exit(1);
    });
    let reader = BufReader::with_capacity(config::BUF_READER_CAPACITY, file);

    let mut line_num: u64 = 0;

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                warn!("  Line {} read error: {e}", line_num + 1);
                health.parse_errors += 1;
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }

        line_num += 1;
        health.total_messages += 1;

        match parser::parse_line(&line) {
            ParsedMessage::Position(p) => {
                health.position_reports += 1;
                health.unique_mmsis.insert(p.mmsi);

                debug!(
                    "  [L{}] POSITION mmsi={} ts={} lat={:.5} lon={:.5} sog={:?} cog={:?} hdg={:?} rot={:?} nav={:?} name={:?}",
                    line_num, p.mmsi, p.timestamp_s, p.latitude, p.longitude,
                    p.sog, p.cog, p.heading, p.rot, p.nav_status, p.ship_name
                );

                if let Some(status) = p.nav_status {
                    if let Some((new_s, old_s)) = edge.check_nav_status(p.mmsi, status) {
                        info!("  [L{}]   EDGE nav_status mmsi={}: {:?} → {}", line_num, p.mmsi, old_s, new_s);
                        nav_buf.mmsi.push(p.mmsi);
                        nav_buf.timestamp_s.push(p.timestamp_s);
                        nav_buf.nav_status.push(new_s);
                    }
                }

                if let Some(ref name) = p.ship_name {
                    if let Some(new_name) = edge.check_ship_name(p.mmsi, name) {
                        info!("  [L{}]   EDGE ship_name mmsi={}: → {:?}", line_num, p.mmsi, new_name);
                        name_buf.mmsi.push(p.mmsi);
                        name_buf.timestamp_s.push(p.timestamp_s);
                        name_buf.ship_name.push(new_name);
                    }
                }

                pos_buf.mmsi.push(p.mmsi);
                pos_buf.timestamp_s.push(p.timestamp_s);
                pos_buf.latitude.push(p.latitude);
                pos_buf.longitude.push(p.longitude);
                pos_buf.sog.push(p.sog);
                pos_buf.cog.push(p.cog);
                pos_buf.heading.push(p.heading);
                pos_buf.rot.push(p.rot);
            }
            ParsedMessage::Static(s) => {
                health.static_data_reports += 1;
                health.unique_mmsis.insert(s.mmsi);

                debug!(
                    "  [L{}] STATIC  mmsi={} ts={} name={:?} call={:?} imo={:?} type={:?} draught={:?} dest={:?} eta={:?}",
                    line_num, s.mmsi, s.timestamp_s, s.ship_name, s.call_sign,
                    s.imo_number, s.ship_type, s.draught, s.destination, s.eta_utc
                );

                process_static_record_debug(s, &mut edge, &mut draught_buf, &mut dest_buf, &mut static_buf, &mut name_buf, &mut health, line_num);
            }
            ParsedMessage::Ignored => {
                debug!("  [L{}] IGNORED", line_num);
            }
            ParsedMessage::Error(e) => {
                health.parse_errors += 1;
                warn!("  [L{}] PARSE ERROR: {e}", line_num);
            }
        }
    }

    // ── Flush all buffers to Parquet (same as normal mode) ──
    info!("Flushing buffers to Parquet (lake root: {}) …", config::LAKE_ROOT);
    let batch_start = Utc::now();

    let pos_count = flush_positions(config::LAKE_ROOT, &mut pos_buf)
        .unwrap_or_else(|e| { error!("flush_positions failed: {e}"); 0 });
    let dr_count = flush_draughts(config::LAKE_ROOT, &mut draught_buf)
        .unwrap_or_else(|e| { error!("flush_draughts failed: {e}"); 0 });
    let nav_count = flush_nav_statuses(config::LAKE_ROOT, &mut nav_buf)
        .unwrap_or_else(|e| { error!("flush_nav_statuses failed: {e}"); 0 });
    let dst_count = flush_destinations(config::LAKE_ROOT, &mut dest_buf)
        .unwrap_or_else(|e| { error!("flush_destinations failed: {e}"); 0 });
    let sta_count = flush_vessel_static(config::LAKE_ROOT, &mut static_buf)
        .unwrap_or_else(|e| { error!("flush_vessel_static failed: {e}"); 0 });
    let name_count = flush_vessel_name_history(config::LAKE_ROOT, &mut name_buf)
        .unwrap_or_else(|e| { error!("flush_vessel_name_history failed: {e}"); 0 });

    let batch_end = Utc::now();
    let window_start_s = batch_start.timestamp();
    let window_end_s = batch_end.timestamp();
    if let Err(e) = flush_health(config::LAKE_ROOT, &mut health, window_start_s, window_end_s) {
        error!("flush_health failed: {e}");
    }

    // ── Move processed file ──
    let filename = file_path.file_name().unwrap();
    let dest = Path::new(config::AIS_PROCESSED_DIR).join(filename);
    fs::create_dir_all(config::AIS_PROCESSED_DIR).ok();
    match fs::rename(&file_path, &dest) {
        Ok(()) => info!("  Moved → {}", dest.display()),
        Err(e) => {
            warn!("  rename failed ({e}), trying copy+delete …");
            match fs::copy(&file_path, &dest) {
                Ok(_) => {
                    fs::remove_file(&file_path).ok();
                    info!("  Copied+deleted → {}", dest.display());
                }
                Err(e2) => error!("  Failed to move {}: {e2}", file_path.display()),
            }
        }
    }

    info!("═══════════════════════════════════════════════════════════════");
    info!("  DEBUG SUMMARY");
    info!("  Lines read:        {}", line_num);
    info!("  Total messages:    {}", health.total_messages);
    info!("  Position reports:  {}", health.position_reports);
    info!("  Static reports:    {}", health.static_data_reports);
    info!("  Parse errors:      {}", health.parse_errors);
    info!("  Unique MMSIs:      {}", health.unique_mmsis.len());
    info!("  Parquet written: {} positions, {} draughts, {} nav, {} dest, {} static, {} names",
        pos_count, dr_count, nav_count, dst_count, sta_count, name_count);
    info!("  Edge state vessels: {}", edge.vessel_count());
    info!("═══════════════════════════════════════════════════════════════");
}

/// Like process_static_record but with verbose debug logging for each edge change.
fn process_static_record_debug(
    s: StaticRecord,
    edge: &mut EdgeState,
    draught_buf: &mut DraughtBuf,
    dest_buf: &mut DestinationBuf,
    static_buf: &mut VesselStaticBuf,
    name_buf: &mut VesselNameHistoryBuf,
    _health: &mut HealthCounters,
    line_num: u64,
) {
    if let Some(ref name) = s.ship_name {
        if let Some(new_name) = edge.check_ship_name(s.mmsi, name) {
            info!("  [L{}]   EDGE ship_name mmsi={}: → {:?}", line_num, s.mmsi, new_name);
            name_buf.mmsi.push(s.mmsi);
            name_buf.timestamp_s.push(s.timestamp_s);
            name_buf.ship_name.push(new_name);
        }
    }

    if let Some(draught) = s.draught {
        if let Some((new_d, old_d)) = edge.check_draught(s.mmsi, draught) {
            info!("  [L{}]   EDGE draught mmsi={}: {:?} → {}", line_num, s.mmsi, old_d, new_d);
            draught_buf.mmsi.push(s.mmsi);
            draught_buf.timestamp_s.push(s.timestamp_s);
            draught_buf.draught.push(new_d);
        }
    }

    if let Some(ref dest) = s.destination {
        if let Some((new_dest, old_dest)) = edge.check_destination(s.mmsi, dest) {
            info!("  [L{}]   EDGE destination mmsi={}: {:?} → {:?}", line_num, s.mmsi, old_dest, new_dest);
            dest_buf.mmsi.push(s.mmsi);
            dest_buf.timestamp_s.push(s.timestamp_s);
            dest_buf.destination.push(new_dest);
            dest_buf.eta_utc.push(s.eta_utc);
        }
    }

    let length_overall = match (s.dim_a, s.dim_b) {
        (Some(a), Some(b)) if (a + b) > 0 => Some(a + b),
        _ => None,
    };
    let beam = match (s.dim_c, s.dim_d) {
        (Some(c), Some(d)) if (c + d) > 0 => Some(c + d),
        _ => None,
    };

    let key = edge_state::VesselStaticKey {
        call_sign: s.call_sign.clone(),
        imo_number: s.imo_number,
        ship_type: s.ship_type,
        length_overall,
        beam,
    };
    if edge.check_vessel_static(s.mmsi, key) {
        info!(
            "  [L{}]   EDGE vessel_static mmsi={}: call={:?} imo={:?} type={:?} len={:?} beam={:?}",
            line_num, s.mmsi, s.call_sign, s.imo_number, s.ship_type, length_overall, beam
        );
        static_buf.mmsi.push(s.mmsi);
        static_buf.timestamp_s.push(s.timestamp_s);
        static_buf.call_sign.push(s.call_sign);
        static_buf.imo_number.push(s.imo_number);
        static_buf.ship_type.push(s.ship_type);
        static_buf.length_overall.push(length_overall);
        static_buf.beam.push(beam);
    }
}

/// Sleep for `secs` seconds, but check shutdown flag every second.
fn sleep_interruptible(secs: u64) {
    for _ in 0..secs {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// Sleep for `ms` milliseconds, checking shutdown flag.
fn sleep_ms_interruptible(ms: u64) {
    let chunks = ms / 100;
    let remainder = ms % 100;
    for _ in 0..chunks {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if remainder > 0 && !SHUTDOWN.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(remainder));
    }
}

/// Scan the input directory for JSONL files, process up to FILES_PER_BATCH,
/// flush to Parquet, and move processed files.  Returns number of files processed.
fn scan_and_ingest(edge: &mut EdgeState) -> Result<usize, Box<dyn std::error::Error>> {
    // ── Verify mount before reading the input directory ──
    if !mount_check::ensure_mount_healthy(
        config::MOUNT_POINT,
        config::MOUNT_CHECK_TIMEOUT_SECS,
    ) {
        return Err("Mount unhealthy — skipping batch".into());
    }

    let mut jsonl_files = list_jsonl_files(config::AIS_INPUT_DIR)?;
    if jsonl_files.is_empty() {
        return Ok(0);
    }

    // Sort by filename (which includes timestamp) so we process in order
    jsonl_files.sort();

    // Take at most FILES_PER_BATCH
    let batch: Vec<PathBuf> = jsonl_files
        .into_iter()
        .take(config::FILES_PER_BATCH)
        .collect();

    info!(
        "────── Batch: {} file(s) ──────",
        batch.len()
    );

    let batch_start = Utc::now();

    // Buffers — allocated once per batch, freed after flush
    let mut pos_buf = PositionBuf::default();
    let mut draught_buf = DraughtBuf::default();
    let mut nav_buf = NavStatusBuf::default();
    let mut dest_buf = DestinationBuf::default();
    let mut static_buf = VesselStaticBuf::default();
    let mut name_buf = VesselNameHistoryBuf::default();
    let mut health = HealthCounters::default();

    let mut total_lines: u64 = 0;

    for file_path in &batch {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break;
        }
        info!("  Reading: {}", file_path.display());
        let file = fs::File::open(file_path)?;
        let reader = BufReader::with_capacity(config::BUF_READER_CAPACITY, file);

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    warn!("  Line read error: {e}");
                    health.parse_errors += 1;
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }

            total_lines += 1;
            health.total_messages += 1;

            if total_lines % config::LOG_EVERY_N_MESSAGES == 0 {
                info!("  … parsed {total_lines} lines so far");
            }

            match parser::parse_line(&line) {
                ParsedMessage::Position(p) => {
                    health.position_reports += 1;
                    health.unique_mmsis.insert(p.mmsi);

                    // Edge filter nav status from position reports
                    if let Some(status) = p.nav_status {
                        if let Some((new_s, _old_s)) = edge.check_nav_status(p.mmsi, status) {
                            nav_buf.mmsi.push(p.mmsi);
                            nav_buf.timestamp_s.push(p.timestamp_s);
                            nav_buf.nav_status.push(new_s);
                        }
                    }

                    // Edge filter ship name from position metadata
                    if let Some(ref name) = p.ship_name {
                        if let Some(new_name) = edge.check_ship_name(p.mmsi, name) {
                            name_buf.mmsi.push(p.mmsi);
                            name_buf.timestamp_s.push(p.timestamp_s);
                            name_buf.ship_name.push(new_name);
                        }
                    }

                    // Buffer position (no ship_name, no nav_status, no message_type)
                    pos_buf.mmsi.push(p.mmsi);
                    pos_buf.timestamp_s.push(p.timestamp_s);
                    pos_buf.latitude.push(p.latitude);
                    pos_buf.longitude.push(p.longitude);
                    pos_buf.sog.push(p.sog);
                    pos_buf.cog.push(p.cog);
                    pos_buf.heading.push(p.heading);
                    pos_buf.rot.push(p.rot);
                }
                ParsedMessage::Static(s) => {
                    health.static_data_reports += 1;
                    health.unique_mmsis.insert(s.mmsi);
                    process_static_record(s, edge, &mut draught_buf, &mut dest_buf, &mut static_buf, &mut name_buf, &mut health);
                }
                ParsedMessage::Ignored => {}
                ParsedMessage::Error(e) => {
                    health.parse_errors += 1;
                    if health.parse_errors <= 10 {
                        warn!("  Parse error: {e}");
                    }
                }
            }

            // ── Early flush if position buffer is too large (prevent OOM) ──
            if pos_buf.len() >= config::MAX_POSITION_ROWS_BEFORE_FLUSH {
                info!("  ⚠ Position buffer hit {} rows — early flush to avoid OOM", pos_buf.len());
                let _ = flush_positions(config::LAKE_ROOT, &mut pos_buf);
                // Give the HDD a breather after a big write
                sleep_ms_interruptible(200);
            }
        }

        // Sleep between files to let the OS, collector, and HDD breathe
        if !SHUTDOWN.load(Ordering::Relaxed) {
            sleep_ms_interruptible(config::INTER_FILE_SLEEP_MS);
        }
    }

    // ── Verify mount before writing Parquet ──
    if !mount_check::ensure_mount_healthy(
        config::MOUNT_POINT,
        config::MOUNT_CHECK_TIMEOUT_SECS,
    ) {
        return Err("Mount unhealthy before flush — aborting batch to prevent hang".into());
    }

    // ── Flush all buffers to Parquet ────────────────────────────────────
    info!("Flushing buffers to Parquet …");
    let pos_count = flush_positions(config::LAKE_ROOT, &mut pos_buf)?;
    let dr_count = flush_draughts(config::LAKE_ROOT, &mut draught_buf)?;
    let nav_count = flush_nav_statuses(config::LAKE_ROOT, &mut nav_buf)?;
    let dst_count = flush_destinations(config::LAKE_ROOT, &mut dest_buf)?;
    let sta_count = flush_vessel_static(config::LAKE_ROOT, &mut static_buf)?;
    let name_count = flush_vessel_name_history(config::LAKE_ROOT, &mut name_buf)?;

    let batch_end = Utc::now();
    let window_start_s = batch_start.timestamp();
    let window_end_s = batch_end.timestamp();
    flush_health(config::LAKE_ROOT, &mut health, window_start_s, window_end_s)?;

    info!(
        "Flush complete: {} positions, {} draughts, {} nav, {} dest, {} static, {} names",
        pos_count, dr_count, nav_count, dst_count, sta_count, name_count
    );

    // ── Verify mount before moving files ──
    if !mount_check::ensure_mount_healthy(
        config::MOUNT_POINT,
        config::MOUNT_CHECK_TIMEOUT_SECS,
    ) {
        return Err("Mount unhealthy before file move — aborting".into());
    }

    // ── Move processed files ────────────────────────────────────────────
    for file_path in &batch {
        let filename = file_path.file_name().unwrap();
        let dest = Path::new(config::AIS_PROCESSED_DIR).join(filename);
        match fs::rename(file_path, &dest) {
            Ok(()) => info!("  Moved → {}", dest.display()),
            Err(e) => {
                // rename fails across mount points; fall back to copy+delete
                warn!("  rename failed ({e}), trying copy+delete …");
                match fs::copy(file_path, &dest) {
                    Ok(_) => {
                        fs::remove_file(file_path).ok();
                        info!("  Copied+deleted → {}", dest.display());
                    }
                    Err(e2) => error!("  Failed to move {}: {e2}", file_path.display()),
                }
            }
        }
    }

    Ok(batch.len())
}

/// Process a ShipStaticData record through all edge filters.
fn process_static_record(
    s: StaticRecord,
    edge: &mut EdgeState,
    draught_buf: &mut DraughtBuf,
    dest_buf: &mut DestinationBuf,
    static_buf: &mut VesselStaticBuf,
    name_buf: &mut VesselNameHistoryBuf,
    _health: &mut HealthCounters,
) {
    // Edge filter: ship name
    if let Some(ref name) = s.ship_name {
        if let Some(new_name) = edge.check_ship_name(s.mmsi, name) {
            name_buf.mmsi.push(s.mmsi);
            name_buf.timestamp_s.push(s.timestamp_s);
            name_buf.ship_name.push(new_name);
        }
    }

    // Edge filter: draught
    if let Some(draught) = s.draught {
        if let Some((new_d, _old_d)) = edge.check_draught(s.mmsi, draught) {
            draught_buf.mmsi.push(s.mmsi);
            draught_buf.timestamp_s.push(s.timestamp_s);
            draught_buf.draught.push(new_d);
        }
    }

    // Edge filter: destination
    if let Some(ref dest) = s.destination {
        if let Some((new_dest, _old_dest)) = edge.check_destination(s.mmsi, dest) {
            dest_buf.mmsi.push(s.mmsi);
            dest_buf.timestamp_s.push(s.timestamp_s);
            dest_buf.destination.push(new_dest);
            dest_buf.eta_utc.push(s.eta_utc);
        }
    }

    // Compute length and beam from AIS dimensions
    let length_overall = match (s.dim_a, s.dim_b) {
        (Some(a), Some(b)) if (a + b) > 0 => Some(a + b),
        _ => None,
    };
    let beam = match (s.dim_c, s.dim_d) {
        (Some(c), Some(d)) if (c + d) > 0 => Some(c + d),
        _ => None,
    };

    // Edge filter: vessel static — only write when any field changes
    let key = edge_state::VesselStaticKey {
        call_sign: s.call_sign.clone(),
        imo_number: s.imo_number,
        ship_type: s.ship_type,
        length_overall,
        beam,
    };
    if edge.check_vessel_static(s.mmsi, key) {
        static_buf.mmsi.push(s.mmsi);
        static_buf.timestamp_s.push(s.timestamp_s);
        static_buf.call_sign.push(s.call_sign);
        static_buf.imo_number.push(s.imo_number);
        static_buf.ship_type.push(s.ship_type);
        static_buf.length_overall.push(length_overall);
        static_buf.beam.push(beam);
    }
}

/// List all .jsonl files in a directory, sorted by name.
fn list_jsonl_files(dir: &str) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let path = Path::new(dir);
    if !path.exists() {
        return Ok(vec![]);
    }
    let mut files: Vec<PathBuf> = fs::read_dir(path)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    files.sort();
    Ok(files)
}
