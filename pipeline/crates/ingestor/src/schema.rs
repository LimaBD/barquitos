// ─── Arrow Schemas ────────────────────────────────────────────────────────────
// Migrated unchanged from ais_ingestor_rs/src/schema.rs
// ─────────────────────────────────────────────────────────────────────────────

use arrow_schema::{DataType, Field, Schema, TimeUnit};
use std::sync::Arc;

pub fn position_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi",               DataType::UInt32,  false),
        Field::new("timestamp_utc",      DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), false),
        Field::new("latitude",           DataType::Float64, false),
        Field::new("longitude",          DataType::Float64, false),
        Field::new("speed_over_ground",  DataType::Float32, true),
        Field::new("course_over_ground", DataType::Float32, true),
        Field::new("true_heading",       DataType::UInt16,  true),
        Field::new("rate_of_turn",       DataType::Int16,   true),
    ]))
}

pub fn draught_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi",          DataType::UInt32,  false),
        Field::new("timestamp_utc", DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), false),
        Field::new("draught",       DataType::Float32, false),
    ]))
}

pub fn nav_status_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi",                DataType::UInt32, false),
        Field::new("timestamp_utc",       DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), false),
        Field::new("navigational_status", DataType::UInt8,  false),
    ]))
}

pub fn destination_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi",          DataType::UInt32, false),
        Field::new("timestamp_utc", DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), false),
        Field::new("destination",   DataType::Utf8,   false),
        Field::new("eta_utc",       DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), true),
    ]))
}

pub fn vessel_static_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi",           DataType::UInt32,  false),
        Field::new("timestamp_utc",  DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), false),
        Field::new("call_sign",      DataType::Utf8,    true),
        Field::new("imo_number",     DataType::UInt32,  true),
        Field::new("ship_type",      DataType::UInt8,   true),
        Field::new("length_overall", DataType::UInt16,  true),
        Field::new("beam",           DataType::UInt16,  true),
    ]))
}

pub fn vessel_name_history_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("mmsi",          DataType::UInt32, false),
        Field::new("timestamp_utc", DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), false),
        Field::new("ship_name",     DataType::Utf8,   false),
    ]))
}

pub fn health_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("window_start",         DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), false),
        Field::new("window_end",           DataType::Timestamp(TimeUnit::Second, Some("UTC".into())), false),
        Field::new("position_reports",     DataType::UInt32, false),
        Field::new("static_data_reports",  DataType::UInt32, false),
        Field::new("total_messages",       DataType::UInt32, false),
        Field::new("unique_mmsi_count",    DataType::UInt32, false),
        Field::new("parse_errors",         DataType::UInt32, false),
    ]))
}
