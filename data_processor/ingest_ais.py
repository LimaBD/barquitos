"""
AIS JSONL → SQLite ingestion script.

Reads every unprocessed *.jsonl file produced by the AIS collector,
parses each line, inserts the data into the normalised database, then
moves the file to the backup (processed) folder.

Usage:
    python ingest_ais.py            # one-shot: process pending files then exit
    python ingest_ais.py --watch    # loop forever, checking every 30 s

Can safely be re-run — already-ingested files are skipped.
"""

import argparse
import json
import logging
import os
import shutil
import sys
import time
from datetime import datetime, timezone
from glob import glob
from pathlib import Path

# ---------------------------------------------------------------------------
# Ensure package imports work when running as a script
# ---------------------------------------------------------------------------
_here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, _here)

import config  # noqa: E402
from db import init_db  # noqa: E402

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
log = logging.getLogger("ingest_ais")

# ---------------------------------------------------------------------------
# Time parsing helper
# ---------------------------------------------------------------------------
def parse_ais_time(time_str: str) -> str:
    """
    Convert the Go-style timestamp from AISstream metadata into ISO-8601.

    Example input : '2026-03-04 04:34:59.795811956 +0000 UTC'
    Example output: '2026-03-04T04:34:59.795811+00:00'
    """
    try:
        # Strip trailing " UTC" and the tz offset "+0000" → use as-is (already UTC)
        # Python's %f only handles 6 digits, so truncate nanoseconds
        clean = time_str.replace(" UTC", "").replace(" +0000", "")
        # Truncate fractional seconds to microseconds (6 digits)
        dot_pos = clean.find(".")
        if dot_pos != -1:
            frac = clean[dot_pos + 1:]
            clean = clean[: dot_pos + 1] + frac[:6]
        dt = datetime.strptime(clean, "%Y-%m-%d %H:%M:%S.%f")
        return dt.replace(tzinfo=timezone.utc).isoformat()
    except Exception:
        return time_str  # return as-is rather than crash


# ---------------------------------------------------------------------------
# Row extraction
# ---------------------------------------------------------------------------
def _extract_position(msg: dict, meta: dict, source_file: str) -> dict | None:
    """Extract a position row from a PositionReport / StandardClassBPositionReport / etc."""
    report = (
        msg.get("PositionReport")
        or msg.get("StandardClassBPositionReport")
        or msg.get("ExtendedClassBPositionReport")
        or msg.get("StandardSearchAndRescueAircraftReport")
    )
    if report is None:
        return None

    lat = report.get("Latitude") or meta.get("latitude")
    lon = report.get("Longitude") or meta.get("longitude")
    if lat is None or lon is None:
        return None

    return {
        "mmsi": meta.get("MMSI"),
        "timestamp_utc": parse_ais_time(meta.get("time_utc", "")),
        "latitude": lat,
        "longitude": lon,
        "sog": report.get("Sog"),
        "cog": report.get("Cog"),
        "true_heading": report.get("TrueHeading"),
        "nav_status": report.get("NavigationalStatus"),
        "rot": report.get("RateOfTurn"),
        "message_id": report.get("MessageID"),
        "position_accuracy": int(report.get("PositionAccuracy", False)),
        "raim": int(report.get("Raim", False)),
        "source_file": source_file,
    }


def _extract_vessel_static(msg: dict, meta: dict) -> dict | None:
    """Extract vessel identity from ShipStaticData (AIS message 5 / 24)."""
    static = msg.get("ShipStaticData") or msg.get("StaticDataReport")
    if static is None:
        return None

    dim = static.get("Dimension", {}) or {}
    report_a = static.get("ReportA", {}) or {}
    name = (
        static.get("Name")
        or report_a.get("Name")
        or meta.get("ShipName", "")
    )

    return {
        "mmsi": meta.get("MMSI"),
        "ship_name": name.strip() if name else None,
        "call_sign": (static.get("CallSign") or "").strip() or None,
        "imo_number": static.get("ImoNumber") or None,
        "ship_type": static.get("Type") or static.get("ShipType"),
        "dim_a": dim.get("A"),
        "dim_b": dim.get("B"),
        "dim_c": dim.get("C"),
        "dim_d": dim.get("D"),
        "max_draught": static.get("MaximumStaticDraught"),
        "ais_version": static.get("AisVersion"),
        "fix_type": static.get("FixType"),
        "seen_utc": parse_ais_time(meta.get("time_utc", "")),
    }


def _extract_voyage(msg: dict, meta: dict, source_file: str) -> dict | None:
    """Extract voyage info (destination & ETA) from ShipStaticData."""
    static = msg.get("ShipStaticData")
    if static is None:
        return None
    dest = (static.get("Destination") or "").strip()
    eta = static.get("Eta", {}) or {}
    if not dest and not any(eta.values()):
        return None
    return {
        "mmsi": meta.get("MMSI"),
        "destination": dest or None,
        "eta_month": eta.get("Month"),
        "eta_day": eta.get("Day"),
        "eta_hour": eta.get("Hour"),
        "eta_minute": eta.get("Minute"),
        "reported_utc": parse_ais_time(meta.get("time_utc", "")),
        "source_file": source_file,
    }


def _extract_vessel_from_meta(meta: dict) -> dict:
    """Minimal vessel record from MetaData (always available)."""
    return {
        "mmsi": meta.get("MMSI"),
        "ship_name": (meta.get("ShipName") or "").strip() or None,
        "seen_utc": parse_ais_time(meta.get("time_utc", "")),
    }


# ---------------------------------------------------------------------------
# Bulk writers
# ---------------------------------------------------------------------------
_POSITION_INSERT = """
INSERT INTO position_reports
    (mmsi, timestamp_utc, latitude, longitude, sog, cog, true_heading,
     nav_status, rot, message_id, position_accuracy, raim, source_file)
VALUES
    (:mmsi, :timestamp_utc, :latitude, :longitude, :sog, :cog, :true_heading,
     :nav_status, :rot, :message_id, :position_accuracy, :raim, :source_file)
"""

_RTREE_INSERT = """
INSERT INTO position_reports_rtree (id, min_lat, max_lat, min_lon, max_lon)
VALUES (?, ?, ?, ?, ?)
"""

_VESSEL_UPSERT = """
INSERT INTO vessels
    (mmsi, ship_name, call_sign, imo_number, ship_type,
     dim_a, dim_b, dim_c, dim_d, max_draught,
     ais_version, fix_type, first_seen_utc, last_seen_utc)
VALUES
    (:mmsi, :ship_name, :call_sign, :imo_number, :ship_type,
     :dim_a, :dim_b, :dim_c, :dim_d, :max_draught,
     :ais_version, :fix_type, :seen_utc, :seen_utc)
ON CONFLICT(mmsi) DO UPDATE SET
    ship_name      = COALESCE(excluded.ship_name,      vessels.ship_name),
    call_sign      = COALESCE(excluded.call_sign,      vessels.call_sign),
    imo_number     = COALESCE(excluded.imo_number,      vessels.imo_number),
    ship_type      = COALESCE(excluded.ship_type,       vessels.ship_type),
    dim_a          = COALESCE(excluded.dim_a,            vessels.dim_a),
    dim_b          = COALESCE(excluded.dim_b,            vessels.dim_b),
    dim_c          = COALESCE(excluded.dim_c,            vessels.dim_c),
    dim_d          = COALESCE(excluded.dim_d,            vessels.dim_d),
    max_draught    = COALESCE(excluded.max_draught,      vessels.max_draught),
    ais_version    = COALESCE(excluded.ais_version,      vessels.ais_version),
    fix_type       = COALESCE(excluded.fix_type,         vessels.fix_type),
    last_seen_utc  = MAX(excluded.last_seen_utc, vessels.last_seen_utc),
    updated_at     = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
"""

_VESSEL_UPSERT_MINIMAL = """
INSERT INTO vessels
    (mmsi, ship_name, first_seen_utc, last_seen_utc)
VALUES
    (:mmsi, :ship_name, :seen_utc, :seen_utc)
ON CONFLICT(mmsi) DO UPDATE SET
    ship_name     = COALESCE(excluded.ship_name,  vessels.ship_name),
    last_seen_utc = MAX(excluded.last_seen_utc, vessels.last_seen_utc),
    updated_at    = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
"""

_VOYAGE_INSERT = """
INSERT INTO voyages
    (mmsi, destination, eta_month, eta_day, eta_hour, eta_minute,
     reported_utc, source_file)
VALUES
    (:mmsi, :destination, :eta_month, :eta_day, :eta_hour, :eta_minute,
     :reported_utc, :source_file)
"""

_FILE_RECORD = """
INSERT OR IGNORE INTO processed_files
    (filename, row_count, positions_added, voyages_added, vessels_upserted)
VALUES (?, ?, ?, ?, ?)
"""


# ---------------------------------------------------------------------------
# File processing
# ---------------------------------------------------------------------------
def process_file(conn, filepath: str) -> bool:
    """
    Parse a single JSONL file and load its data into the database.

    Returns True if the file was processed (or was already processed).
    """
    filename = os.path.basename(filepath)

    # Already done?
    cur = conn.execute(
        "SELECT 1 FROM processed_files WHERE filename = ?", (filename,)
    )
    if cur.fetchone():
        log.info("  ⏩  %s already ingested, skipping.", filename)
        return True

    log.info("  📄  Processing %s …", filename)

    positions = []
    vessels_static = []
    vessels_minimal = []
    voyages = []
    total_lines = 0

    with open(filepath, "r", encoding="utf-8") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            total_lines += 1
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue

            msg = record.get("Message", {})
            meta = record.get("MetaData", {})
            msg_type = record.get("MessageType", "")

            # --- Position ---------------------------------------------------
            if msg_type in (
                "PositionReport",
                "StandardClassBPositionReport",
                "ExtendedClassBPositionReport",
                "StandardSearchAndRescueAircraftReport",
            ):
                pos = _extract_position(msg, meta, filename)
                if pos and pos["mmsi"]:
                    positions.append(pos)
                    # We also create a minimal vessel record
                    vessels_minimal.append(_extract_vessel_from_meta(meta))

            # --- Static data ------------------------------------------------
            elif msg_type in ("ShipStaticData", "StaticDataReport"):
                vs = _extract_vessel_static(msg, meta)
                if vs and vs["mmsi"]:
                    vessels_static.append(vs)
                voy = _extract_voyage(msg, meta, filename)
                if voy and voy["mmsi"]:
                    voyages.append(voy)

            # --- Other message types (BaseStationReport, etc.) --------------
            else:
                # At minimum track the vessel from metadata if MMSI present
                if meta.get("MMSI"):
                    vessels_minimal.append(_extract_vessel_from_meta(meta))

    # ---- Write to DB in one transaction ------------------------------------
    try:
        cur = conn.cursor()

        # 1) Upsert vessels (static first — richer data; then minimal)
        for v in vessels_static:
            cur.execute(_VESSEL_UPSERT, v)
        for v in vessels_minimal:
            cur.execute(_VESSEL_UPSERT_MINIMAL, v)

        # 2) Get the current max id so we know which rows are new
        max_id_before = cur.execute(
            "SELECT COALESCE(MAX(id), 0) FROM position_reports"
        ).fetchone()[0]

        # Insert positions in batches
        for i in range(0, len(positions), config.BATCH_SIZE):
            batch = positions[i : i + config.BATCH_SIZE]
            cur.executemany(_POSITION_INSERT, batch)

        # Populate the R*Tree from all newly inserted rows
        cur.execute(
            """INSERT INTO position_reports_rtree (id, min_lat, max_lat, min_lon, max_lon)
               SELECT id, latitude, latitude, longitude, longitude
               FROM position_reports
               WHERE id > ?""",
            (max_id_before,),
        )

        # 3) Insert voyages
        if voyages:
            cur.executemany(_VOYAGE_INSERT, voyages)

        # 4) Record the file as processed
        cur.execute(
            _FILE_RECORD,
            (filename, total_lines, len(positions), len(voyages), len(vessels_static)),
        )
        conn.commit()
    except Exception:
        conn.rollback()
        log.exception("  ❌  Failed to ingest %s", filename)
        return False

    log.info(
        "  ✅  %s → %d positions, %d voyages, %d vessel updates (%d lines)",
        filename,
        len(positions),
        len(voyages),
        len(vessels_static),
        total_lines,
    )
    return True


def move_to_backup(filepath: str, backup_dir: str):
    """Move an ingested JSONL file to the backup folder."""
    os.makedirs(backup_dir, exist_ok=True)
    dest = os.path.join(backup_dir, os.path.basename(filepath))
    shutil.move(filepath, dest)
    log.info("  📦  Moved → %s", dest)


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def run_once(conn, raw_dir: str, backup_dir: str):
    """Scan the raw directory, ingest new files, move them to backup."""
    pattern = os.path.join(raw_dir, config.JSONL_GLOB)
    files = sorted(glob(pattern))

    if not files:
        log.info("No new JSONL files found in %s", raw_dir)
        return

    log.info("Found %d JSONL file(s) to process.", len(files))

    for fp in files:
        ok = process_file(conn, fp)
        if ok:
            move_to_backup(fp, backup_dir)


def main():
    parser = argparse.ArgumentParser(description="Ingest AIS JSONL into SQLite")
    parser.add_argument(
        "--watch",
        action="store_true",
        help="Run continuously, checking for new files every 30 seconds.",
    )
    parser.add_argument(
        "--interval",
        type=int,
        default=30,
        help="Seconds between checks in --watch mode (default: 30).",
    )
    parser.add_argument(
        "--input-dir",
        type=str,
        default=None,
        help="Override the raw JSONL input directory (default: from config).",
    )
    parser.add_argument(
        "--backup-dir",
        type=str,
        default=None,
        help="Override the backup directory for processed files (default: from config).",
    )
    args = parser.parse_args()

    raw_dir = args.input_dir or config.AIS_RAW_DIR
    backup_dir = args.backup_dir or config.AIS_BACKUP_DIR

    log.info("Database : %s", config.DATABASE_PATH)
    log.info("Raw dir  : %s", raw_dir)
    log.info("Backup   : %s", backup_dir)

    conn = init_db(config.DATABASE_PATH)

    if args.watch:
        log.info("👀  Watch mode — checking every %d s  (Ctrl+C to stop)", args.interval)
        try:
            while True:
                run_once(conn, raw_dir, backup_dir)
                time.sleep(args.interval)
        except KeyboardInterrupt:
            log.info("Stopped.")
    else:
        run_once(conn, raw_dir, backup_dir)

    conn.close()
    log.info("Done.")


if __name__ == "__main__":
    main()
