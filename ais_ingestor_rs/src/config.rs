// ─── Configuration ──────────────────────────────────────────────────────────
//
// All tunables for the AIS Data Lake file-based ingestion pipeline.
// The collector (Python) writes JSONL files to AIS_INPUT_DIR.
// This ingestor reads them, writes Parquet to LAKE_ROOT, then moves
// processed files to AIS_PROCESSED_DIR.
//
// Tuned for a desktop i7 / 16 GB RAM with a mechanical HDD.
// The HDD is the bottleneck — the mandatory cooldown between batches
// ensures the collector, shell, and OS always get I/O time.
// ────────────────────────────────────────────────────────────────────────────

// Some constants are only used by the daemon (ais_ingestor) and not by the
// backfill tool, but they live here as a shared config module.
#![allow(dead_code)]

/// Root of the Parquet Data Lake on the mounted mechanical drive.
pub const LAKE_ROOT: &str = "/mnt/wdblack/trading_lake";

/// Directory where the Python collector writes raw JSONL files.
pub const AIS_INPUT_DIR: &str = "/mnt/wdblack/ais";

/// Directory where processed JSONL files are moved after ingestion.
pub const AIS_PROCESSED_DIR: &str = "/mnt/wdblack/ais_processed";

/// How often (in seconds) to scan for new JSONL files when the queue is empty.
pub const SCAN_INTERVAL_SECS: u64 = 30;

/// How many JSONL files to ingest per micro-batch before flushing to Parquet.
/// Kept small so the HDD is free for the collector and other programs.
pub const FILES_PER_BATCH: usize = 3;

/// Maximum number of position rows to buffer before forcing an early flush.
/// 200 000 rows ≈ ~40 MB of Arrow buffers — smaller batches = less pressure.
pub const MAX_POSITION_ROWS_BEFORE_FLUSH: usize = 200_000;

/// Milliseconds to sleep between processing individual files within a batch.
/// 3 seconds between files — generous breathing room for the HDD.
pub const INTER_FILE_SLEEP_MS: u64 = 3_000;

/// ⭐ MANDATORY COOLDOWN (seconds) after every batch — even when files remain.
/// With 3 files/batch + 50s cooldown ≈ 3 files/minute.
/// The collector writes 1 file/minute, so we catch up gradually without
/// starving other processes of HDD I/O.
pub const COOLDOWN_AFTER_BATCH_SECS: u64 = 50;

/// Parquet writer settings ────────────────────────────────────────────────

/// Maximum Parquet row group size.  Larger = better compression, more RAM.
pub const PARQUET_ROW_GROUP_SIZE: usize = 65_536;

/// Parquet data page size in bytes (256 KB — good for desktop).
pub const PARQUET_DATA_PAGE_SIZE: usize = 256 * 1024;

/// Parquet dictionary page size in bytes (512 KB — good for desktop).
pub const PARQUET_DICT_PAGE_SIZE: usize = 512 * 1024;

/// Log a status line every N messages parsed.
pub const LOG_EVERY_N_MESSAGES: u64 = 100_000;

/// BufReader capacity for reading JSONL files.
/// 256 KB — large enough to amortise HDD seeks on the mechanical drive.
pub const BUF_READER_CAPACITY: usize = 256 * 1024;

// ─── Mount-Point Safety ─────────────────────────────────────────────────

/// The mount point that must be verified before any read/write.
pub const MOUNT_POINT: &str = "/mnt/wdblack";

/// Timeout in seconds to verify mount point is responsive.
/// If stat()/readdir() doesn't respond within this window, the mount is
/// considered hung (disconnected HDD, failed SATA link, etc.).
pub const MOUNT_CHECK_TIMEOUT_SECS: u64 = 30;

/// How often (seconds) the background watchdog thread checks mount health.
pub const WATCHDOG_INTERVAL_SECS: u64 = 60;

// ─── File-Count Throttling ──────────────────────────────────────────────

/// After processing this many files, take a long rest to let the HDD and
/// system recover.  ~60 files ≈ 20 batches ≈ ~20 minutes of work.
pub const THROTTLE_FILE_COUNT: u64 = 60;

/// Duration (in seconds) of the rest period after THROTTLE_FILE_COUNT files.
/// 5 minutes = 300 seconds.
pub const THROTTLE_REST_SECS: u64 = 300;
