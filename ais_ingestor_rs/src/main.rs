/// AIS JSONL → SQLite ingestor (Rust port).
///
/// Drop-in replacement for `python -m ais_ingestor` — same CLI flags,
/// same database, same file-move behaviour, but significantly faster on
/// the Raspberry Pi.

mod config;
mod db;
mod ingest;
mod models;

use std::collections::HashSet;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use clap::Parser;
use log::info;

#[derive(Parser, Debug)]
#[command(name = "ais_ingestor", about = "Ingest AIS JSONL into SQLite")]
struct Args {
    /// Run continuously, checking for new files every N seconds.
    #[arg(long)]
    watch: bool,

    /// Seconds between checks in --watch mode.
    #[arg(long, default_value_t = 30)]
    interval: u64,

    /// Override the raw JSONL input directory.
    #[arg(long = "input-dir")]
    input_dir: Option<String>,

    /// Override the backup directory for processed files.
    #[arg(long = "backup-dir")]
    backup_dir: Option<String>,
}

fn main() {
    // Initialise logger (reads RUST_LOG; defaults to info)
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let args = Args::parse();

    let raw_dir = PathBuf::from(
        args.input_dir
            .as_deref()
            .unwrap_or(config::AIS_RAW_DIR),
    );
    let backup_dir = PathBuf::from(
        args.backup_dir
            .as_deref()
            .unwrap_or(config::AIS_BACKUP_DIR),
    );

    info!("Database : {}", config::DATABASE_PATH);
    info!("Raw dir  : {}", raw_dir.display());
    info!("Backup   : {}", backup_dir.display());

    let mut conn = db::init_db(config::DATABASE_PATH).expect("Failed to initialise database");

    // Persistent set of non-commercial MMSIs — grows across files
    let mut non_commercial_mmsis: HashSet<i64> = HashSet::new();

    if args.watch {
        info!(
            "👀  Watch mode — checking every {} s  (Ctrl+C to stop)",
            args.interval
        );
        loop {
            ingest::run_once(&mut conn, &raw_dir, &backup_dir, &mut non_commercial_mmsis);
            thread::sleep(Duration::from_secs(args.interval));
        }
    } else {
        ingest::run_once(&mut conn, &raw_dir, &backup_dir, &mut non_commercial_mmsis);
    }

    info!("Done.");
}
