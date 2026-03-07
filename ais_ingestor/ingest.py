"""
AIS JSONL → SQLite ingestion script.

Reads every unprocessed *.jsonl file produced by the AIS collector,
parses each line, inserts the data into the normalised database, then
moves the file to the backup (processed) folder.

Usage:
    python -m ais_ingestor.ingest            # one-shot
    python -m ais_ingestor.ingest --watch    # loop forever, checking every 30 s

Can safely be re-run — already-ingested files are skipped (by checking the
backup directory).

Performance optimisations:
  - Vessel-minimal upserts are deduplicated by MMSI.
  - R*Tree rows are inserted in a single batch via a range sub-select.
  - Non-commercial vessels (yachts, sailing boats) are filtered out early.
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

# ---------------------------------------------------------------------------
# Ensure package imports work when running as a script or as a module
# ---------------------------------------------------------------------------
_project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _project_root not in sys.path:
    sys.path.insert(0, _project_root)

from ais_ingestor import config  # noqa: E402
from db import init_db  # noqa: E402

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
log = logging.getLogger("ais_ingestor")

# ---------------------------------------------------------------------------
# Time parsing helpers
# ---------------------------------------------------------------------------
def parse_ais_time(time_str: str) -> int:
    """
    Convert the Go-style timestamp from AISstream metadata into a Unix
    epoch timestamp (integer seconds).

    Example input : '2026-03-04 04:34:59.795811956 +0000 UTC'
    Example output: 1772517299
    """
    try:
        clean = time_str.replace(" UTC", "").replace(" +0000", "")
        dot_pos = clean.find(".")
        if dot_pos != -1:
            frac = clean[dot_pos + 1:]
            clean = clean[: dot_pos + 1] + frac[:6]
        dt = datetime.strptime(clean, "%Y-%m-%d %H:%M:%S.%f")
        return int(dt.replace(tzinfo=timezone.utc).timestamp())
    except Exception:
        # Last-resort: try ISO-8601 parse
        try:
            return int(datetime.fromisoformat(time_str).timestamp())
        except Exception:
            return int(datetime.now(timezone.utc).timestamp())


def build_eta_timestamp(eta: dict, reference_ts: int) -> int | None:
    """
    Combine AIS ETA fields (Month, Day, Hour, Minute) into a Unix epoch
    timestamp (integer seconds).

    AIS doesn't transmit a year, so we infer it from the reference_ts
    (the message reception timestamp).  Returns None if the ETA is empty
    or clearly invalid (month=0, day=0).
    """
    month = eta.get("Month") or 0
    day = eta.get("Day") or 0
    hour = eta.get("Hour") or 0
    minute = eta.get("Minute") or 0

    if month == 0 or day == 0:
        return None

    # Clamp to valid ranges (AIS sometimes sends 24 for hour, 60 for minute)
    hour = min(hour, 23)
    minute = min(minute, 59)

    ref = datetime.fromtimestamp(reference_ts, tz=timezone.utc)

    year = ref.year
    try:
        eta_dt = datetime(year, month, day, hour, minute, tzinfo=timezone.utc)
    except ValueError:
        return None

    # If the resulting ETA is > 6 months in the past, assume it's next year
    if (ref - eta_dt).days > 180:
        try:
            eta_dt = datetime(year + 1, month, day, hour, minute, tzinfo=timezone.utc)
        except ValueError:
            return None

    return int(eta_dt.timestamp())


# ---------------------------------------------------------------------------
# Non-commercial vessel filtering
# ---------------------------------------------------------------------------
def _is_ship_mmsi(mmsi: int | None) -> bool:
    """Return True if the MMSI belongs to a ship (not a coast station, AtoN, etc.)."""
    if mmsi is None:
        return False
    return 200_000_000 <= mmsi <= 799_999_999


_non_commercial_mmsis: set[int] = set()


def _is_excluded_ship_type(ship_type: int | None) -> bool:
    """Return True if the AIS ship type should be excluded."""
    if ship_type is None:
        return False
    return ship_type in config.EXCLUDED_SHIP_TYPES


# ---------------------------------------------------------------------------
# Row extraction
# ---------------------------------------------------------------------------
def _extract_position(msg: dict, meta: dict) -> dict | None:
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
        "speed_over_ground": report.get("Sog"),
        "course_over_ground": report.get("Cog"),
        "true_heading": report.get("TrueHeading"),
        "rate_of_turn": report.get("RateOfTurn"),
    }


def _extract_nav_status(msg: dict, meta: dict) -> dict | None:
    """Extract navigational status from a position report message."""
    report = (
        msg.get("PositionReport")
        or msg.get("StandardClassBPositionReport")
        or msg.get("ExtendedClassBPositionReport")
    )
    if report is None:
        return None
    nav_status = report.get("NavigationalStatus")
    if nav_status is None:
        return None
    return {
        "mmsi": meta.get("MMSI"),
        "navigational_status": nav_status,
        "timestamp_utc": parse_ais_time(meta.get("time_utc", "")),
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

    dim_a = dim.get("A")
    dim_b = dim.get("B")
    dim_c = dim.get("C")
    dim_d = dim.get("D")

    length_m = (dim_a + dim_b) if (dim_a is not None and dim_b is not None) else None
    beam_m = (dim_c + dim_d) if (dim_c is not None and dim_d is not None) else None

    return {
        "mmsi": meta.get("MMSI"),
        "ship_name": name.strip() if name else None,
        "call_sign": (static.get("CallSign") or "").strip() or None,
        "imo_number": static.get("ImoNumber") or None,
        "ship_type": static.get("Type") or static.get("ShipType"),
        "dimension_to_bow": dim_a,
        "dimension_to_stern": dim_b,
        "dimension_to_port": dim_c,
        "dimension_to_starboard": dim_d,
        "length_m": length_m,
        "beam_m": beam_m,
        "max_draught": static.get("MaximumStaticDraught"),
        "ais_version": static.get("AisVersion"),
        "fix_type": static.get("FixType"),
        "seen_utc": parse_ais_time(meta.get("time_utc", "")),
    }


def _extract_destination(msg: dict, meta: dict) -> dict | None:
    """Extract destination & ETA from ShipStaticData."""
    static = msg.get("ShipStaticData")
    if static is None:
        return None
    dest = (static.get("Destination") or "").strip()
    eta_raw = static.get("Eta", {}) or {}
    if not dest and not any(eta_raw.values()):
        return None

    reported_ts = parse_ais_time(meta.get("time_utc", ""))
    eta_ts = build_eta_timestamp(eta_raw, reported_ts)

    return {
        "mmsi": meta.get("MMSI"),
        "destination": dest or None,
        "estimated_arrival": eta_ts,
        "timestamp_utc": reported_ts,
    }


def _extract_draught(msg: dict, meta: dict) -> dict | None:
    """Extract draught from ShipStaticData."""
    static = msg.get("ShipStaticData")
    if static is None:
        return None
    draught = static.get("MaximumStaticDraught")
    if draught is None or draught <= 0:
        return None
    return {
        "mmsi": meta.get("MMSI"),
        "draught": draught,
        "timestamp_utc": parse_ais_time(meta.get("time_utc", "")),
    }


def _extract_ship_name(msg: dict, meta: dict) -> dict | None:
    """Extract ship name from ShipStaticData for name-change tracking."""
    static = msg.get("ShipStaticData") or msg.get("StaticDataReport")
    if static is None:
        return None
    report_a = static.get("ReportA", {}) or {}
    name = (
        static.get("Name")
        or report_a.get("Name")
        or meta.get("ShipName", "")
    )
    name = name.strip() if name else None
    if not name:
        return None
    return {
        "mmsi": meta.get("MMSI"),
        "ship_name": name,
        "timestamp_utc": parse_ais_time(meta.get("time_utc", "")),
    }


# ---------------------------------------------------------------------------
# SQL templates
# ---------------------------------------------------------------------------
_POSITION_INSERT = """
INSERT INTO position_history
    (mmsi, timestamp_utc, latitude, longitude, speed_over_ground,
     course_over_ground, true_heading, rate_of_turn)
VALUES
    (:mmsi, :timestamp_utc, :latitude, :longitude, :speed_over_ground,
     :course_over_ground, :true_heading, :rate_of_turn)
"""

_VESSEL_UPSERT = """
INSERT INTO vessels
    (mmsi, ship_name, call_sign, imo_number, ship_type,
     dimension_to_bow, dimension_to_stern, dimension_to_port, dimension_to_starboard,
     length_m, beam_m, max_draught,
     ais_version, fix_type, first_seen_utc, last_seen_utc)
VALUES
    (:mmsi, :ship_name, :call_sign, :imo_number, :ship_type,
     :dimension_to_bow, :dimension_to_stern, :dimension_to_port, :dimension_to_starboard,
     :length_m, :beam_m, :max_draught,
     :ais_version, :fix_type, :seen_utc, :seen_utc)
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
    updated_at         = CAST(strftime('%s', 'now') AS INTEGER)
"""

_VESSEL_UPSERT_MINIMAL = """
INSERT INTO vessels
    (mmsi, ship_name, first_seen_utc, last_seen_utc)
VALUES
    (:mmsi, :ship_name, :seen_utc, :seen_utc)
ON CONFLICT(mmsi) DO UPDATE SET
    ship_name     = COALESCE(excluded.ship_name,  vessels.ship_name),
    last_seen_utc = MAX(excluded.last_seen_utc, vessels.last_seen_utc),
    updated_at    = CAST(strftime('%s', 'now') AS INTEGER)
"""

_DESTINATION_INSERT = """
INSERT INTO destination_history
    (mmsi, destination, estimated_arrival, timestamp_utc)
VALUES
    (:mmsi, :destination, :estimated_arrival, :timestamp_utc)
"""

_DRAUGHT_INSERT = """
INSERT INTO draught_history (mmsi, draught, timestamp_utc)
VALUES (:mmsi, :draught, :timestamp_utc)
"""

_NAV_STATUS_INSERT = """
INSERT INTO navigational_status_history (mmsi, navigational_status, timestamp_utc)
VALUES (:mmsi, :navigational_status, :timestamp_utc)
"""

_SHIP_NAME_INSERT = """
INSERT INTO ship_name_history (mmsi, ship_name, timestamp_utc)
VALUES (:mmsi, :ship_name, :timestamp_utc)
"""


# ---------------------------------------------------------------------------
# File processing
# ---------------------------------------------------------------------------
def _is_already_processed(filepath: str, backup_dir: str) -> bool:
    """Check if the file has already been processed (exists in backup dir)."""
    filename = os.path.basename(filepath)
    return os.path.exists(os.path.join(backup_dir, filename))


def process_file(conn, filepath: str, backup_dir: str) -> bool:
    """
    Parse a single JSONL file and load its data into the database.

    Returns True if the file was processed (or was already processed).
    """
    filename = os.path.basename(filepath)

    # Already done? (check backup dir instead of DB table)
    if _is_already_processed(filepath, backup_dir):
        log.info("  ⏩  %s already ingested, skipping.", filename)
        return True

    log.info("  📄  Processing %s …", filename)

    positions: list[dict] = []
    vessels_static: list[dict] = []
    vessels_minimal: dict[int, dict] = {}
    destinations: list[dict] = []
    draughts: list[dict] = []
    nav_statuses: list[dict] = []
    ship_names: list[dict] = []
    total_lines = 0
    skipped_non_commercial = 0

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
            mmsi = meta.get("MMSI")

            # --- Skip non-ship MMSIs (coast stations, AtoN, etc.) -----------
            if not _is_ship_mmsi(mmsi):
                continue

            # --- Skip known non-commercial vessels ---------------------------
            if mmsi in _non_commercial_mmsis:
                skipped_non_commercial += 1
                continue

            # --- Static data ------------------------------------------------
            if msg_type in ("ShipStaticData", "StaticDataReport"):
                vs = _extract_vessel_static(msg, meta)
                if vs and vs["mmsi"]:
                    if _is_excluded_ship_type(vs["ship_type"]):
                        _non_commercial_mmsis.add(vs["mmsi"])
                        skipped_non_commercial += 1
                        continue

                    vessels_static.append(vs)

                dest = _extract_destination(msg, meta)
                if dest and dest["mmsi"]:
                    destinations.append(dest)

                dr = _extract_draught(msg, meta)
                if dr and dr["mmsi"]:
                    draughts.append(dr)

                sn = _extract_ship_name(msg, meta)
                if sn and sn["mmsi"]:
                    ship_names.append(sn)

            # --- Position ---------------------------------------------------
            elif msg_type in (
                "PositionReport",
                "StandardClassBPositionReport",
                "ExtendedClassBPositionReport",
                "StandardSearchAndRescueAircraftReport",
            ):
                pos = _extract_position(msg, meta)
                if pos and pos["mmsi"]:
                    positions.append(pos)
                    vessels_minimal[mmsi] = {
                        "mmsi": mmsi,
                        "ship_name": (meta.get("ShipName") or "").strip() or None,
                        "seen_utc": parse_ais_time(meta.get("time_utc", "")),
                    }

                ns = _extract_nav_status(msg, meta)
                if ns and ns["mmsi"]:
                    nav_statuses.append(ns)

            # --- Other message types (BaseStationReport, etc.) --------------
            else:
                if mmsi:
                    vessels_minimal[mmsi] = {
                        "mmsi": mmsi,
                        "ship_name": (meta.get("ShipName") or "").strip() or None,
                        "seen_utc": parse_ais_time(meta.get("time_utc", "")),
                    }

    # ---- Write to DB in one transaction ------------------------------------
    try:
        cur = conn.cursor()

        # 1) Upsert vessels — static first (richer data), then minimal (deduped)
        if vessels_static:
            cur.executemany(_VESSEL_UPSERT, vessels_static)
        if vessels_minimal:
            cur.executemany(_VESSEL_UPSERT_MINIMAL, list(vessels_minimal.values()))

        # 2) Insert history tables
        if destinations:
            cur.executemany(_DESTINATION_INSERT, destinations)
        if draughts:
            cur.executemany(_DRAUGHT_INSERT, draughts)
        if nav_statuses:
            cur.executemany(_NAV_STATUS_INSERT, nav_statuses)
        if ship_names:
            cur.executemany(_SHIP_NAME_INSERT, ship_names)

        # 3) Get the current max id so we know which rows are new
        max_id_before = cur.execute(
            "SELECT COALESCE(MAX(id), 0) FROM position_history"
        ).fetchone()[0]

        # Insert positions in batches
        for i in range(0, len(positions), config.BATCH_SIZE):
            batch = positions[i : i + config.BATCH_SIZE]
            cur.executemany(_POSITION_INSERT, batch)

        # Populate the R*Tree for all newly inserted rows in one go
        cur.execute(
            """INSERT INTO position_history_rtree (id, min_lat, max_lat, min_lon, max_lon)
               SELECT id, latitude, latitude, longitude, longitude
               FROM position_history
               WHERE id > ?""",
            (max_id_before,),
        )

        conn.commit()
    except Exception:
        conn.rollback()
        log.exception("  ❌  Failed to ingest %s", filename)
        return False

    log.info(
        "  ✅  %s → %d positions, %d destinations, %d draughts, %d nav_statuses, "
        "%d ship_names, %d static, %d minimal vessels "
        "(%d lines, %d skipped non-commercial)",
        filename,
        len(positions),
        len(destinations),
        len(draughts),
        len(nav_statuses),
        len(ship_names),
        len(vessels_static),
        len(vessels_minimal),
        total_lines,
        skipped_non_commercial,
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
        ok = process_file(conn, fp, backup_dir)
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
