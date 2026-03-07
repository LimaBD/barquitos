/// Configuration constants — mirrors `ais_ingestor/config.py`.
///
/// Paths and settings used by the AIS ingestion binary.

/// Where the AIS collector writes raw JSONL files.
pub const AIS_RAW_DIR: &str = "/mnt/wdblack/ais";

/// After a JSONL file is ingested it is moved here.
pub const AIS_BACKUP_DIR: &str = "/mnt/wdblack/ais_processed";

/// SQLite database file.
pub const DATABASE_PATH: &str = "/mnt/wdblack/barquitos.db";

/// How many JSONL lines to buffer per parallel chunk for intra-file parsing.
pub const PARSE_CHUNK_SIZE: usize = 10_000;

/// JSONL filename glob pattern.
pub const JSONL_GLOB: &str = "ais_*.jsonl";

/// AIS ship-type codes to EXCLUDE (pleasure / recreational).
/// 36 = Sailing vessel, 37 = Pleasure craft / yacht.
pub const EXCLUDED_SHIP_TYPES: &[i64] = &[36, 37];
