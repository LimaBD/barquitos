// ─── AIS Data Lake Ingestor ───────────────────────────────────────────────────
//
// File-based ingestion daemon.  Scans the configured input directory for JSONL
// files, parses them with edge-filtering, writes Parquet to the Bronze layer,
// then moves processed files.
//
// Anti-crash features:
//   • Every file read runs in a dedicated thread with a configurable deadline.
//     If a bad sector causes the read to hang, the file is quarantined and the
//     daemon moves on — it never blocks forever.
//   • A background watchdog exits the process if the disk mount freezes.
//   • All mount health checks run with timeouts.
//   • Corrupted files that pass the read but fail to parse are counted in
//     health metrics and skipped — they don't crash the daemon.
// ─────────────────────────────────────────────────────────────────────────────

mod edge_state;
mod parser;
mod schema;
mod writer;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use chrono::Utc;
use log::{debug, error, info, warn};

use barquitos_common::config::IngestorFile;
use barquitos_common::mount_check;
use barquitos_common::safe_io::{quarantine_file, read_lines_with_timeout, ReadLinesResult};

use crate::edge_state::EdgeState;
use crate::parser::{ParsedMessage, StaticRecord};
use crate::writer::*;

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // ── Debug single-file mode ──
    if let Some(pos) = args.iter().position(|a| a == "--debug-file") {
        let path = args.get(pos + 1).unwrap_or_else(|| {
            eprintln!("Usage: ais_ingestor --debug-file <path.jsonl> [--config <cfg>]");
            std::process::exit(1);
        });
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug"))
            .format_timestamp_secs()
            .init();
        let cfg = IngestorFile::load_from_args_or_env(&args).unwrap_or_else(|e| {
            error!("Config: {e}"); std::process::exit(1);
        });
        run_debug_single_file(path, &cfg);
        return;
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let cfg = IngestorFile::load_from_args_or_env(&args).unwrap_or_else(|e| {
        error!("Failed to load config: {e}");
        std::process::exit(1);
    });

    info!("═══════════════════════════════════════════════════════════════");
    info!("  AIS Data Lake Ingestor v{}", env!("CARGO_PKG_VERSION"));
    info!("  Input:          {}", cfg.ingestor.input_dir);
    info!("  Processed:      {}", cfg.ingestor.processed_dir);
    info!("  Quarantine:     {}", cfg.ingestor.quarantine_dir);
    info!("  Lake root:      {}", cfg.ingestor.lake_root);
    info!("  Mount:          {}", cfg.ingestor.mount_point);
    info!("  File timeout:   {}s (anti-crash)", cfg.ingestor.file_read_timeout_secs);
    info!("  Files/batch:    {}", cfg.ingestor.files_per_batch);
    info!("  Cooldown:       {}s", cfg.ingestor.cooldown_after_batch_secs);
    info!("═══════════════════════════════════════════════════════════════");

    // ── Pre-flight mount check ──
    if !mount_check::wait_for_mount(
        &cfg.ingestor.mount_point,
        cfg.mount.check_timeout_secs,
        cfg.mount.max_retries,
        cfg.mount.retry_interval_secs,
    ) {
        error!("💀 Mount {} unavailable. Aborting.", cfg.ingestor.mount_point);
        std::process::exit(1);
    }
    info!("✅ Mount {} healthy.", cfg.ingestor.mount_point);

    // ── Watchdog: kills process if mount freezes ──
    let _watchdog = mount_check::start_watchdog(
        cfg.ingestor.mount_point.clone(),
        cfg.mount.check_timeout_secs,
        cfg.mount.watchdog_interval_secs,
    );

    // ── Graceful shutdown ──
    ctrlc::set_handler(|| {
        info!("Shutdown signal received …");
        SHUTDOWN.store(true, Ordering::Relaxed);
    })
    .expect("ctrlc handler failed");

    // Ensure directories exist
    fs::create_dir_all(&cfg.ingestor.input_dir).ok();
    fs::create_dir_all(&cfg.ingestor.processed_dir).ok();
    fs::create_dir_all(&cfg.ingestor.quarantine_dir).ok();
    fs::create_dir_all(&cfg.ingestor.lake_root).ok();

    let mut edge = EdgeState::new();
    let mut files_since_rest: u64 = 0;

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            info!("Shutting down gracefully.");
            break;
        }

        // Mount check before every batch
        if !mount_check::ensure_mount_healthy(&cfg.ingestor.mount_point, cfg.mount.check_timeout_secs) {
            error!("Mount unhealthy before batch. Waiting …");
            if !mount_check::wait_for_mount(
                &cfg.ingestor.mount_point,
                cfg.mount.check_timeout_secs,
                cfg.mount.max_retries,
                cfg.mount.retry_interval_secs,
            ) {
                error!("💀 Mount did not recover. Exiting.");
                std::process::exit(1);
            }
        }

        match scan_and_ingest(&mut edge, &cfg) {
            Ok(0) => {
                sleep_interruptible(cfg.ingestor.scan_interval_secs);
            }
            Ok(n) => {
                files_since_rest += n as u64;
                info!("Batch complete: {n} files ({files_since_rest} since last rest).");

                if files_since_rest >= cfg.ingestor.throttle_file_count {
                    info!(
                        "🛏️  Throttle: {} files processed — resting {}s …",
                        files_since_rest, cfg.ingestor.throttle_rest_secs
                    );
                    sleep_interruptible(cfg.ingestor.throttle_rest_secs);
                    files_since_rest = 0;
                    info!("🔄 Throttle rest done. Resuming.");
                } else {
                    info!("Cooldown: {}s …", cfg.ingestor.cooldown_after_batch_secs);
                    sleep_interruptible(cfg.ingestor.cooldown_after_batch_secs);
                }
            }
            Err(e) => {
                error!("Batch error: {e}");
                sleep_interruptible(cfg.ingestor.scan_interval_secs);
            }
        }
    }

    info!("Ingestor stopped. Tracked {} vessels.", edge.vessel_count());
}

// ─── Core ingestion loop ───────────────────────────────────────────────────────

fn scan_and_ingest(
    edge: &mut EdgeState,
    cfg:  &IngestorFile,
) -> Result<usize, Box<dyn std::error::Error>> {
    if !mount_check::ensure_mount_healthy(&cfg.ingestor.mount_point, cfg.mount.check_timeout_secs) {
        return Err("Mount unhealthy — skipping batch".into());
    }

    let mut jsonl_files = list_jsonl_files(&cfg.ingestor.input_dir)?;
    if jsonl_files.is_empty() { return Ok(0); }
    jsonl_files.sort();
    let batch: Vec<PathBuf> = jsonl_files.into_iter().take(cfg.ingestor.files_per_batch).collect();

    info!("────── Batch: {} file(s) ──────", batch.len());

    let batch_start   = Utc::now();
    let read_timeout  = Duration::from_secs(cfg.ingestor.file_read_timeout_secs);
    let buf_cap       = cfg.parquet.buf_reader_capacity;

    let mut pos_buf    = PositionBuf::default();
    let mut draught    = DraughtBuf::default();
    let mut nav        = NavStatusBuf::default();
    let mut dest       = DestinationBuf::default();
    let mut stat       = VesselStaticBuf::default();
    let mut names      = VesselNameHistoryBuf::default();
    let mut health     = HealthCounters::default();
    let mut total_lines: u64 = 0;

    for file_path in &batch {
        if SHUTDOWN.load(Ordering::Relaxed) { break; }
        info!("  Reading: {}", file_path.display());

        // ── Anti-crash file read (timeout protected) ──────────────────────────
        let lines = match read_lines_with_timeout(file_path, read_timeout, buf_cap) {
            ReadLinesResult::Lines(l)      => l,
            ReadLinesResult::Timeout       => {
                warn!("  ⚠ File read timed out — quarantining: {}", file_path.display());
                quarantine_file(file_path, &cfg.ingestor.quarantine_dir);
                continue;
            }
            ReadLinesResult::OpenError(e)  => {
                warn!("  ⚠ Cannot open file ({e}) — quarantining: {}", file_path.display());
                quarantine_file(file_path, &cfg.ingestor.quarantine_dir);
                continue;
            }
        };
        // ─────────────────────────────────────────────────────────────────────

        for line in &lines {
            let line = line.trim();
            if line.is_empty() { continue; }

            total_lines += 1;
            health.total_messages += 1;

            if total_lines % cfg.parquet.log_every_n_messages == 0 {
                info!("  … parsed {total_lines} lines");
            }

            match parser::parse_line(line) {
                ParsedMessage::Position(p) => {
                    health.position_reports += 1;
                    health.unique_mmsis.insert(p.mmsi);

                    if let Some(status) = p.nav_status {
                        if let Some((ns, _)) = edge.check_nav_status(p.mmsi, status) {
                            nav.mmsi.push(p.mmsi); nav.timestamp_s.push(p.timestamp_s);
                            nav.nav_status.push(ns);
                        }
                    }
                    if let Some(ref name) = p.ship_name {
                        if let Some(n) = edge.check_ship_name(p.mmsi, name) {
                            names.mmsi.push(p.mmsi); names.timestamp_s.push(p.timestamp_s);
                            names.ship_name.push(n);
                        }
                    }
                    pos_buf.mmsi.push(p.mmsi); pos_buf.timestamp_s.push(p.timestamp_s);
                    pos_buf.latitude.push(p.latitude); pos_buf.longitude.push(p.longitude);
                    pos_buf.sog.push(p.sog); pos_buf.cog.push(p.cog);
                    pos_buf.heading.push(p.heading); pos_buf.rot.push(p.rot);
                }
                ParsedMessage::Static(s) => {
                    health.static_data_reports += 1;
                    health.unique_mmsis.insert(s.mmsi);
                    process_static(s, edge, &mut draught, &mut dest, &mut stat, &mut names);
                }
                ParsedMessage::Ignored => {}
                ParsedMessage::Error(e) => {
                    health.parse_errors += 1;
                    if health.parse_errors <= 10 { warn!("  Parse error: {e}"); }
                }
            }

            // Early flush to prevent OOM on very large files
            if pos_buf.len() >= cfg.ingestor.max_position_rows_before_flush {
                info!("  ⚠ Position buffer at {} rows — early flush", pos_buf.len());
                let _ = flush_positions(&cfg.ingestor.lake_root, &mut pos_buf, &cfg.parquet);
                sleep_ms_interruptible(200);
            }
        }

        // Breathe between files
        if !SHUTDOWN.load(Ordering::Relaxed) {
            sleep_ms_interruptible(cfg.ingestor.inter_file_sleep_ms);
        }
    }

    // ── Mount check before writing Parquet ──
    if !mount_check::ensure_mount_healthy(&cfg.ingestor.mount_point, cfg.mount.check_timeout_secs) {
        return Err("Mount unhealthy before flush — aborting batch".into());
    }

    // ── Flush all buffers ──
    info!("Flushing to Parquet …");
    let pos_n  = flush_positions(&cfg.ingestor.lake_root, &mut pos_buf, &cfg.parquet)?;
    let dr_n   = flush_draughts(&cfg.ingestor.lake_root, &mut draught, &cfg.parquet)?;
    let nav_n  = flush_nav_statuses(&cfg.ingestor.lake_root, &mut nav, &cfg.parquet)?;
    let dst_n  = flush_destinations(&cfg.ingestor.lake_root, &mut dest, &cfg.parquet)?;
    let sta_n  = flush_vessel_static(&cfg.ingestor.lake_root, &mut stat, &cfg.parquet)?;
    let name_n = flush_vessel_name_history(&cfg.ingestor.lake_root, &mut names, &cfg.parquet)?;

    let window_start = batch_start.timestamp();
    let window_end   = Utc::now().timestamp();
    flush_health(&cfg.ingestor.lake_root, &mut health, window_start, window_end, &cfg.parquet)?;

    info!(
        "Flush: {} pos, {} dr, {} nav, {} dst, {} static, {} names",
        pos_n, dr_n, nav_n, dst_n, sta_n, name_n
    );

    // ── Mount check before moving files ──
    if !mount_check::ensure_mount_healthy(&cfg.ingestor.mount_point, cfg.mount.check_timeout_secs) {
        return Err("Mount unhealthy before file move — aborting move".into());
    }

    // ── Move processed files ──
    for file_path in &batch {
        let fname = file_path.file_name().unwrap();
        let dest  = Path::new(&cfg.ingestor.processed_dir).join(fname);
        match fs::rename(file_path, &dest) {
            Ok(()) => info!("  Moved → {}", dest.display()),
            Err(e) => {
                warn!("  rename failed ({e}), trying copy+delete …");
                match fs::copy(file_path, &dest) {
                    Ok(_) => { fs::remove_file(file_path).ok(); info!("  Copied+deleted."); }
                    Err(e2) => error!("  Failed to move {}: {e2}", file_path.display()),
                }
            }
        }
    }

    Ok(batch.len())
}

// ─── Static record edge processing ────────────────────────────────────────────

fn process_static(
    s:     StaticRecord,
    edge:  &mut EdgeState,
    dr:    &mut DraughtBuf,
    dest:  &mut DestinationBuf,
    stat:  &mut VesselStaticBuf,
    names: &mut VesselNameHistoryBuf,
) {
    if let Some(ref name) = s.ship_name {
        if let Some(n) = edge.check_ship_name(s.mmsi, name) {
            names.mmsi.push(s.mmsi); names.timestamp_s.push(s.timestamp_s);
            names.ship_name.push(n);
        }
    }
    if let Some(d) = s.draught {
        if let Some((nd, _)) = edge.check_draught(s.mmsi, d) {
            dr.mmsi.push(s.mmsi); dr.timestamp_s.push(s.timestamp_s); dr.draught.push(nd);
        }
    }
    if let Some(ref d) = s.destination {
        if let Some((nd, _)) = edge.check_destination(s.mmsi, d) {
            dest.mmsi.push(s.mmsi); dest.timestamp_s.push(s.timestamp_s);
            dest.destination.push(nd); dest.eta_utc.push(s.eta_utc);
        }
    }
    let length = match (s.dim_a, s.dim_b) { (Some(a), Some(b)) if a+b > 0 => Some(a+b), _ => None };
    let beam   = match (s.dim_c, s.dim_d) { (Some(c), Some(d)) if c+d > 0 => Some(c+d), _ => None };
    let key = edge_state::VesselStaticKey {
        call_sign: s.call_sign.clone(), imo_number: s.imo_number,
        ship_type: s.ship_type, length_overall: length, beam,
    };
    if edge.check_vessel_static(s.mmsi, key) {
        stat.mmsi.push(s.mmsi); stat.timestamp_s.push(s.timestamp_s);
        stat.call_sign.push(s.call_sign); stat.imo_number.push(s.imo_number);
        stat.ship_type.push(s.ship_type); stat.length_overall.push(length); stat.beam.push(beam);
    }
}

// ─── Debug single-file mode ────────────────────────────────────────────────────

fn run_debug_single_file(path: &str, cfg: &IngestorFile) {
    let file_path = PathBuf::from(path);
    if !file_path.exists() {
        error!("File not found: {path}");
        std::process::exit(1);
    }

    info!("DEBUG mode — file: {path}");

    let read_timeout = Duration::from_secs(cfg.ingestor.file_read_timeout_secs);
    let buf_cap      = cfg.parquet.buf_reader_capacity;

    let lines = match read_lines_with_timeout(&file_path, read_timeout, buf_cap) {
        ReadLinesResult::Lines(l)     => l,
        ReadLinesResult::Timeout      => { error!("Read timed out!"); std::process::exit(1); }
        ReadLinesResult::OpenError(e) => { error!("Open error: {e}"); std::process::exit(1); }
    };

    let mut edge   = EdgeState::new();
    let mut pos_buf = PositionBuf::default();
    let mut draught = DraughtBuf::default();
    let mut nav     = NavStatusBuf::default();
    let mut dest    = DestinationBuf::default();
    let mut stat    = VesselStaticBuf::default();
    let mut names   = VesselNameHistoryBuf::default();
    let mut health  = HealthCounters::default();

    for (line_num, line) in lines.iter().enumerate() {
        let line = line.trim();
        if line.is_empty() { continue; }
        health.total_messages += 1;

        match parser::parse_line(line) {
            ParsedMessage::Position(p) => {
                health.position_reports += 1;
                health.unique_mmsis.insert(p.mmsi);
                debug!("[L{}] POSITION mmsi={} lat={:.5} lon={:.5}", line_num+1, p.mmsi, p.latitude, p.longitude);

                if let Some(s) = p.nav_status {
                    if let Some((ns, _)) = edge.check_nav_status(p.mmsi, s) {
                        nav.mmsi.push(p.mmsi); nav.timestamp_s.push(p.timestamp_s); nav.nav_status.push(ns);
                    }
                }
                if let Some(ref n) = p.ship_name {
                    if let Some(nn) = edge.check_ship_name(p.mmsi, n) {
                        info!("[L{}] EDGE name mmsi={} → {:?}", line_num+1, p.mmsi, nn);
                        names.mmsi.push(p.mmsi); names.timestamp_s.push(p.timestamp_s); names.ship_name.push(nn);
                    }
                }
                pos_buf.mmsi.push(p.mmsi); pos_buf.timestamp_s.push(p.timestamp_s);
                pos_buf.latitude.push(p.latitude); pos_buf.longitude.push(p.longitude);
                pos_buf.sog.push(p.sog); pos_buf.cog.push(p.cog);
                pos_buf.heading.push(p.heading); pos_buf.rot.push(p.rot);
            }
            ParsedMessage::Static(s) => {
                health.static_data_reports += 1;
                health.unique_mmsis.insert(s.mmsi);
                debug!("[L{}] STATIC  mmsi={} name={:?} draught={:?}", line_num+1, s.mmsi, s.ship_name, s.draught);
                process_static(s, &mut edge, &mut draught, &mut dest, &mut stat, &mut names);
            }
            ParsedMessage::Ignored   => {}
            ParsedMessage::Error(e)  => { health.parse_errors += 1; warn!("[L{}] {e}", line_num+1); }
        }
    }

    info!("Flushing to {} …", cfg.ingestor.lake_root);
    let _ = flush_positions(&cfg.ingestor.lake_root, &mut pos_buf, &cfg.parquet);
    let _ = flush_draughts(&cfg.ingestor.lake_root, &mut draught, &cfg.parquet);
    let _ = flush_nav_statuses(&cfg.ingestor.lake_root, &mut nav, &cfg.parquet);
    let _ = flush_destinations(&cfg.ingestor.lake_root, &mut dest, &cfg.parquet);
    let _ = flush_vessel_static(&cfg.ingestor.lake_root, &mut stat, &cfg.parquet);
    let _ = flush_vessel_name_history(&cfg.ingestor.lake_root, &mut names, &cfg.parquet);

    let now = Utc::now().timestamp();
    let _ = flush_health(&cfg.ingestor.lake_root, &mut health, now - 60, now, &cfg.parquet);

    info!("══ DEBUG SUMMARY ══");
    info!("  Lines:      {}", lines.len());
    info!("  Total msgs: {}", health.total_messages);
    info!("  Positions:  {}", health.position_reports);
    info!("  Static:     {}", health.static_data_reports);
    info!("  Errors:     {}", health.parse_errors);
    info!("  Unique MMSI:{}", health.unique_mmsis.len());
    info!("  Vessels tracked: {}", edge.vessel_count());
}

// ─── Utilities ─────────────────────────────────────────────────────────────────

fn list_jsonl_files(dir: &str) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let p = Path::new(dir);
    if !p.exists() { return Ok(vec![]); }
    let files: Vec<PathBuf> = fs::read_dir(p)?
        .filter_map(|e| {
            let e = e.ok()?;
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("jsonl") { Some(p) } else { None }
        })
        .collect();
    Ok(files)
}

fn sleep_interruptible(secs: u64) {
    for _ in 0..secs {
        if SHUTDOWN.load(Ordering::Relaxed) { return; }
        thread::sleep(Duration::from_secs(1));
    }
}

fn sleep_ms_interruptible(ms: u64) {
    let chunks = ms / 100;
    let rem    = ms % 100;
    for _ in 0..chunks {
        if SHUTDOWN.load(Ordering::Relaxed) { return; }
        thread::sleep(Duration::from_millis(100));
    }
    if rem > 0 && !SHUTDOWN.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(rem));
    }
}
