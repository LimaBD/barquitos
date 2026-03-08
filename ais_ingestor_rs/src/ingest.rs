/// AIS JSONL → SQLite ingestion logic — PARALLEL version.
///
/// Architecture:
///   1. Files are parsed in parallel using rayon (CPU-bound).
///   2. Each file produces a `ParsedFile` struct with all extracted rows.
///   3. All `ParsedFile` results are merged and written to SQLite in a
///      single large transaction (SQLite is single-writer anyway).
///   4. Within each file, lines are parsed in parallel chunks using rayon.
///
/// This gives near-linear speedup on multi-core machines because parsing
/// and JSON deserialization dominate the runtime.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use log::{error, info};
use rayon::prelude::*;
use rusqlite::{params, Connection, Transaction};

use crate::config;
use crate::models::*;

// =========================================================================
// Collected rows — typed structs (Send + Sync for parallel use)
// =========================================================================

struct PositionRow {
    mmsi: i64,
    timestamp_utc: i64,
    latitude: f64,
    longitude: f64,
    sog: Option<f64>,
    cog: Option<f64>,
    true_heading: Option<i64>,
    rate_of_turn: Option<f64>,
}

struct VesselStaticRow {
    mmsi: i64,
    ship_name: Option<String>,
    call_sign: Option<String>,
    imo_number: Option<i64>,
    ship_type: Option<i64>,
    dim_bow: Option<i64>,
    dim_stern: Option<i64>,
    dim_port: Option<i64>,
    dim_starboard: Option<i64>,
    length_m: Option<f64>,
    beam_m: Option<f64>,
    max_draught: Option<f64>,
    ais_version: Option<i64>,
    fix_type: Option<i64>,
    seen_utc: i64,
}

struct VesselMinimalRow {
    mmsi: i64,
    ship_name: Option<String>,
    seen_utc: i64,
}

struct DestinationRow {
    mmsi: i64,
    destination: Option<String>,
    estimated_arrival: Option<i64>,
    timestamp_utc: i64,
}

struct DraughtRow {
    mmsi: i64,
    draught: f64,
    timestamp_utc: i64,
}

struct NavStatusRow {
    mmsi: i64,
    navigational_status: i64,
    timestamp_utc: i64,
}

struct ShipNameRow {
    mmsi: i64,
    ship_name: String,
    timestamp_utc: i64,
}

// =========================================================================
// Parsed file — all rows extracted from a single JSONL file
// =========================================================================

struct ParsedFile {
    filepath: PathBuf,
    filename: String,
    positions: Vec<PositionRow>,
    vessels_static: Vec<VesselStaticRow>,
    vessels_minimal: HashMap<i64, VesselMinimalRow>,
    destinations: Vec<DestinationRow>,
    draughts: Vec<DraughtRow>,
    nav_statuses: Vec<NavStatusRow>,
    ship_names: Vec<ShipNameRow>,
    /// AIS message counts per 60-second bucket: bucket_start → count
    message_counts: HashMap<i64, i64>,
    total_lines: u64,
    skipped_non_commercial: u64,
    /// MMSIs discovered as non-commercial during parsing of this file
    new_non_commercial: HashSet<i64>,
}

// =========================================================================
// Time helpers
// =========================================================================

fn parse_ais_time(raw: &str) -> i64 {
    let clean = raw.replace(" UTC", "").replace(" +0000", "");
    let truncated = if let Some(dot) = clean.find('.') {
        let frac = &clean[dot + 1..];
        let frac_trimmed = if frac.len() > 6 { &frac[..6] } else { frac };
        format!("{}.{}", &clean[..dot], frac_trimmed)
    } else {
        clean.clone()
    };

    if let Ok(dt) = NaiveDateTime::parse_from_str(&truncated, "%Y-%m-%d %H:%M:%S%.f") {
        return dt.and_utc().timestamp();
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(&truncated, "%Y-%m-%d %H:%M:%S") {
        return dt.and_utc().timestamp();
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return dt.timestamp();
    }
    Utc::now().timestamp()
}

fn build_eta_timestamp(eta: &Eta, reference_ts: i64) -> Option<i64> {
    let month = eta.month.unwrap_or(0);
    let day = eta.day.unwrap_or(0);
    let hour = eta.hour.unwrap_or(0).min(23);
    let minute = eta.minute.unwrap_or(0).min(59);

    if month == 0 || day == 0 {
        return None;
    }

    let ref_dt = chrono::DateTime::from_timestamp(reference_ts, 0)
        .unwrap_or_else(|| Utc::now());

    let year = ref_dt.year();
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let eta_dt = date.and_time(time).and_utc();

    let eta_final = if (ref_dt - eta_dt).num_days() > 180 {
        let date2 = NaiveDate::from_ymd_opt(year + 1, month, day)?;
        date2.and_time(time).and_utc()
    } else {
        eta_dt
    };

    Some(eta_final.timestamp())
}

// =========================================================================
// Filtering helpers
// =========================================================================

fn is_ship_mmsi(mmsi: i64) -> bool {
    (200_000_000..=799_999_999).contains(&mmsi)
}

fn is_excluded_ship_type(ship_type: Option<i64>) -> bool {
    match ship_type {
        Some(st) => config::EXCLUDED_SHIP_TYPES.contains(&st),
        None => false,
    }
}

// =========================================================================
// Row extraction from serde_json::Value
// =========================================================================

fn get_position_report(msg: &serde_json::Value) -> Option<PositionReport> {
    msg.get("PositionReport")
        .or_else(|| msg.get("StandardClassBPositionReport"))
        .or_else(|| msg.get("ExtendedClassBPositionReport"))
        .or_else(|| msg.get("StandardSearchAndRescueAircraftReport"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn get_nav_report(msg: &serde_json::Value) -> Option<PositionReport> {
    msg.get("PositionReport")
        .or_else(|| msg.get("StandardClassBPositionReport"))
        .or_else(|| msg.get("ExtendedClassBPositionReport"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn get_static_data(msg: &serde_json::Value) -> Option<ShipStaticData> {
    msg.get("ShipStaticData")
        .or_else(|| msg.get("StaticDataReport"))
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn extract_position(msg: &serde_json::Value, meta: &MetaData) -> Option<PositionRow> {
    let report = get_position_report(msg)?;
    let lat = report.latitude.or(meta.latitude)?;
    let lon = report.longitude.or(meta.longitude)?;
    let mmsi = meta.mmsi?;
    let ts = parse_ais_time(meta.time_utc.as_deref().unwrap_or(""));

    Some(PositionRow {
        mmsi,
        timestamp_utc: ts,
        latitude: lat,
        longitude: lon,
        sog: report.sog,
        cog: report.cog,
        true_heading: report.true_heading,
        rate_of_turn: report.rate_of_turn,
    })
}

fn extract_nav_status(msg: &serde_json::Value, meta: &MetaData) -> Option<NavStatusRow> {
    let report = get_nav_report(msg)?;
    let nav = report.navigational_status?;
    let mmsi = meta.mmsi?;

    Some(NavStatusRow {
        mmsi,
        navigational_status: nav,
        timestamp_utc: parse_ais_time(meta.time_utc.as_deref().unwrap_or("")),
    })
}

fn extract_vessel_static(msg: &serde_json::Value, meta: &MetaData) -> Option<VesselStaticRow> {
    let static_data = get_static_data(msg)?;
    let mmsi = meta.mmsi?;

    let name = static_data
        .name
        .as_deref()
        .or_else(|| {
            static_data
                .report_a
                .as_ref()
                .and_then(|ra| ra.name.as_deref())
        })
        .or(meta.ship_name.as_deref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let dim = static_data.dimension.as_ref();
    let dim_a = dim.and_then(|d| d.a);
    let dim_b = dim.and_then(|d| d.b);
    let dim_c = dim.and_then(|d| d.c);
    let dim_d = dim.and_then(|d| d.d);

    let length_m = match (dim_a, dim_b) {
        (Some(a), Some(b)) => Some((a + b) as f64),
        _ => None,
    };
    let beam_m = match (dim_c, dim_d) {
        (Some(c), Some(d)) => Some((c + d) as f64),
        _ => None,
    };

    let call_sign = static_data
        .call_sign
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let ship_type = static_data.ship_type.or(static_data.ship_type_alt);
    let imo = static_data.imo_number.filter(|&n| n != 0);

    Some(VesselStaticRow {
        mmsi,
        ship_name: name,
        call_sign,
        imo_number: imo,
        ship_type,
        dim_bow: dim_a,
        dim_stern: dim_b,
        dim_port: dim_c,
        dim_starboard: dim_d,
        length_m,
        beam_m,
        max_draught: static_data.max_draught,
        ais_version: static_data.ais_version,
        fix_type: static_data.fix_type,
        seen_utc: parse_ais_time(meta.time_utc.as_deref().unwrap_or("")),
    })
}

fn extract_destination(msg: &serde_json::Value, meta: &MetaData) -> Option<DestinationRow> {
    let v = msg.get("ShipStaticData")?;
    let static_data: ShipStaticData = serde_json::from_value(v.clone()).ok()?;
    let mmsi = meta.mmsi?;

    let dest = static_data
        .destination
        .as_deref()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let eta_raw = static_data.eta.as_ref();

    let has_eta = eta_raw.map_or(false, |e| {
        e.month.unwrap_or(0) != 0
            || e.day.unwrap_or(0) != 0
            || e.hour.unwrap_or(0) != 0
            || e.minute.unwrap_or(0) != 0
    });

    if dest.is_empty() && !has_eta {
        return None;
    }

    let reported_ts = parse_ais_time(meta.time_utc.as_deref().unwrap_or(""));
    let eta_ts = eta_raw.and_then(|e| build_eta_timestamp(e, reported_ts));

    Some(DestinationRow {
        mmsi,
        destination: if dest.is_empty() { None } else { Some(dest) },
        estimated_arrival: eta_ts,
        timestamp_utc: reported_ts,
    })
}

fn extract_draught(msg: &serde_json::Value, meta: &MetaData) -> Option<DraughtRow> {
    let v = msg.get("ShipStaticData")?;
    let static_data: ShipStaticData = serde_json::from_value(v.clone()).ok()?;
    let mmsi = meta.mmsi?;
    let draught = static_data.max_draught.filter(|&d| d > 0.0)?;

    Some(DraughtRow {
        mmsi,
        draught,
        timestamp_utc: parse_ais_time(meta.time_utc.as_deref().unwrap_or("")),
    })
}

fn extract_ship_name(msg: &serde_json::Value, meta: &MetaData) -> Option<ShipNameRow> {
    let static_data = get_static_data(msg)?;
    let mmsi = meta.mmsi?;

    let name = static_data
        .name
        .as_deref()
        .or_else(|| {
            static_data
                .report_a
                .as_ref()
                .and_then(|ra| ra.name.as_deref())
        })
        .or(meta.ship_name.as_deref())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;

    Some(ShipNameRow {
        mmsi,
        ship_name: name,
        timestamp_utc: parse_ais_time(meta.time_utc.as_deref().unwrap_or("")),
    })
}

// =========================================================================
// SQL constants
// =========================================================================

const POSITION_INSERT: &str = "\
INSERT INTO position_history
    (mmsi, timestamp_utc, latitude, longitude, speed_over_ground,
     course_over_ground, true_heading, rate_of_turn)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

const VESSEL_UPSERT: &str = "\
INSERT INTO vessels
    (mmsi, ship_name, call_sign, imo_number, ship_type,
     dimension_to_bow, dimension_to_stern, dimension_to_port, dimension_to_starboard,
     length_m, beam_m, max_draught,
     ais_version, fix_type, first_seen_utc, last_seen_utc)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?15)
ON CONFLICT(mmsi) DO UPDATE SET
    ship_name          = COALESCE(excluded.ship_name,          vessels.ship_name),
    call_sign          = COALESCE(excluded.call_sign,          vessels.call_sign),
    imo_number         = COALESCE(excluded.imo_number,         vessels.imo_number),
    ship_type          = COALESCE(excluded.ship_type,          vessels.ship_type),
    dimension_to_bow       = COALESCE(excluded.dimension_to_bow,       vessels.dimension_to_bow),
    dimension_to_stern     = COALESCE(excluded.dimension_to_stern,     vessels.dimension_to_stern),
    dimension_to_port      = COALESCE(excluded.dimension_to_port,      vessels.dimension_to_port),
    dimension_to_starboard = COALESCE(excluded.dimension_to_starboard, vessels.dimension_to_starboard),
    length_m           = COALESCE(excluded.length_m,           vessels.length_m),
    beam_m             = COALESCE(excluded.beam_m,             vessels.beam_m),
    max_draught        = COALESCE(excluded.max_draught,        vessels.max_draught),
    ais_version        = COALESCE(excluded.ais_version,        vessels.ais_version),
    fix_type           = COALESCE(excluded.fix_type,           vessels.fix_type),
    last_seen_utc      = MAX(excluded.last_seen_utc, vessels.last_seen_utc),
    updated_at         = CAST(strftime('%s', 'now') AS INTEGER)";

const VESSEL_UPSERT_MINIMAL: &str = "\
INSERT INTO vessels
    (mmsi, ship_name, first_seen_utc, last_seen_utc)
VALUES (?1, ?2, ?3, ?3)
ON CONFLICT(mmsi) DO UPDATE SET
    ship_name     = COALESCE(excluded.ship_name,  vessels.ship_name),
    last_seen_utc = MAX(excluded.last_seen_utc, vessels.last_seen_utc),
    updated_at    = CAST(strftime('%s', 'now') AS INTEGER)";

const DESTINATION_INSERT: &str = "\
INSERT INTO destination_history (mmsi, destination, estimated_arrival, timestamp_utc)
VALUES (?1, ?2, ?3, ?4)";

const DRAUGHT_INSERT: &str = "\
INSERT INTO draught_history (mmsi, draught, timestamp_utc)
VALUES (?1, ?2, ?3)";

const NAV_STATUS_INSERT: &str = "\
INSERT INTO navigational_status_history (mmsi, navigational_status, timestamp_utc)
VALUES (?1, ?2, ?3)";

const SHIP_NAME_INSERT: &str = "\
INSERT INTO ship_name_history (mmsi, ship_name, timestamp_utc)
VALUES (?1, ?2, ?3)";

const MESSAGE_COUNT_UPSERT: &str = "\
INSERT INTO ais_message_counts (bucket_start, message_count)
VALUES (?1, ?2)
ON CONFLICT(bucket_start) DO UPDATE SET
    message_count = ais_message_counts.message_count + excluded.message_count";

// =========================================================================
// PARALLEL file parser — reads and parses a single file on a rayon thread
// =========================================================================

/// Parse a single JSONL file into memory. This is the hot loop that runs
/// on rayon worker threads. It does NO database I/O.
fn parse_file(
    filepath: &Path,
    non_commercial_mmsis: &HashSet<i64>,
) -> Option<ParsedFile> {
    let filename = filepath
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    info!("  🔄  Parsing {} …", filename);
    let t0 = Instant::now();

    // Read entire file into memory for maximum throughput
    let mut raw = String::new();
    match fs::File::open(filepath) {
        Ok(mut f) => {
            if let Err(e) = f.read_to_string(&mut raw) {
                error!("  ❌  Cannot read {}: {}", filename, e);
                return None;
            }
        }
        Err(e) => {
            error!("  ❌  Cannot open {}: {}", filename, e);
            return None;
        }
    }

    // Collect all non-empty lines
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let total_lines = lines.len() as u64;

    // Parse lines in parallel chunks using rayon
    // Each chunk produces a partial ParsedFile; we merge them at the end.
    let chunk_size = config::PARSE_CHUNK_SIZE.max(1);

    let partial_results: Vec<ParsedFile> = lines
        .par_chunks(chunk_size)
        .map(|chunk| {
            let mut positions = Vec::new();
            let mut vessels_static = Vec::new();
            let mut vessels_minimal: HashMap<i64, VesselMinimalRow> = HashMap::new();
            let mut destinations = Vec::new();
            let mut draughts = Vec::new();
            let mut nav_statuses = Vec::new();
            let mut ship_names = Vec::new();
            let mut message_counts: HashMap<i64, i64> = HashMap::new();
            let mut skipped_non_commercial: u64 = 0;
            let mut new_non_commercial: HashSet<i64> = HashSet::new();

            // Ingest-time dedup tracking: last known values per mmsi
            let mut last_draught: HashMap<i64, i64> = HashMap::new(); // mmsi → draught bits
            let mut last_nav_status: HashMap<i64, i64> = HashMap::new(); // mmsi → nav status
            let mut last_ship_name: HashMap<i64, String> = HashMap::new(); // mmsi → name
            let mut last_position_ts: HashMap<i64, i64> = HashMap::new(); // mmsi → last ts
            let mut last_destination: HashMap<i64, (Option<String>, Option<i64>)> = HashMap::new(); // mmsi → (dest, eta)

            for line in chunk {
                let record: AisRecord = match serde_json::from_str(line) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                let meta = &record.meta_data;
                let msg = &record.message;
                let msg_type = &record.message_type;

                let mmsi = match meta.mmsi {
                    Some(m) => m,
                    None => continue,
                };

                if !is_ship_mmsi(mmsi) {
                    continue;
                }

                if non_commercial_mmsis.contains(&mmsi) || new_non_commercial.contains(&mmsi) {
                    skipped_non_commercial += 1;
                    continue;
                }

                match msg_type.as_str() {
                    "ShipStaticData" | "StaticDataReport" => {
                        if let Some(vs) = extract_vessel_static(msg, meta) {
                            if is_excluded_ship_type(vs.ship_type) {
                                new_non_commercial.insert(mmsi);
                                skipped_non_commercial += 1;
                                continue;
                            }
                            // Count this message in the appropriate 60s bucket
                            let bucket = (vs.seen_utc / 60) * 60;
                            *message_counts.entry(bucket).or_insert(0) += 1;
                            vessels_static.push(vs);
                        }
                        if let Some(dest) = extract_destination(msg, meta) {
                            // Ingest-time dedup: only keep if destination or ETA changed
                            let key = (dest.destination.clone(), dest.estimated_arrival);
                            let dominated = last_destination.get(&mmsi).map_or(false, |prev| *prev == key);
                            if !dominated {
                                last_destination.insert(mmsi, key);
                                destinations.push(dest);
                            }
                        }
                        if let Some(dr) = extract_draught(msg, meta) {
                            // Ingest-time dedup: only keep if draught changed
                            let draught_bits = dr.draught.to_bits() as i64;
                            let dominated = last_draught.get(&mmsi).map_or(false, |&prev| prev == draught_bits);
                            if !dominated {
                                last_draught.insert(mmsi, draught_bits);
                                draughts.push(dr);
                            }
                        }
                        if let Some(sn) = extract_ship_name(msg, meta) {
                            // Ingest-time dedup: only keep if name changed
                            let dominated = last_ship_name.get(&mmsi).map_or(false, |prev| *prev == sn.ship_name);
                            if !dominated {
                                last_ship_name.insert(mmsi, sn.ship_name.clone());
                                ship_names.push(sn);
                            }
                        }
                    }

                    "PositionReport"
                    | "StandardClassBPositionReport"
                    | "ExtendedClassBPositionReport"
                    | "StandardSearchAndRescueAircraftReport" => {
                        if let Some(pos) = extract_position(msg, meta) {
                            // Ingest-time dedup: skip if same vessel reported within last 5s
                            let dominated = last_position_ts.get(&mmsi).map_or(false, |&prev_ts| {
                                (pos.timestamp_utc - prev_ts).abs() < 5
                            });
                            if !dominated {
                                last_position_ts.insert(mmsi, pos.timestamp_utc);
                                // Count this message in the appropriate 60s bucket
                                let bucket = (pos.timestamp_utc / 60) * 60;
                                *message_counts.entry(bucket).or_insert(0) += 1;
                                positions.push(pos);
                            }
                            let ship_name = meta
                                .ship_name
                                .as_deref()
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty());
                            vessels_minimal.insert(
                                mmsi,
                                VesselMinimalRow {
                                    mmsi,
                                    ship_name,
                                    seen_utc: parse_ais_time(
                                        meta.time_utc.as_deref().unwrap_or(""),
                                    ),
                                },
                            );
                        }
                        if let Some(ns) = extract_nav_status(msg, meta) {
                            // Ingest-time dedup: only keep if status changed
                            let dominated = last_nav_status.get(&mmsi).map_or(false, |&prev| prev == ns.navigational_status);
                            if !dominated {
                                last_nav_status.insert(mmsi, ns.navigational_status);
                                nav_statuses.push(ns);
                            }
                        }
                    }

                    _ => {
                        // Count in bucket using AIS timestamp
                        let ts = parse_ais_time(meta.time_utc.as_deref().unwrap_or(""));
                        let bucket = (ts / 60) * 60;
                        *message_counts.entry(bucket).or_insert(0) += 1;

                        let ship_name = meta
                            .ship_name
                            .as_deref()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty());
                        vessels_minimal.insert(
                            mmsi,
                            VesselMinimalRow {
                                mmsi,
                                ship_name,
                                seen_utc: ts,
                            },
                        );
                    }
                }
            }

            ParsedFile {
                filepath: PathBuf::new(),
                filename: String::new(),
                positions,
                vessels_static,
                vessels_minimal,
                destinations,
                draughts,
                nav_statuses,
                ship_names,
                message_counts,
                total_lines: 0,
                skipped_non_commercial,
                new_non_commercial,
            }
        })
        .collect();

    // Merge partial results
    let mut merged = ParsedFile {
        filepath: filepath.to_path_buf(),
        filename: filename.clone(),
        positions: Vec::new(),
        vessels_static: Vec::new(),
        vessels_minimal: HashMap::new(),
        destinations: Vec::new(),
        draughts: Vec::new(),
        nav_statuses: Vec::new(),
        ship_names: Vec::new(),
        message_counts: HashMap::new(),
        total_lines,
        skipped_non_commercial: 0,
        new_non_commercial: HashSet::new(),
    };

    for part in partial_results {
        merged.positions.extend(part.positions);
        merged.vessels_static.extend(part.vessels_static);
        // For minimal vessels, later entries win (same as original logic)
        merged.vessels_minimal.extend(part.vessels_minimal);
        merged.destinations.extend(part.destinations);
        merged.draughts.extend(part.draughts);
        merged.nav_statuses.extend(part.nav_statuses);
        merged.ship_names.extend(part.ship_names);
        // Merge message counts (sum across chunks)
        for (bucket, count) in part.message_counts {
            *merged.message_counts.entry(bucket).or_insert(0) += count;
        }
        merged.skipped_non_commercial += part.skipped_non_commercial;
        merged.new_non_commercial.extend(part.new_non_commercial);
    }

    let elapsed = t0.elapsed();
    info!(
        "  ⚡  Parsed {} in {:.2}s — {} lines, {} positions, {} static",
        filename,
        elapsed.as_secs_f64(),
        total_lines,
        merged.positions.len(),
        merged.vessels_static.len(),
    );

    Some(merged)
}

// =========================================================================
// Batch DB writer — single-threaded, writes all parsed data
// =========================================================================

fn write_parsed_to_db(
    conn: &mut Connection,
    parsed: &ParsedFile,
) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;

    // 1) Vessel upserts
    insert_vessels_static(&tx, &parsed.vessels_static)?;
    insert_vessels_minimal(&tx, &parsed.vessels_minimal)?;

    // 2) History tables
    insert_destinations(&tx, &parsed.destinations)?;
    insert_draughts(&tx, &parsed.draughts)?;
    insert_nav_statuses(&tx, &parsed.nav_statuses)?;
    insert_ship_names(&tx, &parsed.ship_names)?;

    // 2b) AIS message counts
    insert_message_counts(&tx, &parsed.message_counts)?;

    // 3) Positions + R*Tree
    let max_id_before: i64 = tx.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM position_history",
        [],
        |row| row.get(0),
    )?;

    insert_positions(&tx, &parsed.positions)?;

    tx.execute(
        "INSERT INTO position_history_rtree (id, min_lat, max_lat, min_lon, max_lon)
         SELECT id, latitude, latitude, longitude, longitude
         FROM position_history WHERE id > ?1",
        params![max_id_before],
    )?;

    tx.commit()?;

    info!(
        "  ✅  {} → {} pos, {} dest, {} dr, {} nav, {} names, {} static, {} minimal",
        parsed.filename,
        parsed.positions.len(),
        parsed.destinations.len(),
        parsed.draughts.len(),
        parsed.nav_statuses.len(),
        parsed.ship_names.len(),
        parsed.vessels_static.len(),
        parsed.vessels_minimal.len(),
    );

    Ok(())
}

/// Write multiple parsed files in a single mega-transaction for maximum
/// throughput (fewer fsync calls).
fn write_all_to_db(
    conn: &mut Connection,
    parsed_files: &[ParsedFile],
) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;

    let mut total_pos = 0usize;
    let mut total_dest = 0usize;
    let mut total_dr = 0usize;
    let mut total_nav = 0usize;
    let mut total_names = 0usize;
    let mut total_static = 0usize;
    let mut total_minimal = 0usize;

    // 1) Vessel upserts for all files
    for pf in parsed_files {
        insert_vessels_static(&tx, &pf.vessels_static)?;
        insert_vessels_minimal(&tx, &pf.vessels_minimal)?;
        total_static += pf.vessels_static.len();
        total_minimal += pf.vessels_minimal.len();
    }

    // 2) History tables for all files
    for pf in parsed_files {
        insert_destinations(&tx, &pf.destinations)?;
        insert_draughts(&tx, &pf.draughts)?;
        insert_nav_statuses(&tx, &pf.nav_statuses)?;
        insert_ship_names(&tx, &pf.ship_names)?;
        insert_message_counts(&tx, &pf.message_counts)?;
        total_dest += pf.destinations.len();
        total_dr += pf.draughts.len();
        total_nav += pf.nav_statuses.len();
        total_names += pf.ship_names.len();
    }

    // 3) Positions + R*Tree for all files
    let max_id_before: i64 = tx.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM position_history",
        [],
        |row| row.get(0),
    )?;

    for pf in parsed_files {
        insert_positions(&tx, &pf.positions)?;
        total_pos += pf.positions.len();
    }

    tx.execute(
        "INSERT INTO position_history_rtree (id, min_lat, max_lat, min_lon, max_lon)
         SELECT id, latitude, latitude, longitude, longitude
         FROM position_history WHERE id > ?1",
        params![max_id_before],
    )?;

    tx.commit()?;

    info!(
        "  ✅  BATCH COMMIT: {} files → {} pos, {} dest, {} dr, {} nav, {} names, {} static, {} minimal",
        parsed_files.len(),
        total_pos, total_dest, total_dr, total_nav, total_names, total_static, total_minimal,
    );

    Ok(())
}

fn insert_vessels_static(tx: &Transaction, rows: &[VesselStaticRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(VESSEL_UPSERT)?;
    for r in rows {
        stmt.execute(params![
            r.mmsi,
            r.ship_name,
            r.call_sign,
            r.imo_number,
            r.ship_type,
            r.dim_bow,
            r.dim_stern,
            r.dim_port,
            r.dim_starboard,
            r.length_m,
            r.beam_m,
            r.max_draught,
            r.ais_version,
            r.fix_type,
            r.seen_utc,
        ])?;
    }
    Ok(())
}

fn insert_vessels_minimal(
    tx: &Transaction,
    map: &HashMap<i64, VesselMinimalRow>,
) -> Result<(), rusqlite::Error> {
    if map.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(VESSEL_UPSERT_MINIMAL)?;
    for r in map.values() {
        stmt.execute(params![r.mmsi, r.ship_name, r.seen_utc])?;
    }
    Ok(())
}

fn insert_positions(tx: &Transaction, rows: &[PositionRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(POSITION_INSERT)?;
    for r in rows {
        stmt.execute(params![
            r.mmsi,
            r.timestamp_utc,
            r.latitude,
            r.longitude,
            r.sog,
            r.cog,
            r.true_heading,
            r.rate_of_turn,
        ])?;
    }
    Ok(())
}

fn insert_destinations(tx: &Transaction, rows: &[DestinationRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    // DB-level dedup: load last known (destination, eta) per mmsi so we
    // skip rows that repeat the same value across file boundaries.
    let mut last: HashMap<i64, (String, i64)> = HashMap::new();
    {
        let mut q = tx.prepare_cached(
            "SELECT mmsi, COALESCE(destination,''), COALESCE(estimated_arrival,0) \
             FROM destination_history \
             WHERE id IN (SELECT MAX(id) FROM destination_history GROUP BY mmsi)"
        )?;
        let mut qrows = q.query([])?;
        while let Some(r) = qrows.next()? {
            let mmsi: i64 = r.get(0)?;
            let dest: String = r.get(1)?;
            let eta: i64 = r.get(2)?;
            last.insert(mmsi, (dest, eta));
        }
    }
    let mut stmt = tx.prepare_cached(DESTINATION_INSERT)?;
    for r in rows {
        let key = (
            r.destination.clone().unwrap_or_default(),
            r.estimated_arrival.unwrap_or(0),
        );
        if last.get(&r.mmsi).map_or(false, |prev| *prev == key) {
            continue;
        }
        last.insert(r.mmsi, key);
        stmt.execute(params![r.mmsi, r.destination, r.estimated_arrival, r.timestamp_utc])?;
    }
    Ok(())
}

fn insert_draughts(tx: &Transaction, rows: &[DraughtRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    // DB-level dedup: load last known draught per mmsi
    let mut last: HashMap<i64, i64> = HashMap::new();
    {
        let mut q = tx.prepare_cached(
            "SELECT mmsi, draught FROM draught_history \
             WHERE id IN (SELECT MAX(id) FROM draught_history GROUP BY mmsi)"
        )?;
        let mut qrows = q.query([])?;
        while let Some(r) = qrows.next()? {
            let mmsi: i64 = r.get(0)?;
            let d: f64 = r.get(1)?;
            last.insert(mmsi, d.to_bits() as i64);
        }
    }
    let mut stmt = tx.prepare_cached(DRAUGHT_INSERT)?;
    for r in rows {
        let bits = r.draught.to_bits() as i64;
        if last.get(&r.mmsi).map_or(false, |&prev| prev == bits) {
            continue;
        }
        last.insert(r.mmsi, bits);
        stmt.execute(params![r.mmsi, r.draught, r.timestamp_utc])?;
    }
    Ok(())
}

fn insert_nav_statuses(tx: &Transaction, rows: &[NavStatusRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    // DB-level dedup: load last known nav status per mmsi
    let mut last: HashMap<i64, i64> = HashMap::new();
    {
        let mut q = tx.prepare_cached(
            "SELECT mmsi, navigational_status FROM navigational_status_history \
             WHERE id IN (SELECT MAX(id) FROM navigational_status_history GROUP BY mmsi)"
        )?;
        let mut qrows = q.query([])?;
        while let Some(r) = qrows.next()? {
            let mmsi: i64 = r.get(0)?;
            let ns: i64 = r.get(1)?;
            last.insert(mmsi, ns);
        }
    }
    let mut stmt = tx.prepare_cached(NAV_STATUS_INSERT)?;
    for r in rows {
        if last.get(&r.mmsi).map_or(false, |&prev| prev == r.navigational_status) {
            continue;
        }
        last.insert(r.mmsi, r.navigational_status);
        stmt.execute(params![r.mmsi, r.navigational_status, r.timestamp_utc])?;
    }
    Ok(())
}

fn insert_ship_names(tx: &Transaction, rows: &[ShipNameRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    // DB-level dedup: load last known ship name per mmsi
    let mut last: HashMap<i64, String> = HashMap::new();
    {
        let mut q = tx.prepare_cached(
            "SELECT mmsi, ship_name FROM ship_name_history \
             WHERE id IN (SELECT MAX(id) FROM ship_name_history GROUP BY mmsi)"
        )?;
        let mut qrows = q.query([])?;
        while let Some(r) = qrows.next()? {
            let mmsi: i64 = r.get(0)?;
            let name: String = r.get(1)?;
            last.insert(mmsi, name);
        }
    }
    let mut stmt = tx.prepare_cached(SHIP_NAME_INSERT)?;
    for r in rows {
        if last.get(&r.mmsi).map_or(false, |prev| *prev == r.ship_name) {
            continue;
        }
        last.insert(r.mmsi, r.ship_name.clone());
        stmt.execute(params![r.mmsi, r.ship_name, r.timestamp_utc])?;
    }
    Ok(())
}

fn insert_message_counts(tx: &Transaction, counts: &HashMap<i64, i64>) -> Result<(), rusqlite::Error> {
    if counts.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(MESSAGE_COUNT_UPSERT)?;
    for (&bucket, &count) in counts {
        stmt.execute(params![bucket, count])?;
    }
    Ok(())
}

// =========================================================================
// File-level helpers
// =========================================================================

fn is_already_processed(filepath: &Path, backup_dir: &Path) -> bool {
    if let Some(name) = filepath.file_name() {
        backup_dir.join(name).exists()
    } else {
        false
    }
}

pub fn move_to_backup(filepath: &Path, backup_dir: &Path) {
    fs::create_dir_all(backup_dir).ok();
    if let Some(name) = filepath.file_name() {
        let dest = backup_dir.join(name);
        match fs::rename(filepath, &dest) {
            Ok(()) => info!("  📦  Moved → {}", dest.display()),
            Err(_e) => {
                match fs::copy(filepath, &dest) {
                    Ok(_) => {
                        fs::remove_file(filepath).ok();
                        info!("  📦  Moved → {}", dest.display());
                    }
                    Err(e2) => error!(
                        "  ❌  Failed to move {} → {}: {}",
                        filepath.display(),
                        dest.display(),
                        e2
                    ),
                }
            }
        }
    }
}

// =========================================================================
// Public entry point — PARALLEL pipeline
// =========================================================================

/// Parallel ingestion pipeline — STREAMING BATCHES:
///   1. Discover JSONL files
///   2. Process them in batches of FILES_PER_BATCH:
///      a. Parse batch in parallel (rayon)  — CPU-bound
///      b. Write batch to SQLite            — IO-bound
///      c. Move processed files to backup   — immediate feedback
///   3. Repeat until all files are done
///
/// This keeps memory bounded and moves files continuously instead of
/// trying to hold all 3000+ files in memory at once.
pub fn run_once_parallel(
    conn: &mut Connection,
    raw_dir: &Path,
    backup_dir: &Path,
    non_commercial_mmsis: &mut HashSet<i64>,
) {
    let t_total = Instant::now();

    let pattern = raw_dir.join(config::JSONL_GLOB);
    let pattern_str = pattern.to_string_lossy();

    let mut files: Vec<PathBuf> = glob::glob(&pattern_str)
        .unwrap_or_else(|e| {
            error!("Bad glob pattern: {}", e);
            std::process::exit(1);
        })
        .filter_map(|r| r.ok())
        .filter(|fp| !is_already_processed(fp, backup_dir))
        .collect();

    files.sort();

    if files.is_empty() {
        info!("No new JSONL files found in {}", raw_dir.display());
        return;
    }

    let total_files = files.len();
    let batch_size = config::FILES_PER_BATCH;
    let num_batches = (total_files + batch_size - 1) / batch_size;

    info!(
        "🚀  Found {} JSONL file(s) — processing in {} batch(es) of up to {}",
        total_files, num_batches, batch_size,
    );

    let mut files_done = 0usize;
    let mut total_pos = 0usize;
    let mut total_lines_all = 0u64;

    for (batch_idx, file_chunk) in files.chunks(batch_size).enumerate() {
        let t_batch = Instant::now();

        info!(
            "── Batch {}/{} ({} files) ──────────────────────────",
            batch_idx + 1,
            num_batches,
            file_chunk.len(),
        );

        // ── Phase 1: Parallel parse this batch ───────────────────────
        let nc_snapshot: HashSet<i64> = non_commercial_mmsis.clone();

        let parsed_files: Vec<ParsedFile> = file_chunk
            .par_iter()
            .filter_map(|fp| parse_file(fp, &nc_snapshot))
            .collect();

        if parsed_files.is_empty() {
            continue;
        }

        // Collect newly-discovered non-commercial MMSIs
        for pf in &parsed_files {
            non_commercial_mmsis.extend(&pf.new_non_commercial);
        }

        let batch_lines: u64 = parsed_files.iter().map(|pf| pf.total_lines).sum();
        let batch_pos: usize = parsed_files.iter().map(|pf| pf.positions.len()).sum();

        // ── Phase 2: Write this batch to SQLite ──────────────────────
        match write_all_to_db(conn, &parsed_files) {
            Ok(()) => {
                for pf in &parsed_files {
                    move_to_backup(&pf.filepath, backup_dir);
                }
                files_done += parsed_files.len();
            }
            Err(e) => {
                error!("  ❌  Batch write failed: {} — retrying one-by-one", e);
                for pf in &parsed_files {
                    match write_parsed_to_db(conn, pf) {
                        Ok(()) => {
                            move_to_backup(&pf.filepath, backup_dir);
                            files_done += 1;
                        }
                        Err(e2) => error!(
                            "  ❌  Failed to ingest {}: {}",
                            pf.filename, e2
                        ),
                    }
                }
            }
        }

        total_pos += batch_pos;
        total_lines_all += batch_lines;

        let batch_elapsed = t_batch.elapsed();
        info!(
            "── Batch {}/{} done: {} lines, {} positions in {:.2}s  ({}/{} files total) ──",
            batch_idx + 1,
            num_batches,
            batch_lines,
            batch_pos,
            batch_elapsed.as_secs_f64(),
            files_done,
            total_files,
        );
    }

    let total_elapsed = t_total.elapsed();
    info!(
        "🏁  All done: {}/{} files, {} lines, {} positions in {:.2}s",
        files_done,
        total_files,
        total_lines_all,
        total_pos,
        total_elapsed.as_secs_f64(),
    );
}
