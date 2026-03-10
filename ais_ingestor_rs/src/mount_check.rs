// ─── Mount-Point Health Check ────────────────────────────────────────────────
//
// Verifies that /mnt/wdblack is mounted and responsive before any I/O.
// Uses a dedicated thread with a 30-second timeout to detect hung mounts
// (e.g. disconnected USB drive, failed SATA link).
//
// Used by ais_ingestor (main.rs) but not by ais_backfill — suppress warnings.
// If the mount is unresponsive, the process exits cleanly rather than
// hanging indefinitely on a blocked I/O call.
// ────────────────────────────────────────────────────────────────────────────

use std::fs;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use log::{error, info, warn};

/// Result of a mount health check.
#[derive(Debug, Clone, PartialEq)]
pub enum MountStatus {
    /// Mount point is accessible and responsive.
    Healthy,
    /// Mount point does not exist or is not a directory.
    NotMounted,
    /// I/O operation timed out — disk is likely hung or disconnected.
    Timeout,
    /// I/O operation returned an error.
    IoError(String),
}

/// Check if a mount point is mounted and responsive.
///
/// Spawns a thread that attempts to `stat` the path and list its contents.
/// If the thread doesn't respond within `timeout_secs`, the mount is
/// considered hung.
pub fn check_mount(mount_point: &str, timeout_secs: u64) -> MountStatus {
    let path = mount_point.to_string();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let result = match fs::metadata(&path) {
            Ok(meta) => {
                if !meta.is_dir() {
                    MountStatus::NotMounted
                } else {
                    // Also try to actually read the directory to ensure it's not stale
                    match fs::read_dir(&path) {
                        Ok(_) => MountStatus::Healthy,
                        Err(e) => MountStatus::IoError(format!("read_dir failed: {e}")),
                    }
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    MountStatus::NotMounted
                } else {
                    MountStatus::IoError(format!("stat failed: {e}"))
                }
            }
        };
        let _ = tx.send(result);
    });

    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(status) => status,
        Err(_) => {
            error!(
                "Mount check TIMED OUT after {}s for {} — disk may be disconnected or hung!",
                timeout_secs, mount_point
            );
            MountStatus::Timeout
        }
    }
}

/// Verify mount is healthy, logging and returning `true` if OK.
pub fn ensure_mount_healthy(mount_point: &str, timeout_secs: u64) -> bool {
    match check_mount(mount_point, timeout_secs) {
        MountStatus::Healthy => true,
        MountStatus::NotMounted => {
            error!("❌ Mount point {} is not mounted!", mount_point);
            false
        }
        MountStatus::Timeout => {
            error!(
                "❌ Mount point {} is UNRESPONSIVE (timed out after {}s)! \
                 Disk may be disconnected or failing.",
                mount_point, timeout_secs
            );
            false
        }
        MountStatus::IoError(e) => {
            error!("❌ Mount point {} I/O error: {}", mount_point, e);
            false
        }
    }
}

/// Wait (with retries) for a mount point to become healthy.
pub fn wait_for_mount(
    mount_point: &str,
    timeout_secs: u64,
    max_retries: u32,
    retry_interval_secs: u64,
) -> bool {
    for attempt in 1..=max_retries {
        if ensure_mount_healthy(mount_point, timeout_secs) {
            if attempt > 1 {
                info!(
                    "✅ Mount {} is healthy again after {} attempts.",
                    mount_point, attempt
                );
            }
            return true;
        }
        if attempt < max_retries {
            warn!(
                "⏳ Mount {} unhealthy (attempt {}/{}). Retrying in {}s …",
                mount_point, attempt, max_retries, retry_interval_secs
            );
            thread::sleep(Duration::from_secs(retry_interval_secs));
        }
    }
    error!(
        "💀 Mount {} still unhealthy after {} attempts. Aborting!",
        mount_point, max_retries
    );
    false
}

/// Start a background watchdog thread that periodically checks the mount.
/// If the mount becomes unresponsive, the watchdog calls `std::process::exit(1)`
/// to prevent the main process from hanging forever on blocked I/O.
pub fn start_watchdog(
    mount_point: &'static str,
    check_timeout_secs: u64,
    interval_secs: u64,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("mount-watchdog".into())
        .spawn(move || {
            info!(
                "🐕 Mount watchdog started: checking {} every {}s (timeout {}s)",
                mount_point, interval_secs, check_timeout_secs
            );
            loop {
                thread::sleep(Duration::from_secs(interval_secs));

                let status = check_mount(mount_point, check_timeout_secs);
                match status {
                    MountStatus::Healthy => {
                        // All good, continue watching
                    }
                    _ => {
                        error!(
                            "💀 WATCHDOG: Mount {} is {:?}! Forcing process exit to prevent hang.",
                            mount_point, status
                        );
                        // Give logs a moment to flush
                        thread::sleep(Duration::from_millis(500));
                        std::process::exit(1);
                    }
                }
            }
        })
        .expect("Failed to spawn mount watchdog thread")
}
