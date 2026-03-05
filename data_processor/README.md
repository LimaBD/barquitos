# Data Processor

Reads the raw AIS JSONL files produced by `ais_collector`, parses them into
a normalised SQLite database, and moves processed files to a backup folder.

## Database Schema

| Table | Purpose |
|---|---|
| `vessels` | One row per MMSI. Ship identity, dimensions, type — upserted on every static data message. |
| `position_reports` | Time-series of position observations (lat, lon, SOG, COG, heading …). |
| `position_reports_rtree` | R\*Tree spatial index on positions for fast geographic queries. |
| `voyages` | Destination and ETA snapshots extracted from ShipStaticData messages. |
| `processed_files` | Bookkeeping: which JSONL files have already been ingested. |

The schema avoids data duplication — vessel identity is stored once and
referenced by foreign key from positions and voyages.

## Quick Start

```bash
# One-shot: process all pending files then exit
python data_processor/ingest_ais.py

# Watch mode: keep checking for new files every 30 seconds
python data_processor/ingest_ais.py --watch

# Custom interval
python data_processor/ingest_ais.py --watch --interval 60
```

## Configuration

Edit `data_processor/config.py`.  By default it reads
`ais_collector/config.py → DATA_OUTPUT_DIR` to know where the raw JSONL
files are, and creates a backup sibling folder (`ais_processed/`).

| Setting | Default | Description |
|---|---|---|
| `AIS_RAW_DIR` | from ais\_collector config | Where JSONL files are written |
| `AIS_BACKUP_DIR` | `<parent of raw dir>/ais_processed` | Where processed files are moved |
| `DATABASE_PATH` | `data_processor/barquitos.db` | SQLite database location |
| `BATCH_SIZE` | 5000 | Rows per INSERT batch |

## Performance Notes (Raspberry Pi 3B)

- **SQLite** with WAL journal mode — zero server overhead, low RAM.
- R\*Tree for spatial queries — built into SQLite, no extensions needed.
- Indexed on `(mmsi, timestamp_utc)` for fast vessel-timeline queries.
- `PRAGMA synchronous = NORMAL` + WAL gives good write throughput.
  Raw JSONL backups act as a safety net if the DB ever corrupts.

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

## Future Enrichment

This folder is designed to host additional scripts that enrich the database
with data from other sources (commodity prices, port schedules, weather, etc.).
Simply add new tables / columns and new Python scripts alongside `ingest_ais.py`.
