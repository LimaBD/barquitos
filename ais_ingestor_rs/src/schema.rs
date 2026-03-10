// ─── Arrow Schemas ──────────────────────────────────────────────────────────
//
// Defines the columnar schemas for every Bronze-layer fact table.
// All timestamps are seconds since Unix epoch, stored as
// `Timestamp(Second, Some("UTC"))` for downstream DuckDB compatibility.
//
// Vessel identity: MMSI is the primary key in every table.  MMSI is always
// present in every AIS message.  IMO numbers are stored in vessel_static
// and can be JOINed when needed.
//
// Coordinate precision: latitude/longitude are rounded to 5 decimal places
// (~1.1 m at the equator) to save storage without losing useful accuracy.
// ────────────────────────────────────────────────────────────────────────────

use arrow_schema::{DataType, Field, Schema, TimeUnit};
use std::sync::Arc;

/// fact_position — high-frequency GPS pings (typically every 15 seconds).
///
/// Partitioned by year/month/day in 15-minute windows, sorted by MMSI
/// within each file for optimal Parquet min/max predicate pushdown.
pub fn position_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi", DataType::UInt32, false),
        Field::new(
            "timestamp_utc",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            false,
        ),
        Field::new("latitude", DataType::Float64, false),
        Field::new("longitude", DataType::Float64, false),
        Field::new("speed_over_ground", DataType::Float32, true),
        Field::new("course_over_ground", DataType::Float32, true),
        Field::new("true_heading", DataType::UInt16, true),
        Field::new("rate_of_turn", DataType::Int16, true),
    ]))
}

/// fact_draught — edge-filtered draught changes.
///
/// One row per draught change per vessel.  Draught values are compared
/// at 0.1 m resolution to avoid false pivots from floating-point noise.
pub fn draught_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi", DataType::UInt32, false),
        Field::new(
            "timestamp_utc",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            false,
        ),
        Field::new("draught", DataType::Float32, false),
    ]))
}

/// fact_navigational_status — edge-filtered nav status changes.
///
/// One row per navigational status change per vessel.
pub fn nav_status_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi", DataType::UInt32, false),
        Field::new(
            "timestamp_utc",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            false,
        ),
        Field::new("navigational_status", DataType::UInt8, false),
    ]))
}

/// fact_destination — edge-filtered destination changes.
///
/// One row per destination change per vessel.  ETA fields (month, day,
/// hour, minute) are merged into a single nullable UTC timestamp.
pub fn destination_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi", DataType::UInt32, false),
        Field::new(
            "timestamp_utc",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            false,
        ),
        Field::new("destination", DataType::Utf8, false),
        Field::new(
            "eta_utc",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            true,
        ),
    ]))
}

/// dim_vessel — edge-filtered vessel identity and dimensions.
///
/// Stores the slowly-changing "registry" attributes of a vessel.
/// A new row is written only when any field changes for a given MMSI.
/// Not partitioned by time — stored as a single flat file since the
/// volume is low (~1 row per vessel, rarely changing).
///
/// `length_overall` and `beam` are pre-computed from raw AIS dimension
/// fields (A+B and C+D respectively).  Raw dimensions are not stored
/// because the computed values are what analytics needs.
///
/// Draught is not stored here — it changes frequently and is tracked
/// in its own edge-filtered table (`fact_draught`).
pub fn vessel_static_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi", DataType::UInt32, false),
        Field::new(
            "timestamp_utc",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            false,
        ),
        Field::new("call_sign", DataType::Utf8, true),
        Field::new("imo_number", DataType::UInt32, true),
        Field::new("ship_type", DataType::UInt8, true),
        Field::new("length_overall", DataType::UInt16, true),
        Field::new("beam", DataType::UInt16, true),
    ]))
}

/// fact_vessel_name_history — edge-filtered vessel name changes.
///
/// One row per name change per MMSI.  The first observation for a vessel
/// is always recorded.  Subsequent rows are emitted only when the name
/// actually changes (trimmed, uppercased comparison).
pub fn vessel_name_history_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi", DataType::UInt32, false),
        Field::new(
            "timestamp_utc",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            false,
        ),
        Field::new("ship_name", DataType::Utf8, false),
    ]))
}

/// metric_system_health — telemetry per micro-batch window.
///
/// One row per ingestion batch.  Tracks throughput and error rates.
pub fn health_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(
            "window_start",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            false,
        ),
        Field::new(
            "window_end",
            DataType::Timestamp(TimeUnit::Second, Some("UTC".into())),
            false,
        ),
        Field::new("position_reports", DataType::UInt32, false),
        Field::new("static_data_reports", DataType::UInt32, false),
        Field::new("total_messages", DataType::UInt32, false),
        Field::new("unique_mmsi_count", DataType::UInt32, false),
        Field::new("parse_errors", DataType::UInt32, false),
    ]))
}
