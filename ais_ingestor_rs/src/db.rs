/// Database schema and connection helpers — mirrors `db/` Python package.

use rusqlite::Connection;
use std::path::Path;

/// Schema SQL — creates all tables, indexes, views (idempotent).
const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS ship_types (
    ship_type       INTEGER PRIMARY KEY,
    label           TEXT    NOT NULL,
    category        TEXT
);

CREATE TABLE IF NOT EXISTS nav_statuses (
    navigational_status INTEGER PRIMARY KEY,
    label               TEXT    NOT NULL
);

CREATE TABLE IF NOT EXISTS vessels (
    mmsi                INTEGER PRIMARY KEY,
    ship_name           TEXT,
    call_sign           TEXT,
    imo_number          INTEGER,
    ship_type           INTEGER,
    dimension_to_bow        INTEGER,
    dimension_to_stern      INTEGER,
    dimension_to_port       INTEGER,
    dimension_to_starboard  INTEGER,
    length_m            REAL,
    beam_m              REAL,
    max_draught         REAL,
    ais_version         INTEGER,
    fix_type            INTEGER,
    first_seen_utc      TEXT NOT NULL,
    last_seen_utc       TEXT NOT NULL,
    updated_at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS ship_name_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi            INTEGER NOT NULL REFERENCES vessels(mmsi),
    ship_name       TEXT    NOT NULL,
    timestamp_utc   TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shipname_mmsi ON ship_name_history(mmsi, timestamp_utc);

CREATE TABLE IF NOT EXISTS destination_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi            INTEGER NOT NULL REFERENCES vessels(mmsi),
    destination     TEXT,
    estimated_arrival TEXT,
    timestamp_utc   TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_dest_mmsi ON destination_history(mmsi, timestamp_utc);
CREATE INDEX IF NOT EXISTS idx_dest_dest ON destination_history(destination);

CREATE TABLE IF NOT EXISTS draught_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi            INTEGER NOT NULL REFERENCES vessels(mmsi),
    draught         REAL    NOT NULL,
    timestamp_utc   TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_draught_mmsi ON draught_history(mmsi, timestamp_utc);

CREATE TABLE IF NOT EXISTS navigational_status_history (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi                INTEGER NOT NULL REFERENCES vessels(mmsi),
    navigational_status INTEGER NOT NULL,
    timestamp_utc       TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_navstatus_mmsi ON navigational_status_history(mmsi, timestamp_utc);

CREATE TABLE IF NOT EXISTS position_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mmsi            INTEGER NOT NULL REFERENCES vessels(mmsi),
    timestamp_utc   TEXT    NOT NULL,
    latitude        REAL    NOT NULL,
    longitude       REAL    NOT NULL,
    speed_over_ground   REAL,
    course_over_ground  REAL,
    true_heading        INTEGER,
    rate_of_turn        REAL
);
CREATE INDEX IF NOT EXISTS idx_pos_mmsi_ts ON position_history(mmsi, timestamp_utc);
CREATE INDEX IF NOT EXISTS idx_pos_ts      ON position_history(timestamp_utc);

CREATE VIRTUAL TABLE IF NOT EXISTS position_history_rtree USING rtree(
    id,
    min_lat, max_lat,
    min_lon, max_lon
);

CREATE VIEW IF NOT EXISTS v_total_vessels AS
SELECT COUNT(*) AS total_vessels FROM vessels;

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

CREATE VIEW IF NOT EXISTS v_total_positions AS
SELECT COUNT(*) AS total_positions FROM position_history;

CREATE VIEW IF NOT EXISTS v_latest_positions AS
SELECT
    ph.mmsi,
    v.ship_name,
    v.ship_type,
    COALESCE(st.label, 'Unknown') AS ship_type_label,
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

CREATE VIEW IF NOT EXISTS v_vessel_track AS
SELECT
    v.mmsi,
    v.imo_number,
    v.ship_name,
    v.ship_type,
    COALESCE(st.label, 'Unknown') AS ship_type_label,
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
"#;

/// Ship-type seed data: (code, label, category).
const SHIP_TYPE_SEED: &[(i64, &str, &str)] = &[
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
];

/// Navigational-status seed data: (code, label).
const NAV_STATUS_SEED: &[(i64, &str)] = &[
    (0, "Under way using engine"),
    (1, "At anchor"),
    (2, "Not under command"),
    (3, "Restricted manoeuvrability"),
    (4, "Constrained by her draught"),
    (5, "Moored"),
    (6, "Aground"),
    (7, "Engaged in fishing"),
    (8, "Under way sailing"),
    (9, "Reserved for HSC"),
    (10, "Reserved for WIG"),
    (11, "Power-driven vessel towing astern"),
    (12, "Power-driven vessel pushing ahead or towing alongside"),
    (13, "Reserved for future use"),
    (14, "AIS-SART (active), MOB-AIS, EPIRB-AIS"),
    (15, "Undefined / default"),
];

/// Open a connection with performance pragmas tuned for Raspberry Pi.
pub fn get_connection(db_path: &str) -> rusqlite::Result<Connection> {
    let parent = Path::new(db_path).parent();
    if let Some(dir) = parent {
        std::fs::create_dir_all(dir).ok();
    }

    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous  = NORMAL;
         PRAGMA journal_size_limit = 67108864;
         PRAGMA cache_size   = -16000;
         PRAGMA temp_store   = MEMORY;
         PRAGMA mmap_size    = 268435456;
         PRAGMA foreign_keys = ON;",
    )?;
    Ok(conn)
}

/// Create schema (idempotent), seed reference data, return connection.
pub fn init_db(db_path: &str) -> rusqlite::Result<Connection> {
    let conn = get_connection(db_path)?;
    conn.execute_batch(SCHEMA_SQL)?;

    // Seed ship types
    {
        let mut stmt =
            conn.prepare("INSERT OR IGNORE INTO ship_types (ship_type, label, category) VALUES (?1, ?2, ?3)")?;
        for &(code, label, category) in SHIP_TYPE_SEED {
            stmt.execute(rusqlite::params![code, label, category])?;
        }
    }

    // Seed nav statuses
    {
        let mut stmt =
            conn.prepare("INSERT OR IGNORE INTO nav_statuses (navigational_status, label) VALUES (?1, ?2)")?;
        for &(code, label) in NAV_STATUS_SEED {
            stmt.execute(rusqlite::params![code, label])?;
        }
    }

    Ok(conn)
}
