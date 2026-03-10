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
/// With 16 GB RAM we can afford larger batches (15 files ≈ ~200 MB).
pub const FILES_PER_BATCH: usize = 15;

/// Maximum number of position rows to buffer before forcing an early flush.
/// 500 000 rows ≈ ~100 MB of Arrow buffers — comfortable for 16 GB.
pub const MAX_POSITION_ROWS_BEFORE_FLUSH: usize = 500_000;

/// Milliseconds to sleep between processing individual files within a batch.
/// Gives the HDD head a moment to service other I/O requests.
pub const INTER_FILE_SLEEP_MS: u64 = 200;

/// ⭐ MANDATORY COOLDOWN (seconds) after every batch — even when files remain.
/// This is the KEY fix: without it, the ingestor runs a non-stop tight loop
/// when there is a large backlog (you have 5 000+ files), starving the
/// collector, shell, and every other process of HDD I/O.
/// 3 seconds guarantees other processes can read/write the disk.
pub const COOLDOWN_AFTER_BATCH_SECS: u64 = 3;

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
