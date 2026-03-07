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
    first_seen_utc      TEXT NOT NULL,
    last_seen_utc       TEXT NOT NULL,
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- =========================================================================
-- Ship-name history  (ship names can change over time)
-- =========================================================================
CREATE TABLE IF NOT EXISTS ship_name_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    ship_name           TEXT    NOT NULL,
    timestamp_utc       TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_shipname_mmsi ON ship_name_history(mmsi, timestamp_utc);

-- =========================================================================
-- Destination history  (destination & ETA snapshots from ShipStaticData)
-- =========================================================================
CREATE TABLE IF NOT EXISTS destination_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    destination         TEXT,
    estimated_arrival   TEXT,
    timestamp_utc       TEXT    NOT NULL
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
    timestamp_utc       TEXT    NOT NULL
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
    timestamp_utc       TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_navstatus_mmsi ON navigational_status_history(mmsi, timestamp_utc);

-- =========================================================================
-- Position history  (the large time-series table)
-- =========================================================================
CREATE TABLE IF NOT EXISTS position_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    timestamp_utc       TEXT    NOT NULL,
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
-- R*Tree spatial index  (built-in SQLite, no extension needed)
-- Maps position_history.id → bounding box (point = min==max)
-- =========================================================================
CREATE VIRTUAL TABLE IF NOT EXISTS position_history_rtree USING rtree(
    id,
    min_lat, max_lat,
    min_lon, max_lon
);
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
