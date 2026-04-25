// ─── AIS File Mover ───────────────────────────────────────────────────────────
//
// Runs every `interval_secs` (default 3600 = 1 hour).
// Moves completed JSONL files from the collector's local disk to the WD Black
// so the ingestor can pick them up.
//
// Anti-crash features:
//   • Each file copy runs in a dedicated thread with `file_copy_timeout_secs`.
//     If a bad sector on either disk causes the copy to hang, the file is
//     quarantined and the mover continues with the next one.
//   • Mount health of the destination disk is checked before any copy.
//   • A background watchdog exits the process if the destination disk freezes.
//   • Only files older than `min_file_age_secs` are moved (prevents moving a
//     file the collector is still writing to).
// ─────────────────────────────────────────────────────────────────────────────

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, SystemTime};

use log::{error, info, warn};

use barquitos_common::config::MoverFile;
use barquitos_common::mount_check;
use barquitos_common::safe_io::{copy_file_with_timeout, quarantine_file, CopyResult};

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() {
    let args: Vec<String> = std::env::args().collect();

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let cfg = MoverFile::load_from_args_or_env(&args).unwrap_or_else(|e| {
        error!("Failed to load config: {e}");
        std::process::exit(1);
    });

    info!("═══════════════════════════════════════════════════════════════");
    info!("  AIS File Mover v{}", env!("CARGO_PKG_VERSION"));
    info!("  Source     : {}", cfg.mover.source_dir);
    info!("  Dest       : {}", cfg.mover.dest_dir);
    info!("  Mount      : {}", cfg.mover.dest_mount_point);
    info!("  Interval   : {}s ({:.0} min)", cfg.mover.interval_secs, cfg.mover.interval_secs as f64 / 60.0);
    info!("  Min age    : {}s", cfg.mover.min_file_age_secs);
    info!("  Copy timeout: {}s (anti-crash)", cfg.mover.file_copy_timeout_secs);
    info!("  Quarantine : {}", cfg.mover.quarantine_dir);
    info!("═══════════════════════════════════════════════════════════════");

    // ── Pre-flight: check destination mount ──
    if !mount_check::wait_for_mount(
        &cfg.mover.dest_mount_point,
        cfg.mount.check_timeout_secs,
        cfg.mount.max_retries,
        cfg.mount.retry_interval_secs,
    ) {
        error!("💀 Destination mount {} unavailable. Aborting.", cfg.mover.dest_mount_point);
        std::process::exit(1);
    }
    info!("✅ Destination mount {} healthy.", cfg.mover.dest_mount_point);

    // ── Watchdog ──
    let _watchdog = mount_check::start_watchdog(
        cfg.mover.dest_mount_point.clone(),
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
    fs::create_dir_all(&cfg.mover.source_dir).ok();
    fs::create_dir_all(&cfg.mover.dest_dir).ok();
    fs::create_dir_all(&cfg.mover.quarantine_dir).ok();

    // ── Main loop: run once immediately, then every interval ──
    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            info!("Mover stopped.");
            break;
        }

        run_move_cycle(&cfg);

        info!("Sleeping for {}s …", cfg.mover.interval_secs);
        sleep_interruptible(cfg.mover.interval_secs);
    }
}

fn run_move_cycle(cfg: &MoverFile) {
    info!("──── Move cycle ────");

    // Check destination mount before doing any work
    if !mount_check::ensure_mount_healthy(
        &cfg.mover.dest_mount_point,
        cfg.mount.check_timeout_secs,
    ) {
        error!("Destination mount unhealthy — skipping this cycle.");
        let recovered = mount_check::wait_for_mount(
            &cfg.mover.dest_mount_point,
            cfg.mount.check_timeout_secs,
            3,
            30,
        );
        if !recovered {
            error!("Mount still unhealthy after retries — skipping cycle.");
            return;
        }
    }

    // List JSONL files in source dir
    let files = match list_jsonl_files(&cfg.mover.source_dir) {
        Ok(f)  => f,
        Err(e) => { error!("Cannot list source dir: {e}"); return; }
    };

    if files.is_empty() {
        info!("No files to move.");
        return;
    }

    let min_age  = Duration::from_secs(cfg.mover.min_file_age_secs);
    let copy_tmo = Duration::from_secs(cfg.mover.file_copy_timeout_secs);
    let now      = SystemTime::now();

    let mut moved    = 0u32;
    let mut skipped  = 0u32;
    let mut quarantined = 0u32;

    for src in &files {
        if SHUTDOWN.load(Ordering::Relaxed) { break; }

        // ── Age check: skip files too new (collector may still be writing) ──
        let age = match src.metadata().and_then(|m| m.modified()) {
            Ok(mtime) => now.duration_since(mtime).unwrap_or(Duration::ZERO),
            Err(_)    => Duration::ZERO,
        };
        if age < min_age {
            skipped += 1;
            continue;
        }

        let fname = match src.file_name() {
            Some(n) => n,
            None    => continue,
        };
        let dest = PathBuf::from(&cfg.mover.dest_dir).join(fname);

        // Skip if already exists at destination (previous partial run)
        if dest.exists() {
            info!("  Already at dest — removing source: {}", src.display());
            fs::remove_file(src).ok();
            continue;
        }

        // ── Anti-crash copy (timeout protected) ──────────────────────────────
        match copy_file_with_timeout(src, &dest, copy_tmo) {
            CopyResult::Ok(bytes) => {
                // Verify the copy by checking file size
                let src_size  = src.metadata().map(|m| m.len()).unwrap_or(0);
                let dest_size = dest.metadata().map(|m| m.len()).unwrap_or(0);

                if src_size == dest_size || bytes > 0 {
                    if let Err(e) = fs::remove_file(src) {
                        warn!("  Copied but failed to remove source {}: {e}", src.display());
                    } else {
                        info!("  Moved: {} ({} bytes)", src.file_name().unwrap().to_string_lossy(), bytes);
                        moved += 1;
                    }
                } else {
                    // Size mismatch — something went wrong
                    warn!(
                        "  Size mismatch after copy ({src_size} vs {dest_size}) — quarantining: {}",
                        src.display()
                    );
                    fs::remove_file(&dest).ok(); // remove incomplete dest
                    quarantine_file(src, &cfg.mover.quarantine_dir);
                    quarantined += 1;
                }
            }
            CopyResult::Timeout => {
                warn!("  ⚠ Copy timed out — quarantining: {}", src.display());
                fs::remove_file(&dest).ok(); // remove potentially incomplete dest
                quarantine_file(src, &cfg.mover.quarantine_dir);
                quarantined += 1;
            }
            CopyResult::Error(e) => {
                warn!("  ⚠ Copy error ({e}) — quarantining: {}", src.display());
                fs::remove_file(&dest).ok();
                quarantine_file(src, &cfg.mover.quarantine_dir);
                quarantined += 1;
            }
        }
        // ─────────────────────────────────────────────────────────────────────

        // Brief pause between files to avoid hammering the disks
        thread::sleep(Duration::from_millis(200));
    }

    info!(
        "Cycle done: {} moved, {} skipped (too new), {} quarantined",
        moved, skipped, quarantined
    );
}

fn list_jsonl_files(dir: &str) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let p = Path::new(dir);
    if !p.exists() { return Ok(vec![]); }
    let mut files: Vec<PathBuf> = fs::read_dir(p)?
        .filter_map(|e| {
            let e = e.ok()?;
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("jsonl") { Some(p) } else { None }
        })
        .collect();
    files.sort();
    Ok(files)
}

fn sleep_interruptible(secs: u64) {
    for _ in 0..secs {
        if SHUTDOWN.load(Ordering::Relaxed) { return; }
        thread::sleep(Duration::from_secs(1));
    }
}
