# AIS Ingestor

Reads the raw AIS JSONL files produced by `ais_collector`, parses them into
the normalised SQLite database (managed by `db/`), and moves processed files
to a backup folder.

Non-commercial vessels (yachts, sailing boats) are filtered out automatically
based on the AIS ship-type code.

## Quick Start

```bash
# From the project root:

# One-shot: process all pending files then exit
python -m ais_ingestor

# Watch mode: keep checking for new files every 30 seconds
python -m ais_ingestor --watch

# Custom interval
python -m ais_ingestor --watch --interval 60

# Override directories
python -m ais_ingestor --input-dir ./ais_collector/data_samples --backup-dir /tmp/processed
```

## Configuration

Edit `ais_ingestor/config.py`.  By default it reads
`ais_collector/config.py → DATA_OUTPUT_DIR` to know where the raw JSONL
files are, and creates a backup sibling folder (`ais_processed/`).

| Setting | Default | Description |
|---|---|---|
| `AIS_RAW_DIR` | from ais\_collector config | Where JSONL files are written |
| `AIS_BACKUP_DIR` | `<parent of raw dir>/ais_processed` | Where processed files are moved |
| `DATABASE_PATH` | `/mnt/wdblack/barquitos.db` | SQLite database location |
| `BATCH_SIZE` | 5000 | Rows per INSERT batch |
| `EXCLUDED_SHIP_TYPES` | `{36, 37}` | AIS type codes to skip (sailing, yachts) |

## Performance Notes

Compared to the original `data_processor`, this version is significantly faster:

- **Vessel upserts deduplicated by MMSI** — the old code did one `INSERT OR UPDATE`
  per message (~22k per file); now only unique MMSIs are upserted.
- **Non-commercial filtering** — skips yachts / sailing vessels early, reducing
  both parsing and DB write volume.
- **Non-ship MMSI filtering** — coast stations, aids-to-navigation, etc. are
  skipped before any DB work.
- **mmap_size pragma** — enables memory-mapped I/O for faster SQLite reads.
