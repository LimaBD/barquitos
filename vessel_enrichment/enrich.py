"""
Vessel Enrichment Collector

Fetches additional vessel metadata from public APIs to enrich the database
with specifications not available through AIS messages alone:
  - Gross tonnage / Net tonnage / Deadweight tonnage
  - Year built
  - Flag state
  - Class society
  - Vessel category (detailed)
  - Owner / Operator

Data sources:
  1. ITU MARS (Maritime Mobile Access and Retrieval System) — free, MMSI lookup
  2. VesselFinder / MarineTraffic web scraping (fallback, rate-limited)

Usage:
    python vessel_enrichment/enrich.py [--db /path/to/barquitos.db] [--batch-size 50] [--delay 2]

The script processes vessels that haven't been enriched yet, in batches.
Safe to re-run (idempotent, skips already-enriched vessels).
"""

import argparse
import json
import logging
import os
import sqlite3
import sys
import time
from urllib.request import urlopen, Request
from urllib.error import HTTPError, URLError

# Allow importing the shared `db` package from the repo root
_project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _project_root)

from db.connection import get_connection

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
logger = logging.getLogger("vessel_enrichment")

DEFAULT_DB = os.path.join("/mnt", "wdblack", "barquitos.db")

# ── Schema for the enrichment table ────────────────────────────────────
ENRICHMENT_SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS vessel_details (
    mmsi                INTEGER PRIMARY KEY REFERENCES vessels(mmsi),
    gross_tonnage       REAL,
    net_tonnage         REAL,
    deadweight_tonnage  REAL,
    year_built          INTEGER,
    flag_country        TEXT,
    flag_code           TEXT,
    class_society       TEXT,
    registered_owner    TEXT,
    operator            TEXT,
    vessel_category     TEXT,
    hull_type           TEXT,
    engine_type         TEXT,
    max_speed_knots     REAL,
    source              TEXT,
    enriched_at         INTEGER NOT NULL DEFAULT (CAST(strftime('%s', 'now') AS INTEGER))
);
"""

# ── ITU MARS API (free, no key required) ──────────────────────────────
ITU_MARS_URL = "https://www.itu.int/mmsapp/ShipStation/list?is498=false&isMap=false&stationType=SS&query={mmsi}"

def fetch_itu_mars(mmsi: int) -> dict | None:
    """
    Query ITU MARS for vessel details by MMSI.
    Returns a dict with available fields, or None on failure.
    """
    url = ITU_MARS_URL.format(mmsi=mmsi)
    req = Request(url, headers={"User-Agent": "Barquitos/1.0"})
    try:
        with urlopen(req, timeout=15) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            if not data or not isinstance(data, list) or len(data) == 0:
                return None
            ship = data[0]
            result = {}
            if ship.get("callSign"):
                result["call_sign"] = ship["callSign"]
            if ship.get("owner"):
                result["registered_owner"] = ship["owner"]
            if ship.get("grossTonnage"):
                try:
                    result["gross_tonnage"] = float(ship["grossTonnage"])
                except (ValueError, TypeError):
                    pass
            if ship.get("shipName"):
                result["ship_name_itu"] = ship["shipName"]
            return result if result else None
    except (HTTPError, URLError, json.JSONDecodeError, OSError) as e:
        logger.debug("ITU MARS lookup failed for %d: %s", mmsi, e)
        return None


# ── VesselFinder public page scraping (basic, no API key) ─────────────
VESSELFINDER_URL = "https://www.vesselfinder.com/api/pub/click/{mmsi}"

def fetch_vesselfinder(mmsi: int) -> dict | None:
    """
    Attempt to get vessel info from VesselFinder's public click API.
    This is rate-limited; use sparingly with delays.
    """
    url = VESSELFINDER_URL.format(mmsi=mmsi)
    req = Request(url, headers={
        "User-Agent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
        "Accept": "application/json",
    })
    try:
        with urlopen(req, timeout=15) as resp:
            data = json.loads(resp.read().decode("utf-8"))
            result = {}
            for key, field in [
                ("gt", "gross_tonnage"),
                ("dw", "deadweight_tonnage"),
                ("yo", "year_built"),
                ("fc", "flag_country"),
                ("type", "vessel_category"),
            ]:
                val = data.get(key)
                if val is not None and val != "" and val != 0:
                    if field in ("gross_tonnage", "deadweight_tonnage"):
                        try:
                            result[field] = float(val)
                        except (ValueError, TypeError):
                            pass
                    elif field == "year_built":
                        try:
                            result[field] = int(val)
                        except (ValueError, TypeError):
                            pass
                    else:
                        result[field] = str(val)
            return result if result else None
    except (HTTPError, URLError, json.JSONDecodeError, OSError) as e:
        logger.debug("VesselFinder lookup failed for %d: %s", mmsi, e)
        return None


def enrich_vessel(mmsi: int) -> dict:
    """
    Try multiple sources to gather vessel details.
    Returns a merged dict of all found fields.
    """
    merged = {}

    # Source 1: ITU MARS
    itu_data = fetch_itu_mars(mmsi)
    if itu_data:
        merged.update(itu_data)
        merged["source"] = "itu_mars"

    # Source 2: VesselFinder (only if ITU didn't give us tonnage)
    if "gross_tonnage" not in merged:
        vf_data = fetch_vesselfinder(mmsi)
        if vf_data:
            for k, v in vf_data.items():
                if k not in merged:
                    merged[k] = v
            merged["source"] = merged.get("source", "") + "+vesselfinder" if merged.get("source") else "vesselfinder"

    return merged


def get_unenriched_vessels(conn: sqlite3.Connection, batch_size: int) -> list[int]:
    """Get MMSIs that haven't been enriched yet."""
    rows = conn.execute(
        """
        SELECT v.mmsi
        FROM vessels v
        LEFT JOIN vessel_details vd ON v.mmsi = vd.mmsi
        WHERE vd.mmsi IS NULL
        ORDER BY v.last_seen_utc DESC
        LIMIT ?
        """,
        (batch_size,),
    ).fetchall()
    return [r[0] for r in rows]


def save_enrichment(conn: sqlite3.Connection, mmsi: int, data: dict):
    """Upsert vessel details."""
    conn.execute(
        """
        INSERT INTO vessel_details (
            mmsi, gross_tonnage, net_tonnage, deadweight_tonnage,
            year_built, flag_country, flag_code, class_society,
            registered_owner, operator, vessel_category,
            hull_type, engine_type, max_speed_knots, source
        ) VALUES (
            :mmsi, :gross_tonnage, :net_tonnage, :deadweight_tonnage,
            :year_built, :flag_country, :flag_code, :class_society,
            :registered_owner, :operator, :vessel_category,
            :hull_type, :engine_type, :max_speed_knots, :source
        )
        ON CONFLICT(mmsi) DO UPDATE SET
            gross_tonnage      = COALESCE(excluded.gross_tonnage,      vessel_details.gross_tonnage),
            net_tonnage        = COALESCE(excluded.net_tonnage,        vessel_details.net_tonnage),
            deadweight_tonnage = COALESCE(excluded.deadweight_tonnage, vessel_details.deadweight_tonnage),
            year_built         = COALESCE(excluded.year_built,         vessel_details.year_built),
            flag_country       = COALESCE(excluded.flag_country,       vessel_details.flag_country),
            flag_code          = COALESCE(excluded.flag_code,          vessel_details.flag_code),
            class_society      = COALESCE(excluded.class_society,      vessel_details.class_society),
            registered_owner   = COALESCE(excluded.registered_owner,   vessel_details.registered_owner),
            operator           = COALESCE(excluded.operator,           vessel_details.operator),
            vessel_category    = COALESCE(excluded.vessel_category,    vessel_details.vessel_category),
            hull_type          = COALESCE(excluded.hull_type,          vessel_details.hull_type),
            engine_type        = COALESCE(excluded.engine_type,        vessel_details.engine_type),
            max_speed_knots    = COALESCE(excluded.max_speed_knots,    vessel_details.max_speed_knots),
            source             = COALESCE(excluded.source,             vessel_details.source),
            enriched_at        = CAST(strftime('%s', 'now') AS INTEGER)
        """,
        {
            "mmsi": mmsi,
            "gross_tonnage": data.get("gross_tonnage"),
            "net_tonnage": data.get("net_tonnage"),
            "deadweight_tonnage": data.get("deadweight_tonnage"),
            "year_built": data.get("year_built"),
            "flag_country": data.get("flag_country"),
            "flag_code": data.get("flag_code"),
            "class_society": data.get("class_society"),
            "registered_owner": data.get("registered_owner"),
            "operator": data.get("operator"),
            "vessel_category": data.get("vessel_category"),
            "hull_type": data.get("hull_type"),
            "engine_type": data.get("engine_type"),
            "max_speed_knots": data.get("max_speed_knots"),
            "source": data.get("source"),
        },
    )
    conn.commit()


def main():
    parser = argparse.ArgumentParser(description="Enrich vessel data from external sources")
    parser.add_argument("--db", default=DEFAULT_DB, help="Path to SQLite database")
    parser.add_argument("--batch-size", type=int, default=50, help="Vessels per batch")
    parser.add_argument("--delay", type=float, default=2.0, help="Seconds between API calls")
    parser.add_argument("--watch", action="store_true", help="Run continuously")
    parser.add_argument("--interval", type=int, default=300, help="Seconds between batches in watch mode")
    args = parser.parse_args()

    if not os.path.isfile(args.db):
        logger.error("Database not found: %s", args.db)
        sys.exit(1)

    conn = get_connection(args.db)
    conn.row_factory = sqlite3.Row

    # Ensure enrichment table exists
    conn.executescript(ENRICHMENT_SCHEMA_SQL)
    conn.commit()

    while True:
        mmsis = get_unenriched_vessels(conn, args.batch_size)
        if not mmsis:
            logger.info("No unenriched vessels found.")
            if not args.watch:
                break
            logger.info("Sleeping %ds before next check…", args.interval)
            time.sleep(args.interval)
            continue

        logger.info("Enriching %d vessels…", len(mmsis))
        enriched_count = 0
        empty_count = 0

        for i, mmsi in enumerate(mmsis, 1):
            data = enrich_vessel(mmsi)
            if data:
                save_enrichment(conn, mmsi, data)
                enriched_count += 1
                logger.info(
                    "  [%d/%d] MMSI %d → %s (source: %s)",
                    i, len(mmsis), mmsi,
                    ", ".join(f"{k}={v}" for k, v in data.items() if k != "source"),
                    data.get("source", "?"),
                )
            else:
                # Save empty record so we don't retry immediately
                save_enrichment(conn, mmsi, {"source": "no_data"})
                empty_count += 1
                logger.debug("  [%d/%d] MMSI %d → no data found", i, len(mmsis), mmsi)

            if i < len(mmsis):
                time.sleep(args.delay)

        logger.info(
            "Batch done: %d enriched, %d no data, %d total",
            enriched_count, empty_count, len(mmsis),
        )

        if not args.watch:
            break
        logger.info("Sleeping %ds before next batch…", args.interval)
        time.sleep(args.interval)

    conn.close()
    logger.info("Done.")


if __name__ == "__main__":
    main()
