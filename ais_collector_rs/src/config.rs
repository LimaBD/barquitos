// ─── Configuration ──────────────────────────────────────────────────────────
//
// All tunables for the AIS Collector in Rust.
// Connects to AISstream.io WebSocket and writes raw JSONL files.
// ────────────────────────────────────────────────────────────────────────────

#![allow(dead_code)]

/// Your AISstream.io API key.
/// Can be overridden with AISSTREAM_API_KEY environment variable.
pub const API_KEY: &str = "831f2673b69d3426d33f41b455bd3f3ba9b1288f";

/// AISstream WebSocket endpoint.
pub const WS_URL: &str = "wss://stream.aisstream.io/v0/stream";

/// Bounding boxes: [[lat_min, lon_min], [lat_max, lon_max]]
/// Default: entire world.
pub const BOUNDING_BOXES: &[[[f64; 2]; 2]] = &[[[-90.0, -180.0], [90.0, 180.0]]];

/// Filter by specific MMSIs (empty = all).
pub const FILTER_MMSI: &[&str] = &[];

/// Filter by message types (empty = all).
pub const FILTER_MESSAGE_TYPES: &[&str] = &[];

/// Directory where raw JSONL files are written.
pub const DATA_OUTPUT_DIR: &str = "/mnt/wdblack/ais";

/// Mount point to verify before writing.
pub const MOUNT_POINT: &str = "/mnt/wdblack";

/// Maximum messages per file before rotating (0 = unlimited, rotate on minute only).
pub const MAX_MESSAGES_PER_FILE: u64 = 0;

/// Initial reconnect delay in seconds.
pub const RECONNECT_DELAY_SECS: u64 = 5;

/// Maximum reconnect delay (exponential backoff cap) in seconds.
pub const MAX_RECONNECT_DELAY_SECS: u64 = 300;

/// Log a status line every N messages received.
pub const LOG_EVERY_N_MESSAGES: u64 = 1000;

/// Timeout in seconds to verify mount point is responsive.
pub const MOUNT_CHECK_TIMEOUT_SECS: u64 = 30;

/// How often (seconds) the watchdog checks mount health.
pub const WATCHDOG_INTERVAL_SECS: u64 = 60;
