# AIS Data Lake — Data Dictionary

> This document describes every table and column in the Bronze layer of the
> Barquitos AIS Data Lake.  All data is stored as Apache Parquet files
> under `/mnt/wdblack/trading_lake/bronze/source=ais/`.

---

## Design Principles

| Principle | Detail |
|---|---|
| **Vessel identity** | **MMSI** (Maritime Mobile Service Identity) is the primary key in every table. It is always present in every AIS message. IMO numbers are stored in `dim_vessel` and can be JOINed when needed. |
| **Timestamp precision** | All timestamps are **UTC seconds** (`Timestamp(Second, "UTC")`). AIS data does not require sub-second precision, and second-level resolution halves the storage cost vs. microseconds. |
| **Coordinate precision** | Latitude/longitude in `fact_position` are **rounded to 5 decimal places** (~1.1 m at the equator). This is sufficient for vessel tracking while reducing floating-point bloat. |
| **Edge filtering** | All tables except `fact_position` and `metric_system_health` are edge-filtered — a row is written **only when a value changes** for a given MMSI. The first observation per vessel is always recorded. |
| **Normalization** | Ship name lives in `fact_vessel_name_history`. Previous values (e.g. `previous_draught`) are not stored — derive them with `LAG()` in queries. Vessel registry attributes live in `dim_vessel`. |
| **Computed columns** | `length_overall` and `beam` in `dim_vessel` are pre-computed from AIS dimension fields (A+B and C+D). Raw dimensions are not stored. |

---

## Table Overview

```
trading_lake/bronze/source=ais/
├── fact_position/                  ← High-frequency GPS pings
│   └── year=YYYY/month=MM/day=DD/HHMM_to_HHMM.parquet
├── dim_vessel/                     ← Edge-filtered vessel registry (flat)
│   └── vessel_static.parquet
├── fact_vessel_name_history/       ← Edge-filtered name changes
│   └── year=YYYY/month=MM/name_history.parquet
├── fact_draught/                   ← Edge-filtered draught changes
│   └── year=YYYY/month=MM/draught_changes.parquet
├── fact_navigational_status/       ← Edge-filtered nav status changes
│   └── year=YYYY/month=MM/status_changes.parquet
├── fact_destination/               ← Edge-filtered destination changes
│   └── year=YYYY/month=MM/destination_changes.parquet
└── metric_system_health/           ← Ingestion telemetry
    └── year=YYYY/month=MM/health_metrics.parquet
```

---

## 1. `fact_position` — GPS Position Reports

High-frequency position pings (typically every 15 seconds for moving vessels).
Written in 15-minute time windows, sorted by MMSI within each file for optimal
Parquet min/max statistics and DuckDB predicate pushdown.

| Column | Arrow Type | Nullable | Description |
|---|---|---|---|
| `mmsi` | `UInt32` | ✗ | Maritime Mobile Service Identity (vessel ID) |
| `timestamp_utc` | `Timestamp(Second, "UTC")` | ✗ | When the position was reported |
| `latitude` | `Float64` | ✗ | Latitude in decimal degrees (5 dp, ~1 m precision) |
| `longitude` | `Float64` | ✗ | Longitude in decimal degrees (5 dp, ~1 m precision) |
| `speed_over_ground` | `Float32` | ✓ | Speed in knots (0.1 kn resolution) |
| `course_over_ground` | `Float32` | ✓ | Course in degrees (0.1° resolution) |
| `true_heading` | `UInt16` | ✓ | Heading in degrees (0–359, or null if unavailable) |
| `rate_of_turn` | `Int16` | ✓ | Rate of turn (AIS-encoded, °/min) |

**Partitioning:** `year=YYYY/month=MM/day=DD/HHMM_to_HHMM.parquet`
**Parquet features:** ZSTD compression, Bloom filter on `mmsi`, sort-by-MMSI.

---

## 2. `dim_vessel` — Vessel Registry (Slowly-Changing Dimension)

Edge-filtered: a new row is written only when any field changes for a given
MMSI.  Contains the slowly-changing "registry" attributes of a vessel —
call sign, IMO number, ship type, and physical dimensions.

**Not partitioned by time** — stored as a single flat file because the volume
is low (roughly one row per vessel, rarely changing).

Draught is not stored here because it changes frequently with loading
operations and has its own dedicated edge-filtered table (`fact_draught`).

| Column | Arrow Type | Nullable | Description |
|---|---|---|---|
| `mmsi` | `UInt32` | ✗ | Vessel MMSI |
| `timestamp_utc` | `Timestamp(Second, "UTC")` | ✗ | When this state was first observed |
| `call_sign` | `Utf8` | ✓ | Radio call sign |
| `imo_number` | `UInt32` | ✓ | IMO number (unique hull identifier, >0) |
| `ship_type` | `UInt8` | ✓ | AIS ship type code (0–99) |
| `length_overall` | `UInt16` | ✓ | **Computed:** `dim_a + dim_b` — total length in metres |
| `beam` | `UInt16` | ✓ | **Computed:** `dim_c + dim_d` — total width in metres |

**Storage:** `dim_vessel/vessel_static.parquet` (flat, append-on-change)

### AIS Ship Type Codes (common)
| Range | Category |
|---|---|
| 30 | Fishing |
| 36 | Sailing |
| 37 | Pleasure craft |
| 40–49 | High speed craft |
| 50–59 | Service (pilot, SAR, tug, etc.) |
| 60–69 | Passenger |
| 70–79 | Cargo |
| 80–89 | Tanker |
| 90–99 | Other |

### Example: Get Latest Static Data for a Vessel
```sql
SELECT *
FROM read_parquet('…/dim_vessel/vessel_static.parquet')
WHERE mmsi = 257421000
ORDER BY timestamp_utc DESC
LIMIT 1;
```

---

## 3. `fact_vessel_name_history` — Vessel Name Changes

Edge-filtered: one row is emitted per vessel when the ship name **changes**.
The first observation for each MMSI is always recorded. Names are compared
trimmed and uppercased for change detection; the normalized form is stored.

This table enables tracking vessel name changes over time — a vessel's
MMSI stays the same but its name may change when sold, renamed, or
re-flagged.

| Column | Arrow Type | Nullable | Description |
|---|---|---|---|
| `mmsi` | `UInt32` | ✗ | Vessel MMSI |
| `timestamp_utc` | `Timestamp(Second, "UTC")` | ✗ | When this name was first observed |
| `ship_name` | `Utf8` | ✗ | The new vessel name (trimmed, uppercased) |

**Partitioning:** `year=YYYY/month=MM/name_history.parquet`

### Example: Get Current Name for a Vessel
```sql
SELECT ship_name
FROM read_parquet('…/fact_vessel_name_history/**/*.parquet', hive_partitioning=true)
WHERE mmsi = 257266500
ORDER BY timestamp_utc DESC
LIMIT 1;
```

### Example: Full Name History
```sql
SELECT ship_name, timestamp_utc
FROM read_parquet('…/fact_vessel_name_history/**/*.parquet', hive_partitioning=true)
WHERE mmsi = 257266500
ORDER BY timestamp_utc;
```

---

## 4. `fact_draught` — Draught Changes

Edge-filtered: one row per draught change per vessel. Draught values are
compared at 0.1 m resolution (multiplied by 10, rounded) to avoid false
pivots from floating-point noise.

| Column | Arrow Type | Nullable | Description |
|---|---|---|---|
| `mmsi` | `UInt32` | ✗ | Vessel MMSI |
| `timestamp_utc` | `Timestamp(Second, "UTC")` | ✗ | When the change was observed |
| `draught` | `Float32` | ✗ | New draught value (metres) |

**Partitioning:** `year=YYYY/month=MM/draught_changes.parquet`

### Example: Draught History with Previous Value
```sql
SELECT mmsi, timestamp_utc, draught,
       LAG(draught) OVER (PARTITION BY mmsi ORDER BY timestamp_utc) AS previous_draught
FROM read_parquet('…/fact_draught/**/*.parquet', hive_partitioning=true)
WHERE mmsi = 257421000
ORDER BY timestamp_utc;
```

---

## 5. `fact_navigational_status` — Nav Status Changes

Edge-filtered: one row per status change per vessel.

| Column | Arrow Type | Nullable | Description |
|---|---|---|---|
| `mmsi` | `UInt32` | ✗ | Vessel MMSI |
| `timestamp_utc` | `Timestamp(Second, "UTC")` | ✗ | When the change was observed |
| `navigational_status` | `UInt8` | ✗ | New AIS navigational status code |

**Partitioning:** `year=YYYY/month=MM/status_changes.parquet`

### AIS Navigational Status Codes
| Code | Meaning |
|---|---|
| 0 | Under way using engine |
| 1 | At anchor |
| 2 | Not under command |
| 3 | Restricted manoeuvrability |
| 4 | Constrained by draught |
| 5 | Moored |
| 6 | Aground |
| 7 | Engaged in fishing |
| 8 | Under way sailing |
| 14 | AIS-SART / MOB / EPIRB |
| 15 | Undefined (default) |

---

## 6. `fact_destination` — Destination Changes

Edge-filtered: one row per destination change per vessel. Destinations are
compared trimmed and uppercased. Empty destinations are ignored.

| Column | Arrow Type | Nullable | Description |
|---|---|---|---|
| `mmsi` | `UInt32` | ✗ | Vessel MMSI |
| `timestamp_utc` | `Timestamp(Second, "UTC")` | ✗ | When the change was observed |
| `destination` | `Utf8` | ✗ | New destination string (trimmed, uppercased) |
| `eta_utc` | `Timestamp(Second, "UTC")` | ✓ | Estimated time of arrival (merged from AIS month/day/hour/minute fields) |

**Partitioning:** `year=YYYY/month=MM/destination_changes.parquet`

### ETA Merging Logic
The four AIS ETA fields (month, day, hour, minute) are combined into a
single UTC timestamp. The current year is assumed; if the result is more
than 6 months in the past, the next year is used instead.

---

## 7. `metric_system_health` — Ingestion Telemetry

One row per micro-batch window. Tracks ingestion throughput and error rates.

| Column | Arrow Type | Nullable | Description |
|---|---|---|---|
| `window_start` | `Timestamp(Second, "UTC")` | ✗ | Batch processing start time |
| `window_end` | `Timestamp(Second, "UTC")` | ✗ | Batch processing end time |
| `position_reports` | `UInt32` | ✗ | Position messages in this batch |
| `static_data_reports` | `UInt32` | ✗ | Static data messages in this batch |
| `total_messages` | `UInt32` | ✗ | Total AIS messages parsed |
| `unique_mmsi_count` | `UInt32` | ✗ | Distinct vessels seen |
| `parse_errors` | `UInt32` | ✗ | JSON parse failures |

**Partitioning:** `year=YYYY/month=MM/health_metrics.parquet`

---

## How to Query — JOIN Examples

### Get vessel name + type for a position
```sql
SELECT p.mmsi, p.timestamp_utc, p.latitude, p.longitude,
       n.ship_name, s.ship_type, s.length_overall, s.beam
FROM read_parquet('…/fact_position/**/*.parquet', hive_partitioning=true) p
LEFT JOIN (
    SELECT mmsi, ship_name,
           ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
    FROM read_parquet('…/fact_vessel_name_history/**/*.parquet', hive_partitioning=true)
) n ON n.mmsi = p.mmsi AND n.rn = 1
LEFT JOIN (
    SELECT mmsi, ship_type, length_overall, beam,
           ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
    FROM read_parquet('…/dim_vessel/vessel_static.parquet')
) s ON s.mmsi = p.mmsi AND s.rn = 1
WHERE p.mmsi = 257266500;
```

### Get all cargo ships currently moored
```sql
WITH latest_nav AS (
    SELECT mmsi, navigational_status,
           ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
    FROM read_parquet('…/fact_navigational_status/**/*.parquet', hive_partitioning=true)
),
latest_type AS (
    SELECT mmsi, ship_type,
           ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
    FROM read_parquet('…/dim_vessel/vessel_static.parquet')
)
SELECT n.mmsi
FROM latest_nav n
JOIN latest_type t ON t.mmsi = n.mmsi AND t.rn = 1
WHERE n.rn = 1
  AND n.navigational_status = 5      -- Moored
  AND t.ship_type BETWEEN 70 AND 79; -- Cargo
```

---

## Storage & Partitioning

| Table | Partition Scheme | File Granularity |
|---|---|---|
| `fact_position` | `year/month/day` | 15-minute window per file |
| `dim_vessel` | None (flat) | Single file (append-on-change) |
| `fact_vessel_name_history` | `year/month` | One file per month (append) |
| `fact_draught` | `year/month` | One file per month (append) |
| `fact_navigational_status` | `year/month` | One file per month (append) |
| `fact_destination` | `year/month` | One file per month (append) |
| `metric_system_health` | `year/month` | One file per month (append) |

### Parquet Properties
- **Compression:** ZSTD level 3
- **Row group size:** 8,192 rows (optimized for 1 GB RAM)
- **Bloom filter:** On `mmsi` column (fact_position only) — 1% FPP, 50k NDV
- **Statistics:** Page-level min/max on all columns
- **Dictionary encoding:** Enabled globally
