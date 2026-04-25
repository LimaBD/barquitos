// ─── Runtime Configuration ───────────────────────────────────────────────────
//
// Each binary loads its own config file, containing only its relevant sections.
// This avoids one monolithic file that every binary must understand in full.
//
// Per-crate config files:
//   collector.toml  →  CollectorFile  { collector, mount }
//   ingestor.toml   →  IngestorFile   { ingestor, parquet, mount }
//   mover.toml      →  MoverFile      { mover, mount }
//
// Path resolution order (first match wins):
//   1. --config <path>                 CLI argument
//   2. BARQUITOS_<NAME>_CONFIG         per-crate environment variable
//   3. /etc/barquitos/<name>.toml      system install path
//   4. ./config/<name>.toml            cwd-relative (convenient during dev)
// ─────────────────────────────────────────────────────────────────────────────

use serde::Deserialize;
use std::path::{Path, PathBuf};

// ─── Section structs ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct CollectorConfig {
    pub api_key:                  String,
    pub ws_url:                   String,
    /// Array of bounding boxes: each box is [[SW_lat, SW_lon], [NE_lat, NE_lon]]
    pub bounding_boxes:           Vec<Vec<Vec<f64>>>,
    pub filter_mmsi:              Vec<String>,
    pub filter_message_types:     Vec<String>,
    pub output_dir:               String,
    /// Mount point to verify before each write.  Empty = skip check (for local disk).
    #[serde(default)]
    pub mount_point:              String,
    pub log_every_n_messages:     u64,
    pub reconnect_delay_secs:     u64,
    pub max_reconnect_delay_secs: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MoverConfig {
    pub source_dir:             String,
    pub dest_dir:               String,
    pub dest_mount_point:       String,
    /// How often to run (seconds). 3600 = every hour.
    pub interval_secs:          u64,
    /// Only move files older than this (seconds) — let the collector finish writing.
    pub min_file_age_secs:      u64,
    /// Max seconds to wait for a single file copy before quarantining.
    pub file_copy_timeout_secs: u64,
    pub quarantine_dir:         String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct IngestorConfig {
    pub input_dir:                      String,
    pub processed_dir:                  String,
    pub quarantine_dir:                 String,
    pub lake_root:                      String,
    pub mount_point:                    String,
    pub scan_interval_secs:             u64,
    pub files_per_batch:                usize,
    pub inter_file_sleep_ms:            u64,
    pub cooldown_after_batch_secs:      u64,
    pub max_position_rows_before_flush: usize,
    pub throttle_file_count:            u64,
    pub throttle_rest_secs:             u64,
    /// Max seconds a single file read may take before it is quarantined.
    pub file_read_timeout_secs:         u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ParquetConfig {
    pub row_group_size:       usize,
    pub data_page_size:       usize,
    pub dict_page_size:       usize,
    pub log_every_n_messages: u64,
    pub buf_reader_capacity:  usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MountConfig {
    pub check_timeout_secs:     u64,
    pub watchdog_interval_secs: u64,
    pub max_retries:            u32,
    pub retry_interval_secs:    u64,
}

// ─── Per-crate config file types ──────────────────────────────────────────────

/// Loaded by `ais_collector` from `collector.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct CollectorFile {
    pub collector: CollectorConfig,
    pub mount:     MountConfig,
}

/// Loaded by `ais_ingestor` and `ais_backfill` from `ingestor.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct IngestorFile {
    pub ingestor: IngestorConfig,
    pub parquet:  ParquetConfig,
    pub mount:    MountConfig,
}

/// Loaded by `ais_mover` from `mover.toml`.
#[derive(Debug, Deserialize, Clone)]
pub struct MoverFile {
    pub mover: MoverConfig,
    pub mount: MountConfig,
}

// ─── Config loading ───────────────────────────────────────────────────────────

fn load_toml<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<T, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Cannot read {}: {}", path.display(), e))?;
    let cfg = toml::from_str(&content)
        .map_err(|e| format!("TOML parse error in {}: {}", path.display(), e))?;
    Ok(cfg)
}

fn resolve_config_path(
    args:     &[String],
    env_var:  &str,
    filename: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // 1. --config <path>
    if let Some(pos) = args.iter().position(|a| a == "--config") {
        let p = args
            .get(pos + 1)
            .ok_or("--config flag provided but no path followed")?;
        return Ok(PathBuf::from(p));
    }

    // 2. Per-crate env var (e.g. BARQUITOS_COLLECTOR_CONFIG)
    if let Ok(p) = std::env::var(env_var) {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }

    // 3. /etc/barquitos/<filename>
    let system = PathBuf::from("/etc/barquitos").join(filename);
    if system.exists() {
        return Ok(system);
    }

    // 4. ./config/<filename>  (relative to cwd — convenient when running from pipeline/)
    let local = PathBuf::from("config").join(filename);
    if local.exists() {
        return Ok(local);
    }

    Err(format!(
        "{filename} not found. Set {env_var} env var, pass --config <path>, \
         or place it at /etc/barquitos/{filename}"
    )
    .into())
}

impl CollectorFile {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        load_toml(path)
    }
    pub fn load_from_args_or_env(args: &[String]) -> Result<Self, Box<dyn std::error::Error>> {
        let path = resolve_config_path(args, "BARQUITOS_COLLECTOR_CONFIG", "collector.toml")?;
        log::info!("Config: {}", path.display());
        Self::load(&path)
    }
}

impl IngestorFile {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        load_toml(path)
    }
    pub fn load_from_args_or_env(args: &[String]) -> Result<Self, Box<dyn std::error::Error>> {
        let path = resolve_config_path(args, "BARQUITOS_INGESTOR_CONFIG", "ingestor.toml")?;
        log::info!("Config: {}", path.display());
        Self::load(&path)
    }
}

impl MoverFile {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        load_toml(path)
    }
    pub fn load_from_args_or_env(args: &[String]) -> Result<Self, Box<dyn std::error::Error>> {
        let path = resolve_config_path(args, "BARQUITOS_MOVER_CONFIG", "mover.toml")?;
        log::info!("Config: {}", path.display());
        Self::load(&path)
    }
}

