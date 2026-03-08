#!/usr/bin/env python3
"""
Incremental database deduplication / optimisation script.

Removes redundant rows from history tables to reclaim storage.
Only processes rows that have NOT yet been deduplicated (``deduplicated = 0``),
so re-running the script is essentially free when there is no new data.

Strategy per table:

1. **draught_history**              – keep only "inflection" rows (where draught changes).
2. **navigational_status_history**  – keep only inflection rows (where status changes).
3. **ship_name_history**            – keep only inflection rows (where name changes).
4. **destination_history**          – keep only inflection rows (where destination or ETA changes).
5. **position_history**             – keep max 1 row per vessel per 5-second window.

After deleting the redundant rows the surviving new rows are stamped
``deduplicated = 1`` so they will be skipped on the next run.

Usage:
    python optimizations/deduplicate.py [--db /path/to/barquitos.db] [--dry-run]

The script prints how many rows will be (or were) deleted from each table.
It runs each cleanup inside its own transaction and VACUUMs at the end.
"""

import argparse
import os
import sys
import time

# Allow importing the shared `db` package from the repo root
_project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _project_root)

from db.connection import get_connection

DEFAULT_DB = os.path.join("/mnt", "wdblack", "barquitos.db")


# =========================================================================
# Migration: add the `deduplicated` column if it doesn't exist yet
# =========================================================================

_HISTORY_TABLES = [
    "draught_history",
    "navigational_status_history",
    "ship_name_history",
    "destination_history",
    "position_history",
]


def _migrate_add_deduplicated(conn) -> None:
    """Add ``deduplicated`` column + partial index to every history table
    that doesn't have it yet.  Safe to call repeatedly (idempotent)."""
    for table in _HISTORY_TABLES:
        cols = {
            row[1]
            for row in conn.execute(f"PRAGMA table_info([{table}])").fetchall()
        }
        if "deduplicated" not in cols:
            print(f"  ⚙️  Migrating {table}: adding 'deduplicated' column …")
            conn.execute(
                f"ALTER TABLE [{table}] ADD COLUMN deduplicated INTEGER NOT NULL DEFAULT 0"
            )
            conn.execute(
                f"CREATE INDEX IF NOT EXISTS idx_{table}_dedup "
                f"ON [{table}](deduplicated) WHERE deduplicated = 0"
            )
            conn.commit()


# =========================================================================
# Helpers
# =========================================================================


def count_rows(conn, table: str) -> int:
    return conn.execute(f"SELECT COUNT(*) FROM [{table}]").fetchone()[0]


def count_pending(conn, table: str) -> int:
    """Rows not yet processed."""
    return conn.execute(
        f"SELECT COUNT(*) FROM [{table}] WHERE deduplicated = 0"
    ).fetchone()[0]


# =========================================================================
# Inflection-based dedup (draught / nav_status / ship_name / destination)
# =========================================================================
# The key idea for incremental inflection dedup:
#
# We build a CTE called ``scope`` that contains:
#   • every NEW row (deduplicated = 0), PLUS
#   • the single last FROZEN row per vessel (the "anchor").
#
# We then run LAG() over ``scope`` partitioned by mmsi.  Any new row whose
# value equals its predecessor (which may be the anchor) is redundant and
# gets deleted.  After that, all surviving new rows are stamped
# deduplicated = 1.
# =========================================================================


def dedup_draught_history(conn, dry_run: bool) -> int:
    """
    Incremental inflection dedup for draught_history.
    Keeps only rows where the draught value changes compared to the previous
    row for the same vessel.
    """
    # CTE: last frozen row per vessel + all new rows
    scope_cte = """
    WITH anchor AS (
        SELECT DISTINCT mmsi,
               FIRST_VALUE(id)        OVER w AS id,
               FIRST_VALUE(draught)   OVER w AS draught,
               FIRST_VALUE(timestamp_utc) OVER w AS timestamp_utc,
               1 AS deduplicated
        FROM draught_history
        WHERE deduplicated = 1
        WINDOW w AS (PARTITION BY mmsi ORDER BY timestamp_utc DESC, id DESC)
    ),
    scope AS (
        SELECT id, mmsi, draught, timestamp_utc FROM anchor
        UNION ALL
        SELECT id, mmsi, draught, timestamp_utc
        FROM draught_history WHERE deduplicated = 0
    ),
    flagged AS (
        SELECT id, mmsi, draught, timestamp_utc,
               LAG(draught) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_draught
        FROM scope
    )
    """

    delete_sql = f"""
    {scope_cte}
    DELETE FROM draught_history
    WHERE id IN (
        SELECT f.id FROM flagged f
        INNER JOIN draught_history dh ON dh.id = f.id AND dh.deduplicated = 0
        WHERE f.prev_draught IS NOT NULL
          AND f.draught = f.prev_draught
    )
    """

    count_sql = f"""
    {scope_cte}
    SELECT COUNT(*) FROM flagged f
    INNER JOIN draught_history dh ON dh.id = f.id AND dh.deduplicated = 0
    WHERE f.prev_draught IS NOT NULL
      AND f.draught = f.prev_draught
    """

    if dry_run:
        return conn.execute(count_sql).fetchone()[0]
    else:
        cur = conn.execute(delete_sql)
        removed = cur.rowcount
        # Mark surviving new rows as frozen
        conn.execute(
            "UPDATE draught_history SET deduplicated = 1 WHERE deduplicated = 0"
        )
        conn.commit()
        return removed


def dedup_nav_status_history(conn, dry_run: bool) -> int:
    """Incremental inflection dedup for navigational_status_history."""
    scope_cte = """
    WITH anchor AS (
        SELECT DISTINCT mmsi,
               FIRST_VALUE(id)                    OVER w AS id,
               FIRST_VALUE(navigational_status)   OVER w AS navigational_status,
               FIRST_VALUE(timestamp_utc)         OVER w AS timestamp_utc,
               1 AS deduplicated
        FROM navigational_status_history
        WHERE deduplicated = 1
        WINDOW w AS (PARTITION BY mmsi ORDER BY timestamp_utc DESC, id DESC)
    ),
    scope AS (
        SELECT id, mmsi, navigational_status, timestamp_utc FROM anchor
        UNION ALL
        SELECT id, mmsi, navigational_status, timestamp_utc
        FROM navigational_status_history WHERE deduplicated = 0
    ),
    flagged AS (
        SELECT id, mmsi, navigational_status, timestamp_utc,
               LAG(navigational_status) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_status
        FROM scope
    )
    """

    delete_sql = f"""
    {scope_cte}
    DELETE FROM navigational_status_history
    WHERE id IN (
        SELECT f.id FROM flagged f
        INNER JOIN navigational_status_history nh ON nh.id = f.id AND nh.deduplicated = 0
        WHERE f.prev_status IS NOT NULL
          AND f.navigational_status = f.prev_status
    )
    """

    count_sql = f"""
    {scope_cte}
    SELECT COUNT(*) FROM flagged f
    INNER JOIN navigational_status_history nh ON nh.id = f.id AND nh.deduplicated = 0
    WHERE f.prev_status IS NOT NULL
      AND f.navigational_status = f.prev_status
    """

    if dry_run:
        return conn.execute(count_sql).fetchone()[0]
    else:
        cur = conn.execute(delete_sql)
        removed = cur.rowcount
        conn.execute(
            "UPDATE navigational_status_history SET deduplicated = 1 WHERE deduplicated = 0"
        )
        conn.commit()
        return removed


def dedup_ship_name_history(conn, dry_run: bool) -> int:
    """Incremental inflection dedup for ship_name_history."""
    scope_cte = """
    WITH anchor AS (
        SELECT DISTINCT mmsi,
               FIRST_VALUE(id)            OVER w AS id,
               FIRST_VALUE(ship_name)     OVER w AS ship_name,
               FIRST_VALUE(timestamp_utc) OVER w AS timestamp_utc,
               1 AS deduplicated
        FROM ship_name_history
        WHERE deduplicated = 1
        WINDOW w AS (PARTITION BY mmsi ORDER BY timestamp_utc DESC, id DESC)
    ),
    scope AS (
        SELECT id, mmsi, ship_name, timestamp_utc FROM anchor
        UNION ALL
        SELECT id, mmsi, ship_name, timestamp_utc
        FROM ship_name_history WHERE deduplicated = 0
    ),
    flagged AS (
        SELECT id, mmsi, ship_name, timestamp_utc,
               LAG(ship_name) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_name
        FROM scope
    )
    """

    delete_sql = f"""
    {scope_cte}
    DELETE FROM ship_name_history
    WHERE id IN (
        SELECT f.id FROM flagged f
        INNER JOIN ship_name_history sn ON sn.id = f.id AND sn.deduplicated = 0
        WHERE f.prev_name IS NOT NULL
          AND f.ship_name = f.prev_name
    )
    """

    count_sql = f"""
    {scope_cte}
    SELECT COUNT(*) FROM flagged f
    INNER JOIN ship_name_history sn ON sn.id = f.id AND sn.deduplicated = 0
    WHERE f.prev_name IS NOT NULL
      AND f.ship_name = f.prev_name
    """

    if dry_run:
        return conn.execute(count_sql).fetchone()[0]
    else:
        cur = conn.execute(delete_sql)
        removed = cur.rowcount
        conn.execute(
            "UPDATE ship_name_history SET deduplicated = 1 WHERE deduplicated = 0"
        )
        conn.commit()
        return removed


def dedup_destination_history(conn, dry_run: bool) -> int:
    """
    Incremental inflection dedup for destination_history.
    A row is redundant when both destination AND estimated_arrival match the
    previous row for the same vessel.
    """
    scope_cte = """
    WITH anchor AS (
        SELECT DISTINCT mmsi,
               FIRST_VALUE(id)                OVER w AS id,
               FIRST_VALUE(destination)       OVER w AS destination,
               FIRST_VALUE(estimated_arrival) OVER w AS estimated_arrival,
               FIRST_VALUE(timestamp_utc)     OVER w AS timestamp_utc,
               1 AS deduplicated
        FROM destination_history
        WHERE deduplicated = 1
        WINDOW w AS (PARTITION BY mmsi ORDER BY timestamp_utc DESC, id DESC)
    ),
    scope AS (
        SELECT id, mmsi, destination, estimated_arrival, timestamp_utc FROM anchor
        UNION ALL
        SELECT id, mmsi, destination, estimated_arrival, timestamp_utc
        FROM destination_history WHERE deduplicated = 0
    ),
    flagged AS (
        SELECT id, mmsi, destination, estimated_arrival, timestamp_utc,
               LAG(destination)       OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_dest,
               LAG(estimated_arrival) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_eta
        FROM scope
    )
    """

    delete_sql = f"""
    {scope_cte}
    DELETE FROM destination_history
    WHERE id IN (
        SELECT f.id FROM flagged f
        INNER JOIN destination_history dh ON dh.id = f.id AND dh.deduplicated = 0
        WHERE f.prev_dest IS NOT NULL
          AND COALESCE(f.destination, '') = COALESCE(f.prev_dest, '')
          AND COALESCE(f.estimated_arrival, 0) = COALESCE(f.prev_eta, 0)
    )
    """

    count_sql = f"""
    {scope_cte}
    SELECT COUNT(*) FROM flagged f
    INNER JOIN destination_history dh ON dh.id = f.id AND dh.deduplicated = 0
    WHERE f.prev_dest IS NOT NULL
      AND COALESCE(f.destination, '') = COALESCE(f.prev_dest, '')
      AND COALESCE(f.estimated_arrival, 0) = COALESCE(f.prev_eta, 0)
    """

    if dry_run:
        return conn.execute(count_sql).fetchone()[0]
    else:
        cur = conn.execute(delete_sql)
        removed = cur.rowcount
        conn.execute(
            "UPDATE destination_history SET deduplicated = 1 WHERE deduplicated = 0"
        )
        conn.commit()
        return removed


# =========================================================================
# Position history — bucket-based dedup (incremental)
# =========================================================================
# We only look at new (deduplicated = 0) rows. For each (mmsi, 5s-bucket)
# we also consider already-frozen rows that share the same bucket so we
# don't introduce duplicates across runs.  After cleanup the survivors
# are stamped deduplicated = 1.
# =========================================================================


def dedup_position_history(conn, dry_run: bool) -> int:
    """
    Keep at most 1 position per vessel per 5-second window.
    For each (mmsi, 5-second bucket) keep the row with the smallest id
    and delete the rest.  Only buckets that contain at least one new row
    are evaluated.
    """
    # Identify which (mmsi, bucket) pairs contain new rows
    redundant_sql = """
    SELECT ph.id
    FROM position_history ph
    INNER JOIN (
        -- buckets that have a new row
        SELECT mmsi, (timestamp_utc / 5) AS bucket
        FROM position_history
        WHERE deduplicated = 0
        GROUP BY mmsi, bucket
    ) nb ON ph.mmsi = nb.mmsi AND (ph.timestamp_utc / 5) = nb.bucket
    INNER JOIN (
        -- the keeper (smallest id) in each of those buckets across ALL rows
        SELECT mmsi, (timestamp_utc / 5) AS bucket, MIN(id) AS keep_id
        FROM position_history
        WHERE mmsi IN (SELECT DISTINCT mmsi FROM position_history WHERE deduplicated = 0)
        GROUP BY mmsi, bucket
    ) g ON ph.mmsi = g.mmsi
          AND (ph.timestamp_utc / 5) = g.bucket
          AND ph.id != g.keep_id
    """

    count_sql = f"SELECT COUNT(*) FROM ({redundant_sql})"

    if dry_run:
        return conn.execute(count_sql).fetchone()[0]
    else:
        # Delete from R*Tree first
        conn.execute(
            f"DELETE FROM position_history_rtree WHERE id IN ({redundant_sql})"
        )
        cur = conn.execute(
            f"DELETE FROM position_history WHERE id IN ({redundant_sql})"
        )
        removed = cur.rowcount
        # Mark surviving new rows as frozen
        conn.execute(
            "UPDATE position_history SET deduplicated = 1 WHERE deduplicated = 0"
        )
        conn.commit()
        return removed


# =========================================================================
# Main
# =========================================================================


def main():
    parser = argparse.ArgumentParser(description="Deduplicate AIS history tables")
    parser.add_argument("--db", default=DEFAULT_DB, help="Path to SQLite database")
    parser.add_argument(
        "--dry-run", action="store_true", help="Only count, don't delete"
    )
    args = parser.parse_args()

    if not os.path.isfile(args.db):
        print(f"❌  Database not found: {args.db}")
        sys.exit(1)

    conn = get_connection(args.db)
    mode = "DRY RUN" if args.dry_run else "LIVE"
    print(f"🔧  Incremental deduplication ({mode}) — {args.db}\n")

    # Ensure the deduplicated column exists (migration for old databases)
    _migrate_add_deduplicated(conn)

    tasks = [
        ("draught_history", dedup_draught_history),
        ("navigational_status_history", dedup_nav_status_history),
        ("ship_name_history", dedup_ship_name_history),
        ("destination_history", dedup_destination_history),
        ("position_history", dedup_position_history),
    ]

    total_removed = 0
    for table_name, func in tasks:
        total = count_rows(conn, table_name)
        pending = count_pending(conn, table_name)
        t0 = time.time()
        removed = func(conn, args.dry_run)
        elapsed = time.time() - t0
        verb = "would remove" if args.dry_run else "removed"
        print(
            f"  {table_name:40s}  {total:>12,} total, {pending:>10,} pending  "
            f"→ {verb} {removed:,} rows in {elapsed:.1f}s"
        )
        total_removed += removed

    if not args.dry_run and total_removed > 0:
        print(f"\n  🗜️  Running VACUUM …")
        t0 = time.time()
        conn.execute("VACUUM")
        print(f"  Done in {time.time() - t0:.1f}s")

    print(
        f"\n✅  Total rows {'to remove' if args.dry_run else 'removed'}: "
        f"{total_removed:,}"
    )
    conn.close()


if __name__ == "__main__":
    main()
