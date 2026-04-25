// ─── AIS Backfill Tool ────────────────────────────────────────────────────────
//
// One-shot tool: reads all JSONL files from a directory, ingests them into
// the Parquet lake, and exits.  Does NOT move the source files.
//
// Usage:
//   ais_backfill [input-dir]  [--config <path>]
//
// Defaults:
//   input-dir  = ./test_data/
//   config     = standard search order (see barquitos-common config docs)
//
// The lake_root is taken from config.toml [ingestor] section.
// Override with LAKE_ROOT env var if needed.
// ─────────────────────────────────────────────────────────────────────────────

mod edge_state;
mod parser;
mod schema;
mod writer;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use log::{error, info, warn};

use barquitos_common::config::IngestorFile;
use barquitos_common::safe_io::{quarantine_file, read_lines_with_timeout, ReadLinesResult};

use crate::edge_state::EdgeState;
use crate::parser::ParsedMessage;
use crate::writer::*;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let args: Vec<String> = std::env::args().collect();

    let cfg = IngestorFile::load_from_args_or_env(&args).unwrap_or_else(|e| {
        error!("Failed to load config: {e}");
        std::process::exit(1);
    });

    // Input dir: first positional arg that is not a flag
    let input_dir = args.iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .unwrap_or("test_data");

    // lake_root: LAKE_ROOT env var overrides config
    let lake_root = std::env::var("LAKE_ROOT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.ingestor.lake_root.clone());

    info!("══ AIS Backfill v{} ══", env!("CARGO_PKG_VERSION"));
    info!("  Input dir : {input_dir}");
    info!("  Lake root : {lake_root}");
    info!("  Quarantine: {}", cfg.ingestor.quarantine_dir);

    let mut files: Vec<PathBuf> = match fs::read_dir(input_dir) {
        Err(e) => { error!("Cannot read {input_dir}: {e}"); std::process::exit(1); }
        Ok(rd) => rd.filter_map(|e| {
            let e = e.ok()?;
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("jsonl") { Some(p) } else { None }
        }).collect(),
    };
    files.sort();

    if files.is_empty() {
        info!("No .jsonl files found in {input_dir}. Nothing to do.");
        return;
    }
    info!("Found {} file(s) to backfill.", files.len());

    let read_timeout = Duration::from_secs(cfg.ingestor.file_read_timeout_secs);
    let buf_cap      = cfg.parquet.buf_reader_capacity;

    let mut edge   = EdgeState::new();
    let mut pos    = PositionBuf::default();
    let mut dr     = DraughtBuf::default();
    let mut nav    = NavStatusBuf::default();
    let mut dest   = DestinationBuf::default();
    let mut stat   = VesselStaticBuf::default();
    let mut names  = VesselNameHistoryBuf::default();
    let mut health = HealthCounters::default();

    for (i, file_path) in files.iter().enumerate() {
        info!("[{}/{}] {}", i + 1, files.len(), file_path.display());

        let lines = match read_lines_with_timeout(file_path, read_timeout, buf_cap) {
            ReadLinesResult::Lines(l)     => l,
            ReadLinesResult::Timeout      => {
                warn!("  ⚠ Timed out — quarantining");
                quarantine_file(file_path, &cfg.ingestor.quarantine_dir);
                continue;
            }
            ReadLinesResult::OpenError(e) => {
                warn!("  ⚠ Open error ({e}) — quarantining");
                quarantine_file(file_path, &cfg.ingestor.quarantine_dir);
                continue;
            }
        };

        for line in &lines {
            let line = line.trim();
            if line.is_empty() { continue; }
            health.total_messages += 1;

            match parser::parse_line(line) {
                ParsedMessage::Position(p) => {
                    health.position_reports += 1;
                    health.unique_mmsis.insert(p.mmsi);
                    if let Some(s) = p.nav_status {
                        if let Some((ns, _)) = edge.check_nav_status(p.mmsi, s) {
                            nav.mmsi.push(p.mmsi); nav.timestamp_s.push(p.timestamp_s); nav.nav_status.push(ns);
                        }
                    }
                    if let Some(ref n) = p.ship_name {
                        if let Some(nn) = edge.check_ship_name(p.mmsi, n) {
                            names.mmsi.push(p.mmsi); names.timestamp_s.push(p.timestamp_s); names.ship_name.push(nn);
                        }
                    }
                    pos.mmsi.push(p.mmsi); pos.timestamp_s.push(p.timestamp_s);
                    pos.latitude.push(p.latitude); pos.longitude.push(p.longitude);
                    pos.sog.push(p.sog); pos.cog.push(p.cog);
                    pos.heading.push(p.heading); pos.rot.push(p.rot);
                }
                ParsedMessage::Static(s) => {
                    health.static_data_reports += 1;
                    health.unique_mmsis.insert(s.mmsi);
                    if let Some(ref n) = s.ship_name {
                        if let Some(nn) = edge.check_ship_name(s.mmsi, n) {
                            names.mmsi.push(s.mmsi); names.timestamp_s.push(s.timestamp_s); names.ship_name.push(nn);
                        }
                    }
                    if let Some(d) = s.draught {
                        if let Some((nd, _)) = edge.check_draught(s.mmsi, d) {
                            dr.mmsi.push(s.mmsi); dr.timestamp_s.push(s.timestamp_s); dr.draught.push(nd);
                        }
                    }
                    if let Some(ref d) = s.destination {
                        if let Some((nd, _)) = edge.check_destination(s.mmsi, d) {
                            dest.mmsi.push(s.mmsi); dest.timestamp_s.push(s.timestamp_s);
                            dest.destination.push(nd); dest.eta_utc.push(s.eta_utc);
                        }
                    }
                    let len = match (s.dim_a, s.dim_b) { (Some(a), Some(b)) if a+b>0 => Some(a+b), _ => None };
                    let beam = match (s.dim_c, s.dim_d) { (Some(c), Some(d)) if c+d>0 => Some(c+d), _ => None };
                    let key  = edge_state::VesselStaticKey {
                        call_sign: s.call_sign.clone(), imo_number: s.imo_number,
                        ship_type: s.ship_type, length_overall: len, beam,
                    };
                    if edge.check_vessel_static(s.mmsi, key) {
                        stat.mmsi.push(s.mmsi); stat.timestamp_s.push(s.timestamp_s);
                        stat.call_sign.push(s.call_sign); stat.imo_number.push(s.imo_number);
                        stat.ship_type.push(s.ship_type); stat.length_overall.push(len); stat.beam.push(beam);
                    }
                }
                ParsedMessage::Ignored  => {}
                ParsedMessage::Error(e) => {
                    health.parse_errors += 1;
                    if health.parse_errors <= 5 { warn!("  Parse error: {e}"); }
                }
            }
        }
    }

    info!("Flushing to Parquet …");
    let t0  = Utc::now();
    let p_n = flush_positions(&lake_root, &mut pos,   &cfg.parquet).unwrap_or_else(|e| { error!("{e}"); 0 });
    let d_n = flush_draughts(&lake_root, &mut dr,     &cfg.parquet).unwrap_or_else(|e| { error!("{e}"); 0 });
    let v_n = flush_nav_statuses(&lake_root, &mut nav, &cfg.parquet).unwrap_or_else(|e| { error!("{e}"); 0 });
    let t_n = flush_destinations(&lake_root, &mut dest, &cfg.parquet).unwrap_or_else(|e| { error!("{e}"); 0 });
    let s_n = flush_vessel_static(&lake_root, &mut stat, &cfg.parquet).unwrap_or_else(|e| { error!("{e}"); 0 });
    let n_n = flush_vessel_name_history(&lake_root, &mut names, &cfg.parquet).unwrap_or_else(|e| { error!("{e}"); 0 });
    let t1  = Utc::now();
    let _ = flush_health(&lake_root, &mut health, t0.timestamp(), t1.timestamp(), &cfg.parquet);

    info!("══ BACKFILL SUMMARY ══");
    info!("  Files processed : {}", files.len());
    info!("  Total messages  : {}", health.total_messages);
    info!("  Positions       : {p_n}");
    info!("  Draughts        : {d_n}");
    info!("  Nav status      : {v_n}");
    info!("  Destinations    : {t_n}");
    info!("  Vessel static   : {s_n}");
    info!("  Name history    : {n_n}");
    info!("  Parse errors    : {}", health.parse_errors);
    info!("  Unique vessels  : {}", edge.vessel_count());
}
