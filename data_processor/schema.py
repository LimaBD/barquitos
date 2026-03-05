"""
Database schema definitions.

Tables are designed following normalisation best practices:
  - `vessels`           : One row per MMSI. Ship identity & static info.
  - `position_reports`  : Time-series of position observations (FK → vessels).
  - `voyages`           : Destination / ETA snapshots from ShipStaticData (FK → vessels).
  - `processed_files`   : Bookkeeping — which raw JSONL files have been ingested.

An R*Tree virtual table (`position_reports_rtree`) gives fast spatial look-ups
without needing the SpatiaLite extension (works on stock SQLite / RPi).
"""

SCHEMA_SQL = """
-- =========================================================================
-- Vessels  (one row per MMSI – upserted on every ShipStaticData message)
-- =========================================================================
CREATE TABLE IF NOT EXISTS vessels (
    mmsi            INTEGER PRIMARY KEY,
    ship_name       TEXT,
    call_sign       TEXT,
    imo_number      INTEGER,
    ship_type       INTEGER,          -- AIS ship/cargo type code
    dim_a           INTEGER,          -- Distance from ref point to bow
    dim_b           INTEGER,          -- Distance from ref point to stern
    dim_c           INTEGER,          -- Distance from ref point to port
    dim_d           INTEGER,          -- Distance from ref point to starboard
    max_draught     REAL,
    ais_version     INTEGER,
    fix_type        INTEGER,
    first_seen_utc  TEXT NOT NULL,    -- ISO-8601
    last_seen_utc   TEXT NOT NULL,    -- ISO-8601
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- =========================================================================
-- Position reports  (the large time-series table)
-- =========================================================================
CREATE TABLE IF NOT EXISTS position_reports (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi            INTEGER NOT NULL REFERENCES vessels(mmsi),
    timestamp_utc   TEXT    NOT NULL,          -- ISO-8601 with fractional seconds
    latitude        REAL    NOT NULL,
    longitude       REAL    NOT NULL,
    sog             REAL,                      -- Speed over ground (knots)
    cog             REAL,                      -- Course over ground (degrees)
    true_heading    INTEGER,
    nav_status      INTEGER,                   -- Navigational status code
    rot             REAL,                      -- Rate of turn
    message_id      INTEGER,                   -- AIS message ID (1/2/3/18/19…)
    position_accuracy INTEGER,
    raim            INTEGER,
    source_file     TEXT                        -- originating JSONL filename
);

-- Composite index for typical queries: "all positions for vessel X in time range"
CREATE INDEX IF NOT EXISTS idx_pos_mmsi_ts   ON position_reports(mmsi, timestamp_utc);
-- Index for time-range scans across all vessels
CREATE INDEX IF NOT EXISTS idx_pos_ts        ON position_reports(timestamp_utc);

-- =========================================================================
-- R*Tree spatial index  (built-in SQLite, no extension needed)
-- Maps position_reports.id → bounding box (point = min==max)
-- =========================================================================
CREATE VIRTUAL TABLE IF NOT EXISTS position_reports_rtree USING rtree(
    id,                     -- matches position_reports.id
    min_lat, max_lat,       -- latitude  bounding box
    min_lon, max_lon        -- longitude bounding box
);

-- =========================================================================
-- Voyage information  (destination & ETA from ShipStaticData / msg 5)
-- =========================================================================
CREATE TABLE IF NOT EXISTS voyages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi            INTEGER NOT NULL REFERENCES vessels(mmsi),
    destination     TEXT,
    eta_month       INTEGER,
    eta_day         INTEGER,
    eta_hour        INTEGER,
    eta_minute      INTEGER,
    reported_utc    TEXT NOT NULL,              -- when we received this info
    source_file     TEXT
);

CREATE INDEX IF NOT EXISTS idx_voyage_mmsi   ON voyages(mmsi, reported_utc);

-- =========================================================================
-- Processed files bookkeeping
-- =========================================================================
CREATE TABLE IF NOT EXISTS processed_files (
    filename        TEXT PRIMARY KEY,
    processed_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    row_count       INTEGER,
    positions_added INTEGER,
    voyages_added   INTEGER,
    vessels_upserted INTEGER
);
"""
