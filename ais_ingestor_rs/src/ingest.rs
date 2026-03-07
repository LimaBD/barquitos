/// AIS JSONL → SQLite ingestion logic — mirrors `ais_ingestor/ingest.py`.
///
/// Reads each JSONL line, extracts position / static / destination / draught /
/// nav-status / ship-name rows, batch-inserts into SQLite, then populates the
/// R*Tree index.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use log::{error, info};
use rusqlite::{params, Connection, Transaction};

use crate::config;
use crate::models::*;

// =========================================================================
// Collected rows — typed structs instead of Python dicts
// =========================================================================

struct PositionRow {
    mmsi: i64,
    timestamp_utc: String,
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
    seen_utc: String,
}

struct VesselMinimalRow {
    mmsi: i64,
    ship_name: Option<String>,
    seen_utc: String,
}

struct DestinationRow {
    mmsi: i64,
    destination: Option<String>,
    estimated_arrival: Option<String>,
    timestamp_utc: String,
}

struct DraughtRow {
    mmsi: i64,
    draught: f64,
    timestamp_utc: String,
}

struct NavStatusRow {
    mmsi: i64,
    navigational_status: i64,
    timestamp_utc: String,
}

struct ShipNameRow {
    mmsi: i64,
    ship_name: String,
    timestamp_utc: String,
}

// =========================================================================
// Time helpers
// =========================================================================

/// Convert Go-style AIS timestamp to ISO-8601.
///   "2026-03-04 04:34:59.795811956 +0000 UTC" → "2026-03-04T04:34:59.795811+00:00"
fn parse_ais_time(raw: &str) -> String {
    let clean = raw.replace(" UTC", "").replace(" +0000", "");
    // Truncate fractional seconds to 6 digits (microseconds)
    let truncated = if let Some(dot) = clean.find('.') {
        let frac = &clean[dot + 1..];
        let frac_trimmed = if frac.len() > 6 { &frac[..6] } else { frac };
        format!("{}.{}", &clean[..dot], frac_trimmed)
    } else {
        clean.clone()
    };

    // Try parsing as "%Y-%m-%d %H:%M:%S%.f"
    if let Ok(dt) = NaiveDateTime::parse_from_str(&truncated, "%Y-%m-%d %H:%M:%S%.f") {
        return dt.and_utc().to_rfc3339();
    }
    // Fallback: no fractional part
    if let Ok(dt) = NaiveDateTime::parse_from_str(&truncated, "%Y-%m-%d %H:%M:%S") {
        return dt.and_utc().to_rfc3339();
    }
    raw.to_string()
}

/// Build an ISO-8601 ETA from AIS month/day/hour/minute fields.
fn build_eta_iso(eta: &Eta, reference_time: &str) -> Option<String> {
    let month = eta.month.unwrap_or(0);
    let day = eta.day.unwrap_or(0);
    let hour = eta.hour.unwrap_or(0).min(23);
    let minute = eta.minute.unwrap_or(0).min(59);

    if month == 0 || day == 0 {
        return None;
    }

    let ref_dt = chrono::DateTime::parse_from_rfc3339(reference_time)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    let year = ref_dt.year();

    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let eta_dt = date.and_time(time).and_utc();

    // If ETA is >6 months in the past, assume next year
    let eta_final = if (ref_dt - eta_dt).num_days() > 180 {
        let date2 = NaiveDate::from_ymd_opt(year + 1, month, day)?;
        date2.and_time(time).and_utc()
    } else {
        eta_dt
    };

    Some(eta_final.to_rfc3339())
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

    let reported_utc = parse_ais_time(meta.time_utc.as_deref().unwrap_or(""));
    let eta_iso = eta_raw.and_then(|e| build_eta_iso(e, &reported_utc));

    Some(DestinationRow {
        mmsi,
        destination: if dest.is_empty() { None } else { Some(dest) },
        estimated_arrival: eta_iso,
        timestamp_utc: reported_utc,
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
    updated_at         = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

const VESSEL_UPSERT_MINIMAL: &str = "\
INSERT INTO vessels
    (mmsi, ship_name, first_seen_utc, last_seen_utc)
VALUES (?1, ?2, ?3, ?3)
ON CONFLICT(mmsi) DO UPDATE SET
    ship_name     = COALESCE(excluded.ship_name,  vessels.ship_name),
    last_seen_utc = MAX(excluded.last_seen_utc, vessels.last_seen_utc),
    updated_at    = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')";

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

// =========================================================================
// Batch DB writer
// =========================================================================

fn write_to_db(
    conn: &mut Connection,
    vessels_static: &[VesselStaticRow],
    vessels_minimal: &HashMap<i64, VesselMinimalRow>,
    positions: &[PositionRow],
    destinations: &[DestinationRow],
    draughts: &[DraughtRow],
    nav_statuses: &[NavStatusRow],
    ship_names: &[ShipNameRow],
    filename: &str,
) -> Result<(), rusqlite::Error> {
    let tx = conn.transaction()?;

    // 1) Vessel upserts — static (rich) first, then minimal (deduped)
    insert_vessels_static(&tx, vessels_static)?;
    insert_vessels_minimal(&tx, vessels_minimal)?;

    // 2) History tables
    insert_destinations(&tx, destinations)?;
    insert_draughts(&tx, draughts)?;
    insert_nav_statuses(&tx, nav_statuses)?;
    insert_ship_names(&tx, ship_names)?;

    // 3) Positions (batched) + R*Tree
    let max_id_before: i64 = tx.query_row(
        "SELECT COALESCE(MAX(id), 0) FROM position_history",
        [],
        |row| row.get(0),
    )?;

    insert_positions(&tx, positions)?;

    tx.execute(
        "INSERT INTO position_history_rtree (id, min_lat, max_lat, min_lon, max_lon)
         SELECT id, latitude, latitude, longitude, longitude
         FROM position_history WHERE id > ?1",
        params![max_id_before],
    )?;

    tx.commit()?;

    info!(
        "  ✅  {} → {} positions, {} destinations, {} draughts, {} nav_statuses, \
         {} ship_names, {} static, {} minimal vessels",
        filename,
        positions.len(),
        destinations.len(),
        draughts.len(),
        nav_statuses.len(),
        ship_names.len(),
        vessels_static.len(),
        vessels_minimal.len(),
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
    for chunk in rows.chunks(config::BATCH_SIZE) {
        for r in chunk {
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
    }
    Ok(())
}

fn insert_destinations(tx: &Transaction, rows: &[DestinationRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(DESTINATION_INSERT)?;
    for r in rows {
        stmt.execute(params![r.mmsi, r.destination, r.estimated_arrival, r.timestamp_utc])?;
    }
    Ok(())
}

fn insert_draughts(tx: &Transaction, rows: &[DraughtRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(DRAUGHT_INSERT)?;
    for r in rows {
        stmt.execute(params![r.mmsi, r.draught, r.timestamp_utc])?;
    }
    Ok(())
}

fn insert_nav_statuses(tx: &Transaction, rows: &[NavStatusRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(NAV_STATUS_INSERT)?;
    for r in rows {
        stmt.execute(params![r.mmsi, r.navigational_status, r.timestamp_utc])?;
    }
    Ok(())
}

fn insert_ship_names(tx: &Transaction, rows: &[ShipNameRow]) -> Result<(), rusqlite::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut stmt = tx.prepare_cached(SHIP_NAME_INSERT)?;
    for r in rows {
        stmt.execute(params![r.mmsi, r.ship_name, r.timestamp_utc])?;
    }
    Ok(())
}

// =========================================================================
// File processing
// =========================================================================

fn is_already_processed(filepath: &Path, backup_dir: &Path) -> bool {
    if let Some(name) = filepath.file_name() {
        backup_dir.join(name).exists()
    } else {
        false
    }
}

pub fn process_file(
    conn: &mut Connection,
    filepath: &Path,
    backup_dir: &Path,
    non_commercial_mmsis: &mut HashSet<i64>,
) -> bool {
    let filename = filepath
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if is_already_processed(filepath, backup_dir) {
        info!("  ⏩  {} already ingested, skipping.", filename);
        return true;
    }

    info!("  📄  Processing {} …", filename);

    let file = match fs::File::open(filepath) {
        Ok(f) => f,
        Err(e) => {
            error!("  ❌  Cannot open {}: {}", filename, e);
            return false;
        }
    };

    let reader = BufReader::with_capacity(256 * 1024, file);

    let mut positions: Vec<PositionRow> = Vec::new();
    let mut vessels_static: Vec<VesselStaticRow> = Vec::new();
    let mut vessels_minimal: HashMap<i64, VesselMinimalRow> = HashMap::new();
    let mut destinations: Vec<DestinationRow> = Vec::new();
    let mut draughts: Vec<DraughtRow> = Vec::new();
    let mut nav_statuses: Vec<NavStatusRow> = Vec::new();
    let mut ship_names: Vec<ShipNameRow> = Vec::new();
    let mut total_lines: u64 = 0;
    let mut skipped_non_commercial: u64 = 0;

    for line_result in reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        total_lines += 1;

        let record: AisRecord = match serde_json::from_str(&line) {
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

        // Skip non-ship MMSIs
        if !is_ship_mmsi(mmsi) {
            continue;
        }

        // Skip known non-commercial
        if non_commercial_mmsis.contains(&mmsi) {
            skipped_non_commercial += 1;
            continue;
        }

        match msg_type.as_str() {
            "ShipStaticData" | "StaticDataReport" => {
                if let Some(vs) = extract_vessel_static(msg, meta) {
                    if is_excluded_ship_type(vs.ship_type) {
                        non_commercial_mmsis.insert(mmsi);
                        skipped_non_commercial += 1;
                        continue;
                    }
                    vessels_static.push(vs);
                }

                if let Some(dest) = extract_destination(msg, meta) {
                    destinations.push(dest);
                }
                if let Some(dr) = extract_draught(msg, meta) {
                    draughts.push(dr);
                }
                if let Some(sn) = extract_ship_name(msg, meta) {
                    ship_names.push(sn);
                }
            }

            "PositionReport"
            | "StandardClassBPositionReport"
            | "ExtendedClassBPositionReport"
            | "StandardSearchAndRescueAircraftReport" => {
                if let Some(pos) = extract_position(msg, meta) {
                    positions.push(pos);
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
                            seen_utc: parse_ais_time(meta.time_utc.as_deref().unwrap_or("")),
                        },
                    );
                }

                if let Some(ns) = extract_nav_status(msg, meta) {
                    nav_statuses.push(ns);
                }
            }

            _ => {
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
                        seen_utc: parse_ais_time(meta.time_utc.as_deref().unwrap_or("")),
                    },
                );
            }
        }
    }

    // Write to DB
    match write_to_db(
        conn,
        &vessels_static,
        &vessels_minimal,
        &positions,
        &destinations,
        &draughts,
        &nav_statuses,
        &ship_names,
        &filename,
    ) {
        Ok(()) => {}
        Err(e) => {
            error!("  ❌  Failed to ingest {}: {}", filename, e);
            return false;
        }
    }

    info!(
        "      ({} lines, {} skipped non-commercial)",
        total_lines, skipped_non_commercial
    );

    true
}

pub fn move_to_backup(filepath: &Path, backup_dir: &Path) {
    fs::create_dir_all(backup_dir).ok();
    if let Some(name) = filepath.file_name() {
        let dest = backup_dir.join(name);
        match fs::rename(filepath, &dest) {
            Ok(()) => info!("  📦  Moved → {}", dest.display()),
            Err(e) => {
                // rename fails across filesystems — fall back to copy + remove
                match fs::copy(filepath, &dest) {
                    Ok(_) => {
                        fs::remove_file(filepath).ok();
                        info!("  📦  Moved → {}", dest.display());
                    }
                    Err(e2) => error!(
                        "  ❌  Failed to move {} → {}: rename={}, copy={}",
                        filepath.display(),
                        dest.display(),
                        e,
                        e2
                    ),
                }
            }
        }
    }
}

pub fn run_once(
    conn: &mut Connection,
    raw_dir: &Path,
    backup_dir: &Path,
    non_commercial_mmsis: &mut HashSet<i64>,
) {
    let pattern = raw_dir.join(config::JSONL_GLOB);
    let pattern_str = pattern.to_string_lossy();

    let mut files: Vec<PathBuf> = glob::glob(&pattern_str)
        .unwrap_or_else(|e| {
            error!("Bad glob pattern: {}", e);
            std::process::exit(1);
        })
        .filter_map(|r| r.ok())
        .collect();

    files.sort();

    if files.is_empty() {
        info!("No new JSONL files found in {}", raw_dir.display());
        return;
    }

    info!("Found {} JSONL file(s) to process.", files.len());

    for fp in &files {
        if process_file(conn, fp, backup_dir, non_commercial_mmsis) {
            move_to_backup(fp, backup_dir);
        }
    }
}
