# Database Optimisation Scripts

Scripts in this folder clean up and compact the SQLite database.
They are safe to re-run at any time (idempotent).

## `deduplicate.py`

**Incremental** deduplication — only processes rows that haven't been
deduplicated yet (`deduplicated = 0`).  Re-running the script after the
first pass is essentially free when no new data has been ingested.

### How it works

Each history table has a `deduplicated` column (`INTEGER NOT NULL DEFAULT 0`).
New rows inserted by the Rust ingestor arrive with `deduplicated = 0`.

When the script runs it:

1. **Migrates** old databases that don't have the column yet (idempotent
   `ALTER TABLE … ADD COLUMN`).
2. For each table, builds a *scope* that includes:
   - all pending rows (`deduplicated = 0`), plus
   - the last frozen row per vessel (the "anchor") so it can detect whether
     the first new row is actually a duplicate of the most recent known value.
3. Deletes redundant rows from the pending set.
4. Stamps all surviving pending rows as `deduplicated = 1` ("frozen").
5. Runs `VACUUM` if any rows were deleted.

| Table | Strategy |
|---|---|
| `draught_history` | Keep only inflection rows — when draught value changes |
| `navigational_status_history` | Keep only inflection rows — when nav status changes |
| `ship_name_history` | Keep only inflection rows — when ship name changes |
| `destination_history` | Keep only inflection rows — when destination **or** ETA changes |
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

This script handles data that was ingested before these optimisations were added,
as well as cross-file/cross-batch duplicates that the ingestor can't catch.
