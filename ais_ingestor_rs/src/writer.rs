// ─── Parquet Writer ─────────────────────────────────────────────────────────
//
// Builds Arrow RecordBatches from in-memory buffers and writes them to
// Hive-partitioned Parquet files on the mechanical HDD.
//
// Key optimisations:
//   • fact_position is sorted by MMSI before writing (Parquet min/max metadata)
//   • Bloom filters on MMSI column for needle-in-haystack DuckDB queries
//   • Small row-group sizes (8 192) for Pi 3B RAM constraints
// ────────────────────────────────────────────────────────────────────────────

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_array::*;
use arrow_ord::sort::sort_to_indices;
use arrow_select::take::take;
use arrow_schema::Schema;
use arrow_array::RecordBatch;
use chrono::{DateTime, Datelike, Utc, Timelike, TimeZone};
use log::info;
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding, ZstdLevel};
use parquet::file::properties::WriterProperties;

use crate::config;
use crate::parser::TsSeconds;
use crate::schema;

// ─── In-memory buffers ──────────────────────────────────────────────────────

/// Buffers for fact_position rows collected during a micro-batch.
#[derive(Default)]
pub struct PositionBuf {
    pub mmsi: Vec<u32>,
    pub timestamp_s: Vec<i64>,
    pub latitude: Vec<f64>,
    pub longitude: Vec<f64>,
    pub sog: Vec<Option<f32>>,
    pub cog: Vec<Option<f32>>,
    pub heading: Vec<Option<u16>>,
    pub rot: Vec<Option<i16>>,
}

impl PositionBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear();
        self.timestamp_s.clear();
        self.latitude.clear();
        self.longitude.clear();
        self.sog.clear();
        self.cog.clear();
        self.heading.clear();
        self.rot.clear();
    }
}

/// Buffers for fact_draught edge events.
#[derive(Default)]
pub struct DraughtBuf {
    pub mmsi: Vec<u32>,
    pub timestamp_s: Vec<i64>,
    pub draught: Vec<f32>,
}

impl DraughtBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear();
        self.timestamp_s.clear();
        self.draught.clear();
    }
}

/// Buffers for fact_navigational_status edge events.
#[derive(Default)]
pub struct NavStatusBuf {
    pub mmsi: Vec<u32>,
    pub timestamp_s: Vec<i64>,
    pub nav_status: Vec<u8>,
}

impl NavStatusBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear();
        self.timestamp_s.clear();
        self.nav_status.clear();
    }
}

/// Buffers for fact_destination edge events.
#[derive(Default)]
pub struct DestinationBuf {
    pub mmsi: Vec<u32>,
    pub timestamp_s: Vec<i64>,
    pub destination: Vec<String>,
    pub eta_utc: Vec<Option<i64>>,
}

impl DestinationBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear();
        self.timestamp_s.clear();
        self.destination.clear();
        self.eta_utc.clear();
    }
}

/// Buffers for dim_vessel edge events (edge-filtered static data).
#[derive(Default)]
pub struct VesselStaticBuf {
    pub mmsi: Vec<u32>,
    pub timestamp_s: Vec<i64>,
    pub call_sign: Vec<Option<String>>,
    pub imo_number: Vec<Option<u32>>,
    pub ship_type: Vec<Option<u8>>,
    pub length_overall: Vec<Option<u16>>,
    pub beam: Vec<Option<u16>>,
}

impl VesselStaticBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear();
        self.timestamp_s.clear();
        self.call_sign.clear();
        self.imo_number.clear();
        self.ship_type.clear();
        self.length_overall.clear();
        self.beam.clear();
    }
}

/// Buffers for fact_vessel_name_history edge events.
#[derive(Default)]
pub struct VesselNameHistoryBuf {
    pub mmsi: Vec<u32>,
    pub timestamp_s: Vec<i64>,
    pub ship_name: Vec<String>,
}

impl VesselNameHistoryBuf {
    pub fn len(&self) -> usize { self.mmsi.len() }
    pub fn is_empty(&self) -> bool { self.mmsi.is_empty() }
    pub fn clear(&mut self) {
        self.mmsi.clear();
        self.timestamp_s.clear();
        self.ship_name.clear();
    }
}

/// Health / telemetry counters for a single batch window.
#[derive(Default)]
pub struct HealthCounters {
    pub position_reports: u32,
    pub static_data_reports: u32,
    pub total_messages: u32,
    pub unique_mmsis: HashSet<u32>,
    pub parse_errors: u32,
}

impl HealthCounters {
    pub fn clear(&mut self) {
        self.position_reports = 0;
        self.static_data_reports = 0;
        self.total_messages = 0;
        self.unique_mmsis.clear();
        self.parse_errors = 0;
    }
}

// ─── Writer properties ──────────────────────────────────────────────────────

/// Build Parquet WriterProperties with Bloom filter on MMSI.
fn writer_props_with_bloom() -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
        .set_max_row_group_size(config::PARQUET_ROW_GROUP_SIZE)
        .set_data_page_size_limit(config::PARQUET_DATA_PAGE_SIZE)
        .set_dictionary_page_size_limit(config::PARQUET_DICT_PAGE_SIZE)
        .set_dictionary_enabled(true)
        .set_encoding(Encoding::PLAIN)
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Page)
        .set_column_bloom_filter_enabled("mmsi".into(), true)
        .set_column_bloom_filter_fpp("mmsi".into(), 0.01)
        .set_column_bloom_filter_ndv("mmsi".into(), 50_000)
        .build()
}

/// Build writer props without bloom filter (for smaller edge tables).
fn writer_props_standard() -> WriterProperties {
    WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(3).unwrap()))
        .set_max_row_group_size(config::PARQUET_ROW_GROUP_SIZE)
        .set_data_page_size_limit(config::PARQUET_DATA_PAGE_SIZE)
        .set_dictionary_page_size_limit(config::PARQUET_DICT_PAGE_SIZE)
        .set_dictionary_enabled(true)
        .set_encoding(Encoding::PLAIN)
        .set_statistics_enabled(parquet::file::properties::EnabledStatistics::Page)
        .build()
}

// ─── Path helpers ───────────────────────────────────────────────────────────

/// Compute the 15-minute time window for a given timestamp (seconds).
/// Returns (window_start, window_end) as DateTime<Utc>.
pub fn time_window_15m(ts_s: TsSeconds) -> (DateTime<Utc>, DateTime<Utc>) {
    let dt = Utc.timestamp_opt(ts_s, 0).unwrap();
    let minute = dt.minute();
    let slot_start_min = (minute / 15) * 15;
    let start = dt
        .with_minute(slot_start_min).unwrap()
        .with_second(0).unwrap()
        .with_nanosecond(0).unwrap();
    let end = start + chrono::Duration::minutes(15);
    (start, end)
}

/// Build the deep Hive partition path for fact_position:
///   bronze/source=ais/fact_position/year=YYYY/month=MM/day=DD/HHMM_to_HHMM.parquet
pub fn position_path(lake_root: &str, window_start: &DateTime<Utc>, window_end: &DateTime<Utc>) -> PathBuf {
    let fname = format!(
        "{:02}{:02}_to_{:02}{:02}.parquet",
        window_start.hour(), window_start.minute(),
        window_end.hour(), window_end.minute(),
    );
    Path::new(lake_root)
        .join("bronze/source=ais/fact_position")
        .join(format!("year={}", window_start.format("%Y")))
        .join(format!("month={}", window_start.format("%m")))
        .join(format!("day={}", window_start.format("%d")))
        .join(fname)
}

/// Build the light partition path for monthly edge tables:
///   bronze/source=ais/<table>/year=YYYY/month=MM/<filename>.parquet
fn monthly_path(lake_root: &str, table: &str, dt: &DateTime<Utc>, filename: &str) -> PathBuf {
    Path::new(lake_root)
        .join(format!("bronze/source=ais/{table}"))
        .join(format!("year={}", dt.format("%Y")))
        .join(format!("month={}", dt.format("%m")))
        .join(filename)
}

/// Build path for health metrics.
fn health_path(lake_root: &str, dt: &DateTime<Utc>) -> PathBuf {
    monthly_path(lake_root, "metric_system_health", dt, "health_metrics.parquet")
}

// ─── Write functions ────────────────────────────────────────────────────────

/// Ensure parent directory exists and write a RecordBatch.
/// If file already exists, append a new row group WITHOUT re-reading the
/// entire file.  This prevents ever-growing RAM usage and I/O stalls as
/// edge tables accumulate data over weeks/months.
fn write_or_append(
    path: &Path,
    schema: Arc<Schema>,
    batch: RecordBatch,
    props: WriterProperties,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    if path.exists() {
        // Read existing row groups, append the new batch, rewrite.
        // Edge tables stay small (a few thousand rows/month) so this is fine.
        let reader_file = fs::File::open(path)?;
        let reader =
            parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(reader_file)?
                .build()?;
        let mut existing_batches: Vec<RecordBatch> = reader.collect::<Result<Vec<_>, _>>()?;
        existing_batches.push(batch);

        let file = fs::File::create(path)?;
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
        for b in &existing_batches {
            writer.write(b)?;
        }
        writer.close()?;
    } else {
        let file = fs::File::create(path)?;
        let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
        writer.write(&batch)?;
        writer.close()?;
    }
    Ok(())
}

/// Write a fresh Parquet file (no append — used for position 15m windows).
fn write_fresh(
    path: &Path,
    schema: Arc<Schema>,
    batch: RecordBatch,
    props: WriterProperties,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(())
}

// ─── Public flush functions ─────────────────────────────────────────────────

/// Flush fact_position: sort by MMSI, split by 15-minute windows, write with bloom filter.
pub fn flush_positions(
    lake_root: &str,
    buf: &mut PositionBuf,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() {
        return Ok(0);
    }
    let total = buf.len() as u64;
    let schema = schema::position_schema();

    // Build RecordBatch
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(std::mem::take(&mut buf.mmsi))),
            Arc::new(
                TimestampSecondArray::from(std::mem::take(&mut buf.timestamp_s))
                    .with_timezone("UTC"),
            ),
            Arc::new(Float64Array::from(std::mem::take(&mut buf.latitude))),
            Arc::new(Float64Array::from(std::mem::take(&mut buf.longitude))),
            Arc::new(Float32Array::from(std::mem::take(&mut buf.sog))),
            Arc::new(Float32Array::from(std::mem::take(&mut buf.cog))),
            Arc::new(UInt16Array::from(std::mem::take(&mut buf.heading))),
            Arc::new(Int16Array::from(std::mem::take(&mut buf.rot))),
        ],
    )?;

    // Sort by MMSI for optimal Parquet min/max statistics
    let mmsi_col = batch.column(0);
    let indices = sort_to_indices(mmsi_col, None, None)?;
    let sorted_columns: Vec<Arc<dyn Array>> = batch
        .columns()
        .iter()
        .map(|col| take(col.as_ref(), &indices, None).unwrap())
        .collect();
    let sorted = RecordBatch::try_new(schema.clone(), sorted_columns)?;

    // Group by 15-minute window and write separate files
    let ts_col = sorted
        .column(1)
        .as_any()
        .downcast_ref::<TimestampSecondArray>()
        .unwrap();

    // Collect all (window_start, window_end) for each row
    let mut windows: Vec<(DateTime<Utc>, DateTime<Utc>)> = Vec::with_capacity(sorted.num_rows());
    for i in 0..sorted.num_rows() {
        windows.push(time_window_15m(ts_col.value(i)));
    }

    // Find unique windows
    let mut unique_windows: Vec<(DateTime<Utc>, DateTime<Utc>)> = windows.clone();
    unique_windows.sort_by_key(|w| w.0);
    unique_windows.dedup();

    let props = writer_props_with_bloom();

    for (ws, we) in &unique_windows {
        let indices_for_window: Vec<usize> = windows
            .iter()
            .enumerate()
            .filter(|(_, w)| w.0 == *ws)
            .map(|(i, _)| i)
            .collect();

        if indices_for_window.is_empty() {
            continue;
        }

        let idx_array = UInt32Array::from(
            indices_for_window.iter().map(|&i| i as u32).collect::<Vec<u32>>(),
        );
        let window_cols: Vec<Arc<dyn Array>> = sorted
            .columns()
            .iter()
            .map(|col| take(col.as_ref(), &idx_array, None).unwrap())
            .collect();
        let window_batch = RecordBatch::try_new(schema.clone(), window_cols)?;

        let path = position_path(lake_root, ws, we);
        write_fresh(&path, schema.clone(), window_batch, props.clone())?;
        info!(
            "  → fact_position: {} rows → {}",
            indices_for_window.len(),
            path.display()
        );
    }

    buf.clear();
    Ok(total)
}

/// Flush fact_draught edge events (append to monthly file).
/// Partitions by the AIS-provided timestamp, not the local clock.
pub fn flush_draughts(
    lake_root: &str,
    buf: &mut DraughtBuf,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() {
        return Ok(0);
    }
    let total = buf.len() as u64;
    let schema = schema::draught_schema();

    let mmsi_vec = std::mem::take(&mut buf.mmsi);
    let ts_vec = std::mem::take(&mut buf.timestamp_s);
    let draught_vec = std::mem::take(&mut buf.draught);

    // Group rows by (year, month) derived from the AIS timestamp
    let mut groups: std::collections::BTreeMap<(i32, u32), Vec<usize>> = std::collections::BTreeMap::new();
    for (i, &ts) in ts_vec.iter().enumerate() {
        let dt = Utc.timestamp_opt(ts, 0).unwrap();
        let key = (dt.year(), dt.month());
        groups.entry(key).or_default().push(i);
    }

    for ((year, month), indices) in &groups {
        let g_mmsi: Vec<u32> = indices.iter().map(|&i| mmsi_vec[i]).collect();
        let g_ts: Vec<i64> = indices.iter().map(|&i| ts_vec[i]).collect();
        let g_dr: Vec<f32> = indices.iter().map(|&i| draught_vec[i]).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(UInt32Array::from(g_mmsi)),
                Arc::new(TimestampSecondArray::from(g_ts).with_timezone("UTC")),
                Arc::new(Float32Array::from(g_dr)),
            ],
        )?;

        let representative_dt = Utc.with_ymd_and_hms(*year, *month, 1, 0, 0, 0).unwrap();
        let path = monthly_path(lake_root, "fact_draught", &representative_dt, "draught_changes.parquet");
        write_or_append(&path, schema.clone(), batch, writer_props_standard())?;
        info!("  → fact_draught: {} events → {}", indices.len(), path.display());
    }

    buf.clear();
    Ok(total)
}

/// Flush fact_navigational_status edge events (append to monthly file).
/// Partitions by the AIS-provided timestamp, not the local clock.
pub fn flush_nav_statuses(
    lake_root: &str,
    buf: &mut NavStatusBuf,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() {
        return Ok(0);
    }
    let total = buf.len() as u64;
    let schema = schema::nav_status_schema();

    let mmsi_vec = std::mem::take(&mut buf.mmsi);
    let ts_vec = std::mem::take(&mut buf.timestamp_s);
    let nav_vec = std::mem::take(&mut buf.nav_status);

    // Group rows by (year, month) derived from the AIS timestamp
    let mut groups: std::collections::BTreeMap<(i32, u32), Vec<usize>> = std::collections::BTreeMap::new();
    for (i, &ts) in ts_vec.iter().enumerate() {
        let dt = Utc.timestamp_opt(ts, 0).unwrap();
        let key = (dt.year(), dt.month());
        groups.entry(key).or_default().push(i);
    }

    for ((year, month), indices) in &groups {
        let g_mmsi: Vec<u32> = indices.iter().map(|&i| mmsi_vec[i]).collect();
        let g_ts: Vec<i64> = indices.iter().map(|&i| ts_vec[i]).collect();
        let g_nav: Vec<u8> = indices.iter().map(|&i| nav_vec[i]).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(UInt32Array::from(g_mmsi)),
                Arc::new(TimestampSecondArray::from(g_ts).with_timezone("UTC")),
                Arc::new(UInt8Array::from(g_nav)),
            ],
        )?;

        let representative_dt = Utc.with_ymd_and_hms(*year, *month, 1, 0, 0, 0).unwrap();
        let path = monthly_path(
            lake_root,
            "fact_navigational_status",
            &representative_dt,
            "status_changes.parquet",
        );
        write_or_append(&path, schema.clone(), batch, writer_props_standard())?;
        info!("  → fact_navigational_status: {} events → {}", indices.len(), path.display());
    }

    buf.clear();
    Ok(total)
}

/// Flush fact_destination edge events (append to monthly file).
/// Partitions by the AIS-provided timestamp, not the local clock.
pub fn flush_destinations(
    lake_root: &str,
    buf: &mut DestinationBuf,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() {
        return Ok(0);
    }
    let total = buf.len() as u64;
    let schema = schema::destination_schema();

    let mmsi_vec = std::mem::take(&mut buf.mmsi);
    let ts_vec = std::mem::take(&mut buf.timestamp_s);
    let dest_vec = std::mem::take(&mut buf.destination);
    let eta_vec = std::mem::take(&mut buf.eta_utc);

    // Group rows by (year, month) derived from the AIS timestamp
    let mut groups: std::collections::BTreeMap<(i32, u32), Vec<usize>> = std::collections::BTreeMap::new();
    for (i, &ts) in ts_vec.iter().enumerate() {
        let dt = Utc.timestamp_opt(ts, 0).unwrap();
        let key = (dt.year(), dt.month());
        groups.entry(key).or_default().push(i);
    }

    for ((year, month), indices) in &groups {
        let g_mmsi: Vec<u32> = indices.iter().map(|&i| mmsi_vec[i]).collect();
        let g_ts: Vec<i64> = indices.iter().map(|&i| ts_vec[i]).collect();
        let g_dest: Vec<String> = indices.iter().map(|i| dest_vec[*i].clone()).collect();
        let g_eta: Vec<Option<i64>> = indices.iter().map(|&i| eta_vec[i]).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(UInt32Array::from(g_mmsi)),
                Arc::new(TimestampSecondArray::from(g_ts).with_timezone("UTC")),
                Arc::new(StringArray::from(g_dest)),
                Arc::new(
                    TimestampSecondArray::from(g_eta).with_timezone("UTC"),
                ),
            ],
        )?;

        let representative_dt = Utc.with_ymd_and_hms(*year, *month, 1, 0, 0, 0).unwrap();
        let path = monthly_path(
            lake_root,
            "fact_destination",
            &representative_dt,
            "destination_changes.parquet",
        );
        write_or_append(&path, schema.clone(), batch, writer_props_standard())?;
        info!("  → fact_destination: {} events → {}", indices.len(), path.display());
    }

    buf.clear();
    Ok(total)
}

/// Flush dim_vessel — edge-filtered vessel static data (flat file, no time partitioning).
pub fn flush_vessel_static(
    lake_root: &str,
    buf: &mut VesselStaticBuf,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() {
        return Ok(0);
    }
    let total = buf.len() as u64;
    let schema = schema::vessel_static_schema();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(UInt32Array::from(std::mem::take(&mut buf.mmsi))),
            Arc::new(
                TimestampSecondArray::from(std::mem::take(&mut buf.timestamp_s))
                    .with_timezone("UTC"),
            ),
            Arc::new(StringArray::from(
                std::mem::take(&mut buf.call_sign)
                    .into_iter()
                    .collect::<Vec<Option<String>>>(),
            )),
            Arc::new(UInt32Array::from(std::mem::take(&mut buf.imo_number))),
            Arc::new(UInt8Array::from(std::mem::take(&mut buf.ship_type))),
            Arc::new(UInt16Array::from(std::mem::take(&mut buf.length_overall))),
            Arc::new(UInt16Array::from(std::mem::take(&mut buf.beam))),
        ],
    )?;

    let path = Path::new(lake_root)
        .join("bronze/source=ais/dim_vessel/vessel_static.parquet");
    write_or_append(&path, schema, batch, writer_props_standard())?;
    info!("  → dim_vessel: {} events → {}", total, path.display());

    buf.clear();
    Ok(total)
}

/// Flush fact_vessel_name_history (append to monthly file).
/// Partitions by the AIS-provided timestamp, not the local clock.
pub fn flush_vessel_name_history(
    lake_root: &str,
    buf: &mut VesselNameHistoryBuf,
) -> Result<u64, Box<dyn std::error::Error>> {
    if buf.is_empty() {
        return Ok(0);
    }
    let total = buf.len() as u64;
    let schema = schema::vessel_name_history_schema();

    let mmsi_vec = std::mem::take(&mut buf.mmsi);
    let ts_vec = std::mem::take(&mut buf.timestamp_s);
    let name_vec = std::mem::take(&mut buf.ship_name);

    // Group rows by (year, month) derived from the AIS timestamp
    let mut groups: std::collections::BTreeMap<(i32, u32), Vec<usize>> = std::collections::BTreeMap::new();
    for (i, &ts) in ts_vec.iter().enumerate() {
        let dt = Utc.timestamp_opt(ts, 0).unwrap();
        let key = (dt.year(), dt.month());
        groups.entry(key).or_default().push(i);
    }

    for ((year, month), indices) in &groups {
        let g_mmsi: Vec<u32> = indices.iter().map(|&i| mmsi_vec[i]).collect();
        let g_ts: Vec<i64> = indices.iter().map(|&i| ts_vec[i]).collect();
        let g_name: Vec<String> = indices.iter().map(|i| name_vec[*i].clone()).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(UInt32Array::from(g_mmsi)),
                Arc::new(TimestampSecondArray::from(g_ts).with_timezone("UTC")),
                Arc::new(StringArray::from(g_name)),
            ],
        )?;

        let representative_dt = Utc.with_ymd_and_hms(*year, *month, 1, 0, 0, 0).unwrap();
        let path = monthly_path(
            lake_root,
            "fact_vessel_name_history",
            &representative_dt,
            "name_history.parquet",
        );
        write_or_append(&path, schema.clone(), batch, writer_props_standard())?;
        info!("  → fact_vessel_name_history: {} events → {}", indices.len(), path.display());
    }

    buf.clear();
    Ok(total)
}

/// Flush health metrics for the batch window.
pub fn flush_health(
    lake_root: &str,
    counters: &mut HealthCounters,
    window_start_s: TsSeconds,
    window_end_s: TsSeconds,
) -> Result<(), Box<dyn std::error::Error>> {
    let schema = schema::health_schema();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(
                TimestampSecondArray::from(vec![window_start_s]).with_timezone("UTC"),
            ),
            Arc::new(
                TimestampSecondArray::from(vec![window_end_s]).with_timezone("UTC"),
            ),
            Arc::new(UInt32Array::from(vec![counters.position_reports])),
            Arc::new(UInt32Array::from(vec![counters.static_data_reports])),
            Arc::new(UInt32Array::from(vec![counters.total_messages])),
            Arc::new(UInt32Array::from(vec![counters.unique_mmsis.len() as u32])),
            Arc::new(UInt32Array::from(vec![counters.parse_errors])),
        ],
    )?;

    let dt = Utc.timestamp_opt(window_end_s, 0).unwrap();
    let path = health_path(lake_root, &dt);
    write_or_append(&path, schema, batch, writer_props_standard())?;
    info!(
        "  → health: {} total msgs, {} positions, {} unique MMSIs → {}",
        counters.total_messages,
        counters.position_reports,
        counters.unique_mmsis.len(),
        path.display()
    );

    counters.clear();
    Ok(())
}
