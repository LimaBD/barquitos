"""
Database schema definitions.

Tables are designed for global trade intelligence & vessel tracking:
  - `ship_types`                    : Lookup table mapping AIS ship-type codes → labels.
  - `nav_statuses`                  : Lookup table mapping AIS navigational-status codes → labels.
  - `vessels`                       : One row per MMSI. Ship identity, static info & computed dimensions.
  - `ship_name_history`             : Track ship name changes over time.
  - `destination_history`           : Destination & ETA snapshots from ShipStaticData.
  - `draught_history`               : Draught snapshots from ShipStaticData.
  - `navigational_status_history`   : Nav status changes (infrequent, ~3/month).
  - `position_history`              : Time-series of position observations (FK → vessels).

All timestamp columns store Unix epoch seconds (INTEGER).  This avoids the
overhead of ISO-8601 string parsing in SQLite comparisons, sorts and indexes,
and reduces storage on the Raspberry Pi.

Views:
  - `v_total_vessels`       : Total number of distinct vessels.
  - `v_vessels_by_type`     : Vessel count grouped by ship type (with label & category).
  - `v_total_positions`     : Total number of position reports.
  - `v_latest_positions`    : Most recent known position of every vessel.
  - `v_vessel_track`        : All positions for a vessel (filter by mmsi or imo_number).

Parametric queries (constants):
  - `VESSEL_POSITIONS_AT_SQL` : Position of all (or filtered) vessels at a given
                                timestamp (±5 s window).

An R*Tree virtual table (`position_history_rtree`) gives fast spatial look-ups
without needing the SpatiaLite extension (works on stock SQLite / RPi).
"""

SCHEMA_SQL = """
-- =========================================================================
-- Ship-type lookup
-- Populated once via seed data; new codes can be added at any time.
-- =========================================================================
CREATE TABLE IF NOT EXISTS ship_types (
    ship_type       INTEGER PRIMARY KEY,
    label           TEXT    NOT NULL,
    category        TEXT
);

-- =========================================================================
-- Navigational-status lookup
-- Populated once via seed data; codes defined in ITU-R M.1371-5, Table 45.
-- =========================================================================
CREATE TABLE IF NOT EXISTS nav_statuses (
    navigational_status INTEGER PRIMARY KEY,
    label               TEXT    NOT NULL
);

-- =========================================================================
-- Vessels  (one row per MMSI – upserted on every ShipStaticData message)
-- All timestamps are Unix epoch seconds (INTEGER).
-- =========================================================================
CREATE TABLE IF NOT EXISTS vessels (
    mmsi                INTEGER PRIMARY KEY,
    ship_name           TEXT,
    call_sign           TEXT,
    imo_number          INTEGER,
    ship_type           INTEGER,
    dimension_to_bow       INTEGER,
    dimension_to_stern     INTEGER,
    dimension_to_port      INTEGER,
    dimension_to_starboard INTEGER,
    length_m            REAL,
    beam_m              REAL,
    max_draught         REAL,
    ais_version         INTEGER,
    fix_type            INTEGER,
    first_seen_utc      INTEGER NOT NULL,
    last_seen_utc       INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL DEFAULT (CAST(strftime('%s', 'now') AS INTEGER))
);

-- =========================================================================
-- Ship-name history  (ship names can change over time)
-- =========================================================================
CREATE TABLE IF NOT EXISTS ship_name_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    ship_name           TEXT    NOT NULL,
    timestamp_utc       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_shipname_mmsi ON ship_name_history(mmsi, timestamp_utc);

-- =========================================================================
-- Destination history  (destination & ETA snapshots from ShipStaticData)
-- =========================================================================
CREATE TABLE IF NOT EXISTS destination_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    destination         TEXT,
    estimated_arrival   INTEGER,
    timestamp_utc       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_dest_mmsi ON destination_history(mmsi, timestamp_utc);
CREATE INDEX IF NOT EXISTS idx_dest_dest ON destination_history(destination);

-- =========================================================================
-- Draught history  (draught snapshots from ShipStaticData)
-- =========================================================================
CREATE TABLE IF NOT EXISTS draught_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    draught             REAL    NOT NULL,
    timestamp_utc       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_draught_mmsi ON draught_history(mmsi, timestamp_utc);

-- =========================================================================
-- Navigational-status history  (changes infrequently, ~3 times/month)
-- Duplicates are acceptable; can be deduplicated later.
-- =========================================================================
CREATE TABLE IF NOT EXISTS navigational_status_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    navigational_status INTEGER NOT NULL,
    timestamp_utc       INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_navstatus_mmsi ON navigational_status_history(mmsi, timestamp_utc);

-- =========================================================================
-- Position history  (the large time-series table)
-- =========================================================================
CREATE TABLE IF NOT EXISTS position_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    timestamp_utc       INTEGER NOT NULL,
    latitude            REAL    NOT NULL,
    longitude           REAL    NOT NULL,
    speed_over_ground   REAL,
    course_over_ground  REAL,
    true_heading        INTEGER,
    rate_of_turn        REAL
);

-- Composite index for typical queries: "all positions for vessel X in time range"
CREATE INDEX IF NOT EXISTS idx_pos_mmsi_ts   ON position_history(mmsi, timestamp_utc);
-- Index for time-range scans across all vessels
CREATE INDEX IF NOT EXISTS idx_pos_ts        ON position_history(timestamp_utc);

-- =========================================================================
-- AIS message counts per minute  (populated at ingest time)
-- Tracks how many AIS messages were received in each 60-second bucket.
-- Even zero-count minutes are recorded to show coverage gaps.
-- bucket_start is the Unix epoch second at the start of the minute
-- (i.e. timestamp floored to 60s: (ts / 60) * 60).
-- =========================================================================
CREATE TABLE IF NOT EXISTS ais_message_counts (
    bucket_start    INTEGER PRIMARY KEY,
    message_count   INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_ais_msg_bucket ON ais_message_counts(bucket_start);

-- =========================================================================
-- Vessel details  (enrichment data from external APIs)
-- Populated by vessel_enrichment/enrich.py — not by the AIS ingestor.
-- =========================================================================
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

-- =========================================================================
-- R*Tree spatial index  (built-in SQLite, no extension needed)
-- Maps position_history.id → bounding box (point = min==max)
-- =========================================================================
CREATE VIRTUAL TABLE IF NOT EXISTS position_history_rtree USING rtree(
    id,
    min_lat, max_lat,
    min_lon, max_lon
);

-- =========================================================================
-- VIEW: Total number of distinct vessels
-- =========================================================================
CREATE VIEW IF NOT EXISTS v_total_vessels AS
SELECT COUNT(*) AS total_vessels
FROM vessels;

-- =========================================================================
-- VIEW: Total vessels by ship type (with label and category)
-- =========================================================================
CREATE VIEW IF NOT EXISTS v_vessels_by_type AS
SELECT
    COALESCE(st.label, 'Unknown') AS ship_type_label,
    COALESCE(st.category, 'Unknown') AS category,
    v.ship_type AS ship_type_code,
    COUNT(*) AS vessel_count
FROM vessels v
LEFT JOIN ship_types st ON v.ship_type = st.ship_type
GROUP BY v.ship_type
ORDER BY vessel_count DESC;

-- =========================================================================
-- VIEW: Total number of position reports
-- =========================================================================
CREATE VIEW IF NOT EXISTS v_total_positions AS
SELECT COUNT(*) AS total_positions
FROM position_history;

-- =========================================================================
-- VIEW: Latest known position of every vessel (closest record to "now")
--        Useful as a base; for a specific timestamp use the parametric
--        query helper  fn_vessel_positions_at(timestamp, ship_type).
-- =========================================================================
CREATE VIEW IF NOT EXISTS v_latest_positions AS
SELECT
    ph.mmsi,
    v.ship_name,
    v.ship_type,
    COALESCE(st.label, 'Unknown') AS ship_type_label,
    COALESCE(st.category, 'Unknown') AS category,
    ph.timestamp_utc,
    ph.latitude,
    ph.longitude,
    ph.speed_over_ground,
    ph.course_over_ground,
    ph.true_heading
FROM position_history ph
INNER JOIN (
    SELECT mmsi, MAX(timestamp_utc) AS max_ts
    FROM position_history
    GROUP BY mmsi
) latest ON ph.mmsi = latest.mmsi AND ph.timestamp_utc = latest.max_ts
LEFT JOIN vessels v ON ph.mmsi = v.mmsi
LEFT JOIN ship_types st ON v.ship_type = st.ship_type;

-- =========================================================================
-- VIEW: All positions for a specific vessel (filter by mmsi or imo_number)
--        Usage:
--          SELECT * FROM v_vessel_track WHERE mmsi = 123456789;
--          SELECT * FROM v_vessel_track WHERE imo_number = 9876543;
-- =========================================================================
CREATE VIEW IF NOT EXISTS v_vessel_track AS
SELECT
    v.mmsi,
    v.imo_number,
    v.ship_name,
    v.ship_type,
    COALESCE(st.label, 'Unknown') AS ship_type_label,
    COALESCE(st.category, 'Unknown') AS category,
    ph.timestamp_utc,
    ph.latitude,
    ph.longitude,
    ph.speed_over_ground,
    ph.course_over_ground,
    ph.true_heading,
    ph.rate_of_turn
FROM position_history ph
JOIN vessels v ON ph.mmsi = v.mmsi
LEFT JOIN ship_types st ON v.ship_type = st.ship_type
ORDER BY ph.mmsi, ph.timestamp_utc;
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

# =========================================================================
# Navigational-status seed data  (AIS nav-status codes → labels)
# Reference: ITU-R M.1371-5, Table 45
# =========================================================================
NAV_STATUS_SEED = [
    # (nav_status, label)
    (0,  "Under way using engine"),
    (1,  "At anchor"),
    (2,  "Not under command"),
    (3,  "Restricted manoeuvrability"),
    (4,  "Constrained by her draught"),
    (5,  "Moored"),
    (6,  "Aground"),
    (7,  "Engaged in fishing"),
    (8,  "Under way sailing"),
    (9,  "Reserved for HSC"),
    (10, "Reserved for WIG"),
    (11, "Power-driven vessel towing astern"),
    (12, "Power-driven vessel pushing ahead or towing alongside"),
    (13, "Reserved for future use"),
    (14, "AIS-SART (active), MOB-AIS, EPIRB-AIS"),
    (15, "Undefined / default"),
]

SEED_NAV_STATUSES_SQL = """
INSERT OR IGNORE INTO nav_statuses (navigational_status, label) VALUES (?, ?);
"""

SEED_SHIP_TYPES_SQL = """
INSERT OR IGNORE INTO ship_types (ship_type, label, category) VALUES (?, ?, ?);
"""

# =========================================================================
# Parametric query: vessel positions at a specific point in time
# =========================================================================
# For each vessel, finds the position record closest to the given timestamp
# within a ±5-second window.  Returns nothing for vessels that have no
# record within that window.
#
# Parameters:
#   :target_ts  — Unix epoch seconds (INTEGER), e.g. 1772517300
#   :ship_type  — (optional) ship-type code to filter by; pass NULL to
#                  include all vessel types.
#
# Usage:
#   conn.execute(VESSEL_POSITIONS_AT_SQL, {"target_ts": 1772517300, "ship_type": None})
#   conn.execute(VESSEL_POSITIONS_AT_SQL, {"target_ts": 1772517300, "ship_type": 80})
# =========================================================================
VESSEL_POSITIONS_AT_SQL = """
WITH candidates AS (
    SELECT
        ph.mmsi,
        ph.timestamp_utc,
        ph.latitude,
        ph.longitude,
        ph.speed_over_ground,
        ph.course_over_ground,
        ph.true_heading,
        ABS(ph.timestamp_utc - :target_ts) AS diff_seconds,
        ROW_NUMBER() OVER (
            PARTITION BY ph.mmsi
            ORDER BY ABS(ph.timestamp_utc - :target_ts)
        ) AS rn
    FROM position_history ph
    WHERE ph.timestamp_utc BETWEEN (:target_ts - 5) AND (:target_ts + 5)
)
SELECT
    c.mmsi,
    v.ship_name,
    v.ship_type,
    COALESCE(st.label, 'Unknown') AS ship_type_label,
    c.timestamp_utc,
    c.latitude,
    c.longitude,
    c.speed_over_ground,
    c.course_over_ground,
    c.true_heading,
    c.diff_seconds
FROM candidates c
JOIN vessels v ON c.mmsi = v.mmsi
LEFT JOIN ship_types st ON v.ship_type = st.ship_type
WHERE c.rn = 1
  AND (:ship_type IS NULL OR v.ship_type = :ship_type)
ORDER BY c.mmsi;
"""

# =========================================================================
# Upsert AIS message count for a 60-second bucket
# =========================================================================
UPSERT_AIS_MESSAGE_COUNT_SQL = """
INSERT INTO ais_message_counts (bucket_start, message_count)
VALUES (:bucket_start, :message_count)
ON CONFLICT(bucket_start) DO UPDATE SET
    message_count = ais_message_counts.message_count + excluded.message_count;
"""

# =========================================================================
# Parametric query: latest positions filtered by ship types + bounding box
# =========================================================================
# Used by the optimized global map endpoint. Filters by:
#   :ship_types — comma-separated list of ship_type codes (or NULL for all)
#   :min_lat, :max_lat, :min_lon, :max_lon — viewport bounding box
#   :limit — max rows to return
# =========================================================================
LATEST_POSITIONS_BBOX_SQL = """
SELECT
    ph.mmsi,
    v.ship_name,
    v.ship_type,
    COALESCE(st.label, 'Unknown') AS ship_type_label,
    COALESCE(st.category, 'Unknown') AS category,
    ph.timestamp_utc,
    ph.latitude,
    ph.longitude,
    ph.speed_over_ground,
    ph.course_over_ground,
    ph.true_heading
FROM position_history ph
INNER JOIN (
    SELECT mmsi, MAX(timestamp_utc) AS max_ts
    FROM position_history
    GROUP BY mmsi
) latest ON ph.mmsi = latest.mmsi AND ph.timestamp_utc = latest.max_ts
JOIN vessels v ON ph.mmsi = v.mmsi
LEFT JOIN ship_types st ON v.ship_type = st.ship_type
WHERE ph.latitude BETWEEN :min_lat AND :max_lat
  AND ph.longitude BETWEEN :min_lon AND :max_lon
ORDER BY RANDOM()
LIMIT :limit;
"""

# =========================================================================
# Query: AIS time coverage (message counts per minute)
# =========================================================================
AIS_COVERAGE_SQL = """
SELECT bucket_start, message_count
FROM ais_message_counts
ORDER BY bucket_start;
"""
