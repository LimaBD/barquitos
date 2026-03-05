# Database

Shared SQLite database layer for all barquitos components.

## Schema

| Table | Purpose |
|---|---|
| `vessels` | One row per MMSI. Ship identity, dimensions, type — upserted on every static data message. |
| `position_reports` | Time-series of position observations (lat, lon, SOG, COG, heading …). |
| `position_reports_rtree` | R\*Tree spatial index on positions for fast geographic queries. |
| `voyages` | Destination and ETA snapshots extracted from ShipStaticData messages. |
| `processed_files` | Bookkeeping: which JSONL files have already been ingested. |

The schema avoids data duplication — vessel identity is stored once and
referenced by foreign key from positions and voyages.

## Usage

```python
from db import init_db, get_connection

# First time: create schema and get connection
conn = init_db("/path/to/barquitos.db")

# Subsequent: just get a tuned connection
conn = get_connection("/path/to/barquitos.db")
```

## Verification

```bash
python db/verify.py                          # uses default DB path
python db/verify.py /path/to/barquitos.db    # explicit path
```

## Performance Tuning (Raspberry Pi 3B)

- **SQLite** with WAL journal mode — zero server overhead, low RAM.
- R\*Tree for spatial queries — built into SQLite, no extensions needed.
- Indexed on `(mmsi, timestamp_utc)` for fast vessel-timeline queries.
- `PRAGMA synchronous = NORMAL` + WAL gives good write throughput.
- `PRAGMA mmap_size = 256 MB` — memory-mapped I/O for faster reads.
- Raw JSONL backups act as a safety net if the DB ever corrupts.

## Example Queries

```sql
-- Positions of a specific vessel in the last hour
SELECT * FROM position_reports
WHERE mmsi = 354128000
  AND timestamp_utc >= datetime('now', '-1 hour')
ORDER BY timestamp_utc;

-- Vessels currently inside a bounding box (fast spatial lookup)
SELECT v.mmsi, v.ship_name, p.latitude, p.longitude, p.sog
FROM position_reports_rtree AS r
JOIN position_reports AS p ON p.id = r.id
JOIN vessels AS v ON v.mmsi = p.mmsi
WHERE r.min_lat >= 40.0 AND r.max_lat <= 42.0
  AND r.min_lon >= 1.0  AND r.max_lon <= 3.0
ORDER BY p.timestamp_utc DESC;

-- All known vessels with their type
SELECT mmsi, ship_name, ship_type, imo_number
FROM vessels
WHERE ship_name IS NOT NULL
ORDER BY last_seen_utc DESC;
```
