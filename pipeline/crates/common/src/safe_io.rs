// ─── Safe (Timeout-Protected) File I/O ──────────────────────────────────────
//
// On a mechanical hard drive with bad sectors, a file open or read can block
// the calling thread *indefinitely* — the kernel retries I/O at the block
// layer and never returns an error.  This module executes all file I/O in a
// dedicated thread and uses a channel with a deadline to protect the caller.
//
// If the file doesn't respond within the deadline:
//   • `read_lines_with_timeout` returns `ReadLinesResult::Timeout`
//   • The blocked thread is abandoned (it will eventually die when the kernel
//     gives up or the drive is disconnected — this is unavoidable in Rust
//     since threads cannot be force-killed).
//   • The caller should quarantine the file with `quarantine_file()` and
//     continue with the next one.
//
// The watchdog in `mount_check` independently handles the case where the
// *entire drive* freezes by calling process::exit(1).
// ─────────────────────────────────────────────────────────────────────────────

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use log::{error, warn};

// ─── Read lines with timeout ─────────────────────────────────────────────────

/// Result returned by `read_lines_with_timeout`.
pub enum ReadLinesResult {
    /// Lines from the file (possibly partial if an I/O error occurred mid-read).
    Lines(Vec<String>),
    /// The reader thread did not finish within the deadline — likely a bad sector.
    Timeout,
    /// File could not be opened (not a hang — an immediate error).
    OpenError(String),
}

/// Read every line of `path` in a separate thread, returning within `timeout`.
///
/// If the read thread does not respond in time, `Timeout` is returned and the
/// thread is abandoned.  The file should then be passed to `quarantine_file()`.
pub fn read_lines_with_timeout(
    path: &Path,
    timeout: Duration,
    buf_capacity: usize,
) -> ReadLinesResult {
    let path_owned = path.to_path_buf();
    let (tx, rx) = mpsc::channel::<ReadLinesResult>();

    thread::Builder::new()
        .name("file-reader".into())
        .spawn(move || {
            let result = read_lines_inner(&path_owned, buf_capacity);
            let _ = tx.send(result);
        })
        .expect("Failed to spawn file-reader thread");

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            warn!(
                "⚠ File read TIMED OUT after {:.0}s: {} — likely a bad sector.",
                timeout.as_secs_f64(),
                path.display()
            );
            ReadLinesResult::Timeout
        }
    }
}

fn read_lines_inner(path: &Path, buf_capacity: usize) -> ReadLinesResult {
    let file = match fs::File::open(path) {
        Ok(f)  => f,
        Err(e) => return ReadLinesResult::OpenError(format!("{e}")),
    };

    let reader = BufReader::with_capacity(buf_capacity, file);
    let mut lines = Vec::new();

    for line_result in reader.lines() {
        match line_result {
            Ok(l)  => lines.push(l),
            Err(e) => {
                // I/O error mid-read (bad sector reached partway through).
                // Return whatever lines we collected so far; the error is
                // logged at the call site via the HealthCounters.
                warn!("I/O error mid-read: {e} — returning {} partial lines", lines.len());
                break;
            }
        }
    }

    ReadLinesResult::Lines(lines)
}

// ─── File copy with timeout ───────────────────────────────────────────────────

/// Result returned by `copy_file_with_timeout`.
pub enum CopyResult {
    /// Bytes copied.
    Ok(u64),
    /// The copy thread did not finish within the deadline.
    Timeout,
    /// Copy failed immediately (src not found, permissions, etc.).
    Error(String),
}

/// Copy `src` to `dest` in a separate thread, returning within `timeout`.
///
/// On success the destination file exists and the source should then be removed
/// by the caller.  On `Timeout`, the source should be quarantined instead.
pub fn copy_file_with_timeout(src: &Path, dest: &Path, timeout: Duration) -> CopyResult {
    let src_owned  = src.to_path_buf();
    let dest_owned = dest.to_path_buf();
    let (tx, rx)   = mpsc::channel::<CopyResult>();

    thread::Builder::new()
        .name("file-copier".into())
        .spawn(move || {
            // Ensure destination parent exists
            if let Some(parent) = dest_owned.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    let _ = tx.send(CopyResult::Error(format!("create_dir_all: {e}")));
                    return;
                }
            }
            let result = match fs::copy(&src_owned, &dest_owned) {
                Ok(n)  => CopyResult::Ok(n),
                Err(e) => CopyResult::Error(format!("{e}")),
            };
            let _ = tx.send(result);
        })
        .expect("Failed to spawn file-copier thread");

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            warn!(
                "⚠ File copy TIMED OUT after {:.0}s: {} → {}",
                timeout.as_secs_f64(),
                src.display(),
                dest.display()
            );
            CopyResult::Timeout
        }
    }
}

// ─── Quarantine ───────────────────────────────────────────────────────────────

/// Move a problematic file to `quarantine_dir` so it is never retried.
/// The quarantine directory is created if it does not exist.
/// Returns `true` on success.
pub fn quarantine_file(file_path: &Path, quarantine_dir: &str) -> bool {
    if let Err(e) = fs::create_dir_all(quarantine_dir) {
        error!("Cannot create quarantine dir {quarantine_dir}: {e}");
        return false;
    }

    let filename = match file_path.file_name() {
        Some(n) => n,
        None    => { error!("Cannot get filename for {}", file_path.display()); return false; }
    };

    // Add a timestamp prefix so name collisions don't happen
    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let dest_name = format!("{ts}_{}", filename.to_string_lossy());
    let dest = PathBuf::from(quarantine_dir).join(&dest_name);

    match fs::rename(file_path, &dest) {
        Ok(()) => {
            warn!("🔒 Quarantined: {} → {}", file_path.display(), dest.display());
            true
        }
        Err(e) => {
            // rename fails across filesystems; fall back to copy + delete
            match fs::copy(file_path, &dest) {
                Ok(_) => {
                    fs::remove_file(file_path).ok();
                    warn!("🔒 Quarantined (copy): {} → {}", file_path.display(), dest.display());
                    true
                }
                Err(e2) => {
                    error!(
                        "Failed to quarantine {}: rename={e}, copy={e2}",
                        file_path.display()
                    );
                    false
                }
            }
        }
    }
}
