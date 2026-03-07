# Database

Shared SQLite database layer for all barquitos components.

## Schema

| Table | Purpose |
|---|---|
| `ship_types` | Lookup table mapping AIS ship-type codes → human-readable labels & categories. |
| `nav_statuses` | Lookup table mapping AIS navigational-status codes → human-readable labels. |
| `vessels` | One row per MMSI. Ship identity, dimensions, type — upserted on every static data message. |
| `ship_name_history` | Ship name changes over time (names can change). |
| `destination_history` | Destination & ETA snapshots from ShipStaticData messages. |
| `draught_history` | Draught snapshots from ShipStaticData messages. |
| `navigational_status_history` | Nav status changes (infrequent, ~3/month per vessel). |
| `position_history` | Time-series of position observations (lat, lon, speed, course, heading …). |
| `position_history_rtree` | R\*Tree spatial index on positions for fast geographic queries. |

The schema avoids data duplication — vessel identity is stored once and
referenced by foreign key from all history tables. Infrequently-changing
data (nav status, destination, draught, ship name) is stored in separate
history tables to minimise row volume.

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
SELECT * FROM position_history
WHERE mmsi = 354128000
  AND timestamp_utc >= datetime('now', '-1 hour')
ORDER BY timestamp_utc;

-- Vessels currently inside a bounding box (fast spatial lookup)
SELECT v.mmsi, v.ship_name, p.latitude, p.longitude, p.speed_over_ground
FROM position_history_rtree AS r
JOIN position_history AS p ON p.id = r.id
JOIN vessels AS v ON v.mmsi = p.mmsi
WHERE r.min_lat >= 40.0 AND r.max_lat <= 42.0
  AND r.min_lon >= 1.0  AND r.max_lon <= 3.0
ORDER BY p.timestamp_utc DESC;

-- All known vessels with their type
SELECT mmsi, ship_name, ship_type, imo_number
FROM vessels
WHERE ship_name IS NOT NULL
ORDER BY last_seen_utc DESC;

-- Latest destination per vessel
SELECT mmsi, destination, estimated_arrival, timestamp_utc
FROM destination_history
WHERE id IN (SELECT MAX(id) FROM destination_history GROUP BY mmsi);
```
