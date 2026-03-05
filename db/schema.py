"""
Database schema definitions.

Tables are designed for global trade intelligence & vessel tracking:
  - `ship_types`        : Lookup table mapping AIS ship-type codes → labels.
  - `vessels`           : One row per MMSI. Ship identity, static info & computed dimensions.
  - `ports`             : Known ports / anchorages (enriched over time from AIS destinations).
  - `voyages`           : Destination / ETA snapshots from ShipStaticData (FK → vessels, ports).
  - `position_reports`  : Time-series of position observations (FK → vessels, optionally → voyages).

An R*Tree virtual table (`position_reports_rtree`) gives fast spatial look-ups
without needing the SpatiaLite extension (works on stock SQLite / RPi).
"""

SCHEMA_SQL = """
-- =========================================================================
-- Ship-type lookup  (AIS type code → human-readable label & category)
-- Populated once via seed data; new codes can be added at any time.
-- =========================================================================
CREATE TABLE IF NOT EXISTS ship_types (
    ship_type       INTEGER PRIMARY KEY,     -- AIS ship/cargo type code
    label           TEXT    NOT NULL,         -- e.g. "Bulk Carrier"
    category        TEXT                      -- coarse grouping: Cargo, Tanker, Passenger, …
);

-- =========================================================================
-- Vessels  (one row per MMSI – upserted on every ShipStaticData message)
-- =========================================================================
CREATE TABLE IF NOT EXISTS vessels (
    mmsi            INTEGER PRIMARY KEY,
    ship_name       TEXT,
    call_sign       TEXT,
    imo_number      INTEGER,
    ship_type       INTEGER,           -- AIS type code (see ship_types table for labels)
    dim_a           INTEGER,          -- bow
    dim_b           INTEGER,          -- stern
    dim_c           INTEGER,          -- port
    dim_d           INTEGER,          -- starboard
    length_m        REAL,             -- dim_a + dim_b  (computed on upsert)
    beam_m          REAL,             -- dim_c + dim_d  (computed on upsert)
    max_draught     REAL,
    ais_version     INTEGER,
    fix_type        INTEGER,
    first_seen_utc  TEXT NOT NULL,    -- ISO-8601
    last_seen_utc   TEXT NOT NULL,    -- ISO-8601
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- =========================================================================
-- Ports  (known destinations / anchorages, enriched over time)
-- =========================================================================
CREATE TABLE IF NOT EXISTS ports (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    name            TEXT    NOT NULL UNIQUE,  -- normalised destination string
    country         TEXT,                     -- ISO-3166-1 alpha-2
    latitude        REAL,
    longitude       REAL,
    unlocode        TEXT                      -- UN/LOCODE when known
);

CREATE INDEX IF NOT EXISTS idx_port_name ON ports(name);

-- =========================================================================
-- Voyages  (destination & ETA snapshots from ShipStaticData / msg 5)
-- A voyage represents a leg: current transit toward a destination.
-- =========================================================================
CREATE TABLE IF NOT EXISTS voyages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi            INTEGER NOT NULL REFERENCES vessels(mmsi),
    destination     TEXT,
    destination_port_id INTEGER REFERENCES ports(id),
    eta             TEXT,              -- ISO-8601 datetime (combined from AIS ETA fields)
    draught         REAL,             -- reported draught at departure (trade signal)
    cargo_status    TEXT,             -- 'laden' / 'ballast' / NULL (inferred from draught)
    reported_utc    TEXT NOT NULL      -- when we received this info
);

CREATE INDEX IF NOT EXISTS idx_voyage_mmsi   ON voyages(mmsi, reported_utc);
CREATE INDEX IF NOT EXISTS idx_voyage_dest   ON voyages(destination);

-- =========================================================================
-- Position reports  (the large time-series table)
-- =========================================================================
CREATE TABLE IF NOT EXISTS position_reports (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi            INTEGER NOT NULL REFERENCES vessels(mmsi),
    voyage_id       INTEGER REFERENCES voyages(id),  -- active voyage at time of report
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
    raim            INTEGER
);

-- Composite index for typical queries: "all positions for vessel X in time range"
CREATE INDEX IF NOT EXISTS idx_pos_mmsi_ts   ON position_reports(mmsi, timestamp_utc);
-- Index for time-range scans across all vessels
CREATE INDEX IF NOT EXISTS idx_pos_ts        ON position_reports(timestamp_utc);
-- Index for "all positions belonging to a voyage"
CREATE INDEX IF NOT EXISTS idx_pos_voyage    ON position_reports(voyage_id);

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
-- View: current voyage & last known position per vessel  (convenience)
-- =========================================================================
CREATE VIEW IF NOT EXISTS v_vessel_status AS
SELECT
    v.mmsi,
    v.ship_name,
    v.imo_number,
    v.length_m,
    v.beam_m,
    st.label      AS ship_type_label,
    st.category   AS ship_type_category,
    lp.latitude   AS last_lat,
    lp.longitude  AS last_lon,
    lp.sog        AS last_sog,
    lp.cog        AS last_cog,
    lp.nav_status AS last_nav_status,
    lp.timestamp_utc AS last_position_utc,
    cv.destination,
    cv.eta,
    cv.cargo_status,
    cv.id         AS current_voyage_id
FROM vessels v
LEFT JOIN ship_types st ON st.ship_type = v.ship_type
LEFT JOIN (
    SELECT mmsi, id, latitude, longitude, sog, cog, nav_status, timestamp_utc,
           ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
    FROM position_reports
) lp ON lp.mmsi = v.mmsi AND lp.rn = 1
LEFT JOIN (
    SELECT mmsi, id, destination, eta, cargo_status,
           ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY reported_utc DESC) AS rn
    FROM voyages
) cv ON cv.mmsi = v.mmsi AND cv.rn = 1;
"""

# =========================================================================
# Ship-type seed data  (AIS type codes → labels & categories)
# Reference: ITU-R M.1371-5, Table 53
# =========================================================================
SHIP_TYPE_SEED = [
    # (ship_type, label, category)
    (20, "Wing in ground", "Other"),
    (21, "WIG – Carrying DG/HS/MP", "Other"),
    (22, "WIG – Category B", "Other"),
    (23, "WIG – Category C", "Other"),
    (29, "WIG – No additional info", "Other"),
    (30, "Fishing", "Fishing"),
    (31, "Towing", "Service"),
    (32, "Towing (large)", "Service"),
    (33, "Dredging / Underwater ops", "Service"),
    (34, "Diving operations", "Service"),
    (35, "Military operations", "Military"),
    (36, "Sailing", "Recreational"),
    (37, "Pleasure craft", "Recreational"),
    (40, "High speed craft", "Passenger"),
    (41, "HSC – Carrying DG/HS/MP", "Passenger"),
    (42, "HSC – Category B", "Passenger"),
    (43, "HSC – Category C", "Passenger"),
    (49, "HSC – No additional info", "Passenger"),
    (50, "Pilot vessel", "Service"),
    (51, "Search and rescue", "Service"),
    (52, "Tug", "Service"),
    (53, "Port tender", "Service"),
    (54, "Anti-pollution equipment", "Service"),
    (55, "Law enforcement", "Service"),
    (56, "Spare – Local vessel 1", "Other"),
    (57, "Spare – Local vessel 2", "Other"),
    (58, "Medical transport", "Service"),
    (59, "Non-combatant ship", "Military"),
    (60, "Passenger", "Passenger"),
    (61, "Passenger – Carrying DG/HS/MP", "Passenger"),
    (62, "Passenger – Category B", "Passenger"),
    (63, "Passenger – Category C", "Passenger"),
    (69, "Passenger – No additional info", "Passenger"),
    (70, "Cargo", "Cargo"),
    (71, "Cargo – Carrying DG/HS/MP", "Cargo"),
    (72, "Cargo – Category B", "Cargo"),
    (73, "Cargo – Category C", "Cargo"),
    (74, "Cargo – Category D", "Cargo"),
    (75, "Cargo – Reserved", "Cargo"),
    (76, "Cargo – Reserved", "Cargo"),
    (77, "Cargo – Reserved", "Cargo"),
    (78, "Cargo – Reserved", "Cargo"),
    (79, "Cargo – No additional info", "Cargo"),
    (80, "Tanker", "Tanker"),
    (81, "Tanker – Carrying DG/HS/MP", "Tanker"),
    (82, "Tanker – Category B", "Tanker"),
    (83, "Tanker – Category C", "Tanker"),
    (84, "Tanker – Category D", "Tanker"),
    (85, "Tanker – Reserved", "Tanker"),
    (86, "Tanker – Reserved", "Tanker"),
    (87, "Tanker – Reserved", "Tanker"),
    (88, "Tanker – Reserved", "Tanker"),
    (89, "Tanker – No additional info", "Tanker"),
    (90, "Other", "Other"),
    (91, "Other – Carrying DG/HS/MP", "Other"),
    (92, "Other – Category B", "Other"),
    (93, "Other – Category C", "Other"),
    (94, "Other – Category D", "Other"),
    (99, "Other – No additional info", "Other"),
]

SEED_SHIP_TYPES_SQL = """
INSERT OR IGNORE INTO ship_types (ship_type, label, category) VALUES (?, ?, ?);
"""
