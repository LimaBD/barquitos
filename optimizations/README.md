# Database Optimisation Scripts

Scripts in this folder clean up and compact the SQLite database.
They are safe to re-run at any time (idempotent).

## `deduplicate.py`

Removes redundant rows from history tables:

| Table | Strategy |
|---|---|
| `draught_history` | Keep only inflection rows — when draught value changes |
| `navigational_status_history` | Keep only inflection rows — when nav status changes |
| `ship_name_history` | Keep only inflection rows — when ship name changes |
| `position_history` | Keep max 1 row per vessel per 5-second bucket |

### Usage

```bash
# Dry run (count only, no changes):
python optimizations/deduplicate.py --dry-run

# Live run:
python optimizations/deduplicate.py

# Custom database path:
python optimizations/deduplicate.py --db /path/to/barquitos.db
```

### Ingest-time deduplication

The Rust ingestor (`ais_ingestor_rs`) already applies these rules at ingest time:
- **Draughts**: only inserted when the value differs from the previous one for the same vessel
- **Nav statuses**: only inserted when the status differs
- **Ship names**: only inserted when the name differs  
- **Positions**: throttled to max 1 per vessel per 5 seconds

This script handles data that was ingested before these optimisations were added.
