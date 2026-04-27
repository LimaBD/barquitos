// ─── Parquet Writer ───────────────────────────────────────────────────────────
//
// Builds Arrow RecordBatches from in-memory column buffers and writes them to
// Hive-partitioned Parquet files.
//
// All flush functions accept `pq: &ParquetConfig` instead of importing
// hard-coded constants so tuning parameters can be changed in config.toml
// without recompiling.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::*;
use arrow_ord::sort::sort_to_indices;
use arrow_schema::Schema;
use arrow_select::take::take;
use chrono::{DateTime, Datelike, TimeZone, Timelike, Utc};
use log::info;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding, ZstdLevel};
use parquet::file::properties::WriterProperties;

use barquitos_common::config::ParquetConfig;
use crate::parser::TsSeconds;
use crate::schema;

// ─── In-memory column buffers ─────────────────────────────────────────────────

#[derive(Default)]
pub struct PositionBuf {
    pub mmsi:        Vec<u32>,
    pub timestamp_s: Vec<i64>,
    pub latitude:    Vec<f64>,
    pub longitude:   Vec<f64>,
    pub sog:         Vec<Option<f32>>,
    pub cog:         Vec<Option<f32>>,
    pub heading:     Vec<Option<u16>>,
    pub rot:         Vec<Option<i16>>,
}
impl PositionBuf {
    pub fn len(&self)      -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool  { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear(); self.timestamp_s.clear();
        self.latitude.clear(); self.longitude.clear();
        self.sog.clear(); self.cog.clear();
        self.heading.clear(); self.rot.clear();
    }
}

#[derive(Default)]
pub struct DraughtBuf {
    pub mmsi: Vec<u32>, pub timestamp_s: Vec<i64>, pub draught: Vec<f32>,
}
impl DraughtBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) { self.mmsi.clear(); self.timestamp_s.clear(); self.draught.clear(); }
}

#[derive(Default)]
pub struct NavStatusBuf {
    pub mmsi: Vec<u32>, pub timestamp_s: Vec<i64>, pub nav_status: Vec<u8>,
}
impl NavStatusBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) { self.mmsi.clear(); self.timestamp_s.clear(); self.nav_status.clear(); }
}

#[derive(Default)]
pub struct DestinationBuf {
    pub mmsi: Vec<u32>, pub timestamp_s: Vec<i64>,
    pub destination: Vec<String>, pub eta_utc: Vec<Option<i64>>,
}
impl DestinationBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear(); self.timestamp_s.clear();
        self.destination.clear(); self.eta_utc.clear();
    }
}

#[derive(Default)]
pub struct VesselStaticBuf {
    pub mmsi: Vec<u32>, pub timestamp_s: Vec<i64>,
    pub call_sign: Vec<Option<String>>, pub imo_number: Vec<Option<u32>>,
    pub ship_type: Vec<Option<u8>>, pub length_overall: Vec<Option<u16>>,
    pub beam: Vec<Option<u16>>,
}
impl VesselStaticBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear(); self.timestamp_s.clear();
        self.call_sign.clear(); self.imo_number.clear();
        self.ship_type.clear(); self.length_overall.clear(); self.beam.clear();
    }
}

#[derive(Default)]
pub struct VesselNameHistoryBuf {
    pub mmsi: Vec<u32>, pub timestamp_s: Vec<i64>, pub ship_name: Vec<String>,
}
impl VesselNameHistoryBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) { self.mmsi.clear(); self.timestamp_s.clear(); self.ship_name.clear(); }
}

#[derive(Default)]
pub struct HealthCounters {
    pub position_reports:     u32,
    pub static_data_reports:  u32,
    pub total_messages:       u32,
    pub unique_mmsis:         HashSet<u32>,
    pub parse_errors:         u32,
}
impl HealthCounters {
    #[allow(dead_code)]
    pub fn clear(&mut self) {
        self.position_reports = 0; self.static_data_reports = 0;
        self.total_messages = 0; self.unique_mmsis.clear(); self.parse_errors = 0;
    }
}

// ─── WriterProperties builders ────────────────────────────────────────────────

fn writer_props_bloom(pq: &ParquetConfig) -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
        .set_max_row_group_size(pq.row_group_size)
        .set_data_page_size_limit(pq.data_page_size)
        .set_dictionary_page_size_limit(pq.dict_page_size)
        .set_dictionary_enabled(true)
        .set_encoding(Encoding::PLAIN)
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Page)
        .set_column_bloom_filter_enabled("mmsi".into(), true)
        .set_column_bloom_filter_fpp("mmsi".into(), 0.01)
        .set_column_bloom_filter_ndv("mmsi".into(), 50_000)
        .build()
}

fn writer_props_standard(pq: &ParquetConfig) -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
        .set_max_row_group_size(pq.row_group_size)
        .set_data_page_size_limit(pq.data_page_size)
        .set_dictionary_page_size_limit(pq.dict_page_size)
        .set_dictionary_enabled(true)
        .set_encoding(Encoding::PLAIN)
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Page)
        .build()
}

// ─── Path helpers ──────────────────────────────────────────────────────────────

pub fn time_window_15m(ts_s: TsSeconds) -> (DateTime<Utc>, DateTime<Utc>) {
    let dt = Utc.timestamp_opt(ts_s, 0).unwrap();
    let slot_start_min = (dt.minute() / 15) * 15;
    let start = dt.with_minute(slot_start_min).unwrap()
        .with_second(0).unwrap()
        .with_nanosecond(0).unwrap();
    (start, start + chrono::Duration::minutes(15))
}

fn position_path(lake_root: &str, ws: &DateTime<Utc>, we: &DateTime<Utc>) -> PathBuf {
    let fname = format!(
        "{:02}{:02}_to_{:02}{:02}.parquet",
        ws.hour(), ws.minute(), we.hour(), we.minute()
    );
    Path::new(lake_root)
        .join("bronze/source=ais/fact_position")
        .join(format!("year={}", ws.format("%Y")))
        .join(format!("month={}", ws.format("%m")))
        .join(format!("day={}", ws.format("%d")))
        .join(fname)
}

fn monthly_path(lake_root: &str, table: &str, dt: &DateTime<Utc>, filename: &str) -> PathBuf {
    Path::new(lake_root)
        .join(format!("bronze/source=ais/{table}"))
        .join(format!("year={}", dt.format("%Y")))
        .join(format!("month={}", dt.format("%m")))
        .join(filename)
}

// ─── Low-level write helpers ───────────────────────────────────────────────────

fn write_or_append(
    path: &Path,
    schema: Arc<Schema>,
    batch: RecordBatch,
    props: WriterProperties,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;

    // Write to a sibling .tmp file first, then atomically rename over the target.
    // This prevents DuckDB from reading a half-written parquet file.
    let tmp_path = path.with_extension("parquet.tmp");

    if path.exists() {
        let rf = fs::File::open(path)?;
        let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(rf)?
            .build()?;
        let mut batches: Vec<RecordBatch> = reader.collect::<Result<Vec<_>, _>>()?;
        batches.push(batch);
        let file = fs::File::create(&tmp_path)?;
        let mut w = ArrowWriter::try_new(file, schema, Some(props))?;
        for b in &batches { w.write(b)?; }
        w.close()?;
    } else {
        let file = fs::File::create(&tmp_path)?;
        let mut w = ArrowWriter::try_new(file, schema, Some(props))?;
        w.write(&batch)?;
        w.close()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

fn write_fresh(
    path: &Path,
    schema: Arc<Schema>,
    batch: RecordBatch,
    props: WriterProperties,
) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp_path = path.with_extension("parquet.tmp");
    let file = fs::File::create(&tmp_path)?;
    let mut w = ArrowWriter::try_new(file, schema, Some(props))?;
    w.write(&batch)?;
    w.close()?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

// ─── Public flush functions ────────────────────────────────────────────────────

pub fn flush_positions(
    lake_root: &str,
    buf: &mut PositionBuf,
    pq: &ParquetConfig,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() { return Ok(0); }
    let total  = buf.len() as u64;
    let schema = schema::position_schema();

    let batch = RecordBatch::try_new(schema.clone(), vec![
        Arc::new(UInt32Array::from(std::mem::take(&mut buf.mmsi))),
        Arc::new(TimestampSecondArray::from(std::mem::take(&mut buf.timestamp_s)).with_timezone("UTC")),
        Arc::new(Float64Array::from(std::mem::take(&mut buf.latitude))),
        Arc::new(Float64Array::from(std::mem::take(&mut buf.longitude))),
        Arc::new(Float32Array::from(std::mem::take(&mut buf.sog))),
        Arc::new(Float32Array::from(std::mem::take(&mut buf.cog))),
        Arc::new(UInt16Array::from(std::mem::take(&mut buf.heading))),
        Arc::new(Int16Array::from(std::mem::take(&mut buf.rot))),
    ])?;

    // Sort by MMSI for Parquet predicate pushdown
    let indices = sort_to_indices(batch.column(0), None, None)?;
    let sorted_cols: Vec<Arc<dyn Array>> = batch.columns().iter()
        .map(|c| take(c.as_ref(), &indices, None).unwrap())
        .collect();
    let sorted = RecordBatch::try_new(schema.clone(), sorted_cols)?;

    let ts_col = sorted.column(1).as_any().downcast_ref::<TimestampSecondArray>().unwrap();
    let mut windows: Vec<(DateTime<Utc>, DateTime<Utc>)> = Vec::with_capacity(sorted.num_rows());
    for i in 0..sorted.num_rows() { windows.push(time_window_15m(ts_col.value(i))); }

    let mut unique_windows = windows.clone();
    unique_windows.sort_by_key(|w| w.0);
    unique_windows.dedup();

    let props = writer_props_bloom(pq);
    for (ws, we) in &unique_windows {
        let idx_vec: Vec<u32> = windows.iter().enumerate()
            .filter(|(_, w)| w.0 == *ws)
            .map(|(i, _)| i as u32)
            .collect();
        let idx_arr = UInt32Array::from(idx_vec);
        let win_cols: Vec<Arc<dyn Array>> = sorted.columns().iter()
            .map(|c| take(c.as_ref(), &idx_arr, None).unwrap())
            .collect();
        let win_batch = RecordBatch::try_new(schema.clone(), win_cols)?;
        let path = position_path(lake_root, ws, we);
        write_fresh(&path, schema.clone(), win_batch, props.clone())?;
        info!("  → fact_position: {} rows → {}", idx_arr.len(), path.display());
    }

    buf.clear();
    Ok(total)
}

pub fn flush_draughts(
    lake_root: &str,
    buf: &mut DraughtBuf,
    pq: &ParquetConfig,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() { return Ok(0); }
    let total  = buf.len() as u64;
    let schema = schema::draught_schema();
    let mmsi   = std::mem::take(&mut buf.mmsi);
    let ts     = std::mem::take(&mut buf.timestamp_s);
    let dr     = std::mem::take(&mut buf.draught);

    for_each_month(&ts, |indices, yr, mo| {
        let batch = RecordBatch::try_new(schema.clone(), vec![
            Arc::new(UInt32Array::from(sel_u32(&mmsi, indices))),
            Arc::new(TimestampSecondArray::from(sel_i64(&ts, indices)).with_timezone("UTC")),
            Arc::new(Float32Array::from(sel_f32(&dr, indices))),
        ])?;
        let dt   = Utc.with_ymd_and_hms(yr, mo, 1, 0, 0, 0).unwrap();
        let path = monthly_path(lake_root, "fact_draught", &dt, "draught_changes.parquet");
        write_or_append(&path, schema.clone(), batch, writer_props_standard(pq))?;
        info!("  → fact_draught: {} events → {}", indices.len(), path.display());
        Ok(())
    })?;
    buf.clear();
    Ok(total)
}

pub fn flush_nav_statuses(
    lake_root: &str,
    buf: &mut NavStatusBuf,
    pq: &ParquetConfig,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() { return Ok(0); }
    let total  = buf.len() as u64;
    let schema = schema::nav_status_schema();
    let mmsi   = std::mem::take(&mut buf.mmsi);
    let ts     = std::mem::take(&mut buf.timestamp_s);
    let nav    = std::mem::take(&mut buf.nav_status);

    for_each_month(&ts, |indices, yr, mo| {
        let batch = RecordBatch::try_new(schema.clone(), vec![
            Arc::new(UInt32Array::from(sel_u32(&mmsi, indices))),
            Arc::new(TimestampSecondArray::from(sel_i64(&ts, indices)).with_timezone("UTC")),
            Arc::new(UInt8Array::from(sel_u8(&nav, indices))),
        ])?;
        let dt   = Utc.with_ymd_and_hms(yr, mo, 1, 0, 0, 0).unwrap();
        let path = monthly_path(lake_root, "fact_navigational_status", &dt, "status_changes.parquet");
        write_or_append(&path, schema.clone(), batch, writer_props_standard(pq))?;
        info!("  → fact_nav_status: {} events → {}", indices.len(), path.display());
        Ok(())
    })?;
    buf.clear();
    Ok(total)
}

pub fn flush_destinations(
    lake_root: &str,
    buf: &mut DestinationBuf,
    pq: &ParquetConfig,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() { return Ok(0); }
    let total  = buf.len() as u64;
    let schema = schema::destination_schema();
    let mmsi   = std::mem::take(&mut buf.mmsi);
    let ts     = std::mem::take(&mut buf.timestamp_s);
    let dest   = std::mem::take(&mut buf.destination);
    let eta    = std::mem::take(&mut buf.eta_utc);

    for_each_month(&ts, |indices, yr, mo| {
        let batch = RecordBatch::try_new(schema.clone(), vec![
            Arc::new(UInt32Array::from(sel_u32(&mmsi, indices))),
            Arc::new(TimestampSecondArray::from(sel_i64(&ts, indices)).with_timezone("UTC")),
            Arc::new(StringArray::from(sel_str(&dest, indices))),
            Arc::new(TimestampSecondArray::from(sel_opt_i64(&eta, indices)).with_timezone("UTC")),
        ])?;
        let dt   = Utc.with_ymd_and_hms(yr, mo, 1, 0, 0, 0).unwrap();
        let path = monthly_path(lake_root, "fact_destination", &dt, "destination_changes.parquet");
        write_or_append(&path, schema.clone(), batch, writer_props_standard(pq))?;
        info!("  → fact_destination: {} events → {}", indices.len(), path.display());
        Ok(())
    })?;
    buf.clear();
    Ok(total)
}

pub fn flush_vessel_static(
    lake_root: &str,
    buf: &mut VesselStaticBuf,
    pq: &ParquetConfig,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() { return Ok(0); }
    let total  = buf.len() as u64;
    let schema = schema::vessel_static_schema();

    let batch = RecordBatch::try_new(schema.clone(), vec![
        Arc::new(UInt32Array::from(std::mem::take(&mut buf.mmsi))),
        Arc::new(TimestampSecondArray::from(std::mem::take(&mut buf.timestamp_s)).with_timezone("UTC")),
        Arc::new(StringArray::from(std::mem::take(&mut buf.call_sign).into_iter().collect::<Vec<_>>())),
        Arc::new(UInt32Array::from(std::mem::take(&mut buf.imo_number))),
        Arc::new(UInt8Array::from(std::mem::take(&mut buf.ship_type))),
        Arc::new(UInt16Array::from(std::mem::take(&mut buf.length_overall))),
        Arc::new(UInt16Array::from(std::mem::take(&mut buf.beam))),
    ])?;

    let path = Path::new(lake_root).join("bronze/source=ais/dim_vessel/vessel_static.parquet");
    write_or_append(&path, schema, batch, writer_props_standard(pq))?;
    info!("  → dim_vessel: {} events", total);
    buf.clear();
    Ok(total)
}

pub fn flush_vessel_name_history(
    lake_root: &str,
    buf: &mut VesselNameHistoryBuf,
    pq: &ParquetConfig,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() { return Ok(0); }
    let total  = buf.len() as u64;
    let schema = schema::vessel_name_history_schema();
    let mmsi   = std::mem::take(&mut buf.mmsi);
    let ts     = std::mem::take(&mut buf.timestamp_s);
    let names  = std::mem::take(&mut buf.ship_name);

    for_each_month(&ts, |indices, yr, mo| {
        let batch = RecordBatch::try_new(schema.clone(), vec![
            Arc::new(UInt32Array::from(sel_u32(&mmsi, indices))),
            Arc::new(TimestampSecondArray::from(sel_i64(&ts, indices)).with_timezone("UTC")),
            Arc::new(StringArray::from(sel_str(&names, indices))),
        ])?;
        let dt   = Utc.with_ymd_and_hms(yr, mo, 1, 0, 0, 0).unwrap();
        let path = monthly_path(lake_root, "fact_vessel_name_history", &dt, "name_history.parquet");
        write_or_append(&path, schema.clone(), batch, writer_props_standard(pq))?;
        info!("  → fact_name_history: {} events → {}", indices.len(), path.display());
        Ok(())
    })?;
    buf.clear();
    Ok(total)
}

pub fn flush_health(
    lake_root: &str,
    counters: &mut HealthCounters,
    window_start_s: TsSeconds,
    window_end_s:   TsSeconds,
    pq: &ParquetConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let schema = schema::health_schema();
    let batch  = RecordBatch::try_new(schema.clone(), vec![
        Arc::new(TimestampSecondArray::from(vec![window_start_s]).with_timezone("UTC")),
        Arc::new(TimestampSecondArray::from(vec![window_end_s]).with_timezone("UTC")),
        Arc::new(UInt32Array::from(vec![counters.position_reports])),
        Arc::new(UInt32Array::from(vec![counters.static_data_reports])),
        Arc::new(UInt32Array::from(vec![counters.total_messages])),
        Arc::new(UInt32Array::from(vec![counters.unique_mmsis.len() as u32])),
        Arc::new(UInt32Array::from(vec![counters.parse_errors])),
    ])?;
    let dt   = Utc.timestamp_opt(window_end_s, 0).unwrap();
    let path = monthly_path(lake_root, "metric_system_health", &dt, "health_metrics.parquet");
    write_or_append(&path, schema, batch, writer_props_standard(pq))?;
    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Iterate over rows grouped by (year, month) of their timestamp.
fn for_each_month(
    ts: &[i64],
    mut f: impl FnMut(&[usize], i32, u32) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<(i32, u32), Vec<usize>> = BTreeMap::new();
    for (i, &t) in ts.iter().enumerate() {
        let dt = Utc.timestamp_opt(t, 0).unwrap();
        groups.entry((dt.year(), dt.month())).or_default().push(i);
    }
    for ((yr, mo), indices) in &groups {
        f(indices, *yr, *mo)?;
    }
    Ok(())
}

fn sel_u32(v: &[u32],      idx: &[usize]) -> Vec<u32>         { idx.iter().map(|&i| v[i]).collect() }
fn sel_i64(v: &[i64],      idx: &[usize]) -> Vec<i64>         { idx.iter().map(|&i| v[i]).collect() }
fn sel_f32(v: &[f32],      idx: &[usize]) -> Vec<f32>         { idx.iter().map(|&i| v[i]).collect() }
fn sel_u8 (v: &[u8],       idx: &[usize]) -> Vec<u8>          { idx.iter().map(|&i| v[i]).collect() }
fn sel_str(v: &[String],   idx: &[usize]) -> Vec<String>      { idx.iter().map(|&i| v[i].clone()).collect() }
fn sel_opt_i64(v: &[Option<i64>], idx: &[usize]) -> Vec<Option<i64>> { idx.iter().map(|&i| v[i]).collect() }
