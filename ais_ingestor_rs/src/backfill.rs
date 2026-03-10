// ─── AIS Backfill ───────────────────────────────────────────────────────────
//
// One-shot backfill tool: reads JSONL files from a given directory (or the
// default test_data/), ingests them into the Parquet Data Lake, but does NOT
// move them.  Useful for:
//   • Initial testing with sample data
//   • Re-processing historical JSONL archives
//
// Usage:
//   cargo run --bin ais_backfill [directory]
//
// If no directory is given, uses ais_ingestor_rs/test_data/.
// ────────────────────────────────────────────────────────────────────────────

mod config;
mod edge_state;
mod parser;
mod schema;
mod writer;

use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use chrono::Utc;
use log::{error, info, warn};

use crate::edge_state::EdgeState;
use crate::parser::{ParsedMessage, StaticRecord};
use crate::writer::*;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let args: Vec<String> = env::args().collect();

    // Allow overriding lake root via LAKE_ROOT env var for testing
    let lake_root = env::var("LAKE_ROOT").unwrap_or_else(|_| config::LAKE_ROOT.to_string());

    let input_dir = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        // Default: test_data/ relative to the cargo manifest dir
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        PathBuf::from(manifest_dir).join("test_data")
    };

    info!("═══════════════════════════════════════════════════════════════");
    info!("  AIS Backfill Tool v{}", env!("CARGO_PKG_VERSION"));
    info!("  Input dir:  {}", input_dir.display());
    info!("  Lake root:  {}", lake_root);
    info!("  (files will NOT be moved)");
    info!("═══════════════════════════════════════════════════════════════");

    // Collect JSONL files
    let mut jsonl_files: Vec<PathBuf> = match fs::read_dir(&input_dir) {
        Ok(entries) => entries
            .filter_map(|e| {
                let p = e.ok()?.path();
                if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    Some(p)
                } else {
                    None
                }
            })
            .collect(),
        Err(e) => {
            error!("Cannot read directory {}: {e}", input_dir.display());
            std::process::exit(1);
        }
    };

    jsonl_files.sort();
    info!("Found {} JSONL file(s)", jsonl_files.len());

    if jsonl_files.is_empty() {
        info!("Nothing to do.");
        return;
    }

    // Edge state
    let mut edge = EdgeState::new();

    // Buffers
    let mut pos_buf = PositionBuf::default();
    let mut draught_buf = DraughtBuf::default();
    let mut nav_buf = NavStatusBuf::default();
    let mut dest_buf = DestinationBuf::default();
    let mut static_buf = VesselStaticBuf::default();
    let mut name_buf = VesselNameHistoryBuf::default();
    let mut health = HealthCounters::default();

    let batch_start = Utc::now();
    let mut total_lines: u64 = 0;

    for file_path in &jsonl_files {
        info!("  Reading: {}", file_path.display());
        let file = match fs::File::open(file_path) {
            Ok(f) => f,
            Err(e) => {
                error!("  Cannot open: {e}");
                continue;
            }
        };
        let reader = BufReader::with_capacity(64 * 1024, file);

        for line_result in reader.lines() {
            let line = match line_result {
                Ok(l) => l,
                Err(e) => {
                    warn!("  Line read error: {e}");
                    health.parse_errors += 1;
                    continue;
                }
            };
            if line.trim().is_empty() {
                continue;
            }

            total_lines += 1;
            health.total_messages += 1;

            if total_lines % config::LOG_EVERY_N_MESSAGES == 0 {
                info!("  … parsed {total_lines} lines so far");
            }

            match parser::parse_line(&line) {
                ParsedMessage::Position(p) => {
                    health.position_reports += 1;
                    health.unique_mmsis.insert(p.mmsi);

                    // Edge filter nav status from position reports
                    if let Some(status) = p.nav_status {
                        if let Some((new_s, _old_s)) = edge.check_nav_status(p.mmsi, status) {
                            nav_buf.mmsi.push(p.mmsi);
                            nav_buf.timestamp_s.push(p.timestamp_s);
                            nav_buf.nav_status.push(new_s);
                        }
                    }

                    // Edge filter ship name from position metadata
                    if let Some(ref name) = p.ship_name {
                        if let Some(new_name) = edge.check_ship_name(p.mmsi, name) {
                            name_buf.mmsi.push(p.mmsi);
                            name_buf.timestamp_s.push(p.timestamp_s);
                            name_buf.ship_name.push(new_name);
                        }
                    }

                    pos_buf.mmsi.push(p.mmsi);
                    pos_buf.timestamp_s.push(p.timestamp_s);
                    pos_buf.latitude.push(p.latitude);
                    pos_buf.longitude.push(p.longitude);
                    pos_buf.sog.push(p.sog);
                    pos_buf.cog.push(p.cog);
                    pos_buf.heading.push(p.heading);
                    pos_buf.rot.push(p.rot);
                }
                ParsedMessage::Static(s) => {
                    health.static_data_reports += 1;
                    health.unique_mmsis.insert(s.mmsi);
                    process_static(s, &mut edge, &mut draught_buf, &mut dest_buf, &mut static_buf, &mut name_buf, &mut health);
                }
                ParsedMessage::Ignored => {}
                ParsedMessage::Error(e) => {
                    health.parse_errors += 1;
                    if health.parse_errors <= 20 {
                        warn!("  Parse error: {e}");
                    }
                }
            }
        }
    }

    info!("Parsed {total_lines} total lines from {} files.", jsonl_files.len());
    info!("Flushing buffers to Parquet at {} …", lake_root);

    match flush_positions(&lake_root, &mut pos_buf) {
        Ok(n) => info!("  Positions: {n}"),
        Err(e) => error!("  Position flush error: {e}"),
    }
    match flush_draughts(&lake_root, &mut draught_buf) {
        Ok(n) => info!("  Draughts: {n}"),
        Err(e) => error!("  Draught flush error: {e}"),
    }
    match flush_nav_statuses(&lake_root, &mut nav_buf) {
        Ok(n) => info!("  Nav statuses: {n}"),
        Err(e) => error!("  Nav status flush error: {e}"),
    }
    match flush_destinations(&lake_root, &mut dest_buf) {
        Ok(n) => info!("  Destinations: {n}"),
        Err(e) => error!("  Destination flush error: {e}"),
    }
    match flush_vessel_static(&lake_root, &mut static_buf) {
        Ok(n) => info!("  Vessel static: {n}"),
        Err(e) => error!("  Vessel static flush error: {e}"),
    }
    match flush_vessel_name_history(&lake_root, &mut name_buf) {
        Ok(n) => info!("  Name history: {n}"),
        Err(e) => error!("  Name history flush error: {e}"),
    }

    let batch_end = Utc::now();
    match flush_health(
        &lake_root,
        &mut health,
        batch_start.timestamp(),
        batch_end.timestamp(),
    ) {
        Ok(()) => info!("  Health metrics flushed."),
        Err(e) => error!("  Health flush error: {e}"),
    }

    info!("═══════════════════════════════════════════════════════════════");
    info!("  Backfill complete.");
    info!("  Edge state tracked {} vessels.", edge.vessel_count());
    info!("═══════════════════════════════════════════════════════════════");
}

/// Process a ShipStaticData record through edge filters (same logic as main).
fn process_static(
    s: StaticRecord,
    edge: &mut EdgeState,
    draught_buf: &mut DraughtBuf,
    dest_buf: &mut DestinationBuf,
    static_buf: &mut VesselStaticBuf,
    name_buf: &mut VesselNameHistoryBuf,
    _health: &mut HealthCounters,
) {
    // Edge filter: ship name
    if let Some(ref name) = s.ship_name {
        if let Some(new_name) = edge.check_ship_name(s.mmsi, name) {
            name_buf.mmsi.push(s.mmsi);
            name_buf.timestamp_s.push(s.timestamp_s);
            name_buf.ship_name.push(new_name);
        }
    }

    if let Some(draught) = s.draught {
        if let Some((new_d, _old_d)) = edge.check_draught(s.mmsi, draught) {
            draught_buf.mmsi.push(s.mmsi);
            draught_buf.timestamp_s.push(s.timestamp_s);
            draught_buf.draught.push(new_d);
        }
    }

    if let Some(ref dest) = s.destination {
        if let Some((new_dest, _old_dest)) = edge.check_destination(s.mmsi, dest) {
            dest_buf.mmsi.push(s.mmsi);
            dest_buf.timestamp_s.push(s.timestamp_s);
            dest_buf.destination.push(new_dest);
            dest_buf.eta_utc.push(s.eta_utc);
        }
    }

    // Compute length and beam from AIS dimensions
    let length_overall = match (s.dim_a, s.dim_b) {
        (Some(a), Some(b)) if (a + b) > 0 => Some(a + b),
        _ => None,
    };
    let beam = match (s.dim_c, s.dim_d) {
        (Some(c), Some(d)) if (c + d) > 0 => Some(c + d),
        _ => None,
    };

    // Edge filter: vessel static — only write when any field changes
    let key = edge_state::VesselStaticKey {
        call_sign: s.call_sign.clone(),
        imo_number: s.imo_number,
        ship_type: s.ship_type,
        length_overall,
        beam,
    };
    if edge.check_vessel_static(s.mmsi, key) {
        static_buf.mmsi.push(s.mmsi);
        static_buf.timestamp_s.push(s.timestamp_s);
        static_buf.call_sign.push(s.call_sign);
        static_buf.imo_number.push(s.imo_number);
        static_buf.ship_type.push(s.ship_type);
        static_buf.length_overall.push(length_overall);
        static_buf.beam.push(beam);
    }
}
