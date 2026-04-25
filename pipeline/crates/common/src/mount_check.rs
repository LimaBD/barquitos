// ─── Mount-Point Health Check ────────────────────────────────────────────────
//
// Verifies that a disk mount is present and responsive before any I/O.
//
// All checks run in a separate thread with a channel timeout so a frozen
// or disconnected drive never blocks the main process.  If a mount becomes
// unresponsive the background watchdog calls process::exit(1) to force a
// clean restart rather than hanging forever.
// ─────────────────────────────────────────────────────────────────────────────

use std::fs;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use log::{error, info, warn};

/// Result of a single mount health probe.
#[derive(Debug, Clone, PartialEq)]
pub enum MountStatus {
    Healthy,
    NotMounted,
    /// Kernel I/O call did not return within the timeout — disk likely hung.
    Timeout,
    IoError(String),
}

/// Probe `mount_point` in a dedicated thread.  Returns `Timeout` if the
/// thread doesn't respond within `timeout_secs` (kernel hang on bad disk).
pub fn check_mount(mount_point: &str, timeout_secs: u64) -> MountStatus {
    let path = mount_point.to_owned();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let status = match fs::metadata(&path) {
            Ok(meta) => {
                if !meta.is_dir() {
                    MountStatus::NotMounted
                } else {
                    match fs::read_dir(&path) {
                        Ok(_)  => MountStatus::Healthy,
                        Err(e) => MountStatus::IoError(format!("read_dir: {e}")),
                    }
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    MountStatus::NotMounted
                } else {
                    MountStatus::IoError(format!("stat: {e}"))
                }
            }
        };
        let _ = tx.send(status);
    });

    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(status) => status,
        Err(_) => {
            error!(
                "Mount check TIMED OUT after {}s for {} — disk may be hung!",
                timeout_secs, mount_point
            );
            MountStatus::Timeout
        }
    }
}

/// Log and return `true` iff the mount is healthy.
pub fn ensure_mount_healthy(mount_point: &str, timeout_secs: u64) -> bool {
    match check_mount(mount_point, timeout_secs) {
        MountStatus::Healthy => true,
        MountStatus::NotMounted => {
            error!("❌ {} is not mounted", mount_point);
            false
        }
        MountStatus::Timeout => {
            error!("❌ {} UNRESPONSIVE ({}s timeout)", mount_point, timeout_secs);
            false
        }
        MountStatus::IoError(e) => {
            error!("❌ {} I/O error: {}", mount_point, e);
            false
        }
    }
}

/// Retry `check_mount` up to `max_retries` times.  Returns `true` when healthy.
pub fn wait_for_mount(
    mount_point: &str,
    timeout_secs: u64,
    max_retries: u32,
    retry_interval_secs: u64,
) -> bool {
    for attempt in 1..=max_retries {
        if ensure_mount_healthy(mount_point, timeout_secs) {
            if attempt > 1 {
                info!("✅ {} healthy after {} attempt(s)", mount_point, attempt);
            }
            return true;
        }
        if attempt < max_retries {
            warn!(
                "⏳ {} unhealthy ({}/{}). Retrying in {}s …",
                mount_point, attempt, max_retries, retry_interval_secs
            );
            thread::sleep(Duration::from_secs(retry_interval_secs));
        }
    }
    error!("💀 {} still unhealthy after {} attempts. Aborting!", mount_point, max_retries);
    false
}

/// Spawn a background watchdog thread.  Every `interval_secs` it checks the
/// mount; if unhealthy it calls `process::exit(1)` so systemd can restart the
/// service cleanly rather than leaving it hung forever on blocked I/O.
///
/// Takes an owned `String` so callers can use a runtime-loaded config value.
pub fn start_watchdog(
    mount_point: String,
    check_timeout_secs: u64,
    interval_secs: u64,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("mount-watchdog".into())
        .spawn(move || {
            info!(
                "🐕 Watchdog: {} every {}s (timeout {}s)",
                mount_point, interval_secs, check_timeout_secs
            );
            loop {
                thread::sleep(Duration::from_secs(interval_secs));
                let status = check_mount(&mount_point, check_timeout_secs);
                if status != MountStatus::Healthy {
                    error!(
                        "💀 WATCHDOG: {} is {:?} — forcing exit to prevent hang.",
                        mount_point, status
                    );
                    thread::sleep(Duration::from_millis(500)); // flush logs
                    std::process::exit(1);
                }
            }
        })
        .expect("Failed to spawn mount-watchdog thread")
}
