#!/usr/bin/env python3
"""
Database deduplication / optimisation script.

Removes redundant rows from history tables to reclaim storage:

1. **draught_history**              – keep only "inflection" rows (where draught changes).
2. **navigational_status_history**  – keep only inflection rows (where status changes).
3. **ship_name_history**            – keep only inflection rows (where name changes).
4. **destination_history**          – keep only inflection rows (where destination or ETA changes).
5. **position_history**             – keep max 1 row per vessel per 5-second window.

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


def count_rows(conn, table: str) -> int:
    return conn.execute(f"SELECT COUNT(*) FROM [{table}]").fetchone()[0]


def dedup_draught_history(conn, dry_run: bool) -> int:
    """
    Keep only inflection rows in draught_history.
    An inflection row is the first row (by timestamp) where the draught value
    differs from the previous row for the same vessel.
    """
    sql = """
    DELETE FROM draught_history
    WHERE id IN (
        SELECT dh.id
        FROM draught_history dh
        INNER JOIN (
            SELECT id, mmsi, draught, timestamp_utc,
                   LAG(draught) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_draught
            FROM draught_history
        ) w ON dh.id = w.id
        WHERE w.prev_draught IS NOT NULL
          AND w.draught = w.prev_draught
    )
    """
    if dry_run:
        count_sql = """
        SELECT COUNT(*)
        FROM draught_history dh
        INNER JOIN (
            SELECT id, mmsi, draught, timestamp_utc,
                   LAG(draught) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_draught
            FROM draught_history
        ) w ON dh.id = w.id
        WHERE w.prev_draught IS NOT NULL
          AND w.draught = w.prev_draught
        """
        return conn.execute(count_sql).fetchone()[0]
    else:
        cur = conn.execute(sql)
        conn.commit()
        return cur.rowcount


def dedup_nav_status_history(conn, dry_run: bool) -> int:
    """Keep only inflection rows in navigational_status_history."""
    sql = """
    DELETE FROM navigational_status_history
    WHERE id IN (
        SELECT nh.id
        FROM navigational_status_history nh
        INNER JOIN (
            SELECT id, mmsi, navigational_status, timestamp_utc,
                   LAG(navigational_status) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_status
            FROM navigational_status_history
        ) w ON nh.id = w.id
        WHERE w.prev_status IS NOT NULL
          AND w.navigational_status = w.prev_status
    )
    """
    if dry_run:
        count_sql = """
        SELECT COUNT(*)
        FROM navigational_status_history nh
        INNER JOIN (
            SELECT id, mmsi, navigational_status, timestamp_utc,
                   LAG(navigational_status) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_status
            FROM navigational_status_history
        ) w ON nh.id = w.id
        WHERE w.prev_status IS NOT NULL
          AND w.navigational_status = w.prev_status
        """
        return conn.execute(count_sql).fetchone()[0]
    else:
        cur = conn.execute(sql)
        conn.commit()
        return cur.rowcount


def dedup_ship_name_history(conn, dry_run: bool) -> int:
    """Keep only inflection rows in ship_name_history."""
    sql = """
    DELETE FROM ship_name_history
    WHERE id IN (
        SELECT sn.id
        FROM ship_name_history sn
        INNER JOIN (
            SELECT id, mmsi, ship_name, timestamp_utc,
                   LAG(ship_name) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_name
            FROM ship_name_history
        ) w ON sn.id = w.id
        WHERE w.prev_name IS NOT NULL
          AND w.ship_name = w.prev_name
    )
    """
    if dry_run:
        count_sql = """
        SELECT COUNT(*)
        FROM ship_name_history sn
        INNER JOIN (
            SELECT id, mmsi, ship_name, timestamp_utc,
                   LAG(ship_name) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_name
            FROM ship_name_history
        ) w ON sn.id = w.id
        WHERE w.prev_name IS NOT NULL
          AND w.ship_name = w.prev_name
        """
        return conn.execute(count_sql).fetchone()[0]
    else:
        cur = conn.execute(sql)
        conn.commit()
        return cur.rowcount


def dedup_destination_history(conn, dry_run: bool) -> int:
    """
    Keep only inflection rows in destination_history.
    An inflection row is where (destination, estimated_arrival) differs
    from the previous row for the same vessel.
    """
    sql = """
    DELETE FROM destination_history
    WHERE id IN (
        SELECT dh.id
        FROM destination_history dh
        INNER JOIN (
            SELECT id, mmsi, destination, estimated_arrival, timestamp_utc,
                   LAG(destination)       OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_dest,
                   LAG(estimated_arrival) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_eta
            FROM destination_history
        ) w ON dh.id = w.id
        WHERE w.prev_dest IS NOT NULL
          AND COALESCE(w.destination, '') = COALESCE(w.prev_dest, '')
          AND COALESCE(w.estimated_arrival, 0) = COALESCE(w.prev_eta, 0)
    )
    """
    if dry_run:
        count_sql = """
        SELECT COUNT(*)
        FROM destination_history dh
        INNER JOIN (
            SELECT id, mmsi, destination, estimated_arrival, timestamp_utc,
                   LAG(destination)       OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_dest,
                   LAG(estimated_arrival) OVER (PARTITION BY mmsi ORDER BY timestamp_utc, id) AS prev_eta
            FROM destination_history
        ) w ON dh.id = w.id
        WHERE w.prev_dest IS NOT NULL
          AND COALESCE(w.destination, '') = COALESCE(w.prev_dest, '')
          AND COALESCE(w.estimated_arrival, 0) = COALESCE(w.prev_eta, 0)
        """
        return conn.execute(count_sql).fetchone()[0]
    else:
        cur = conn.execute(sql)
        conn.commit()
        return cur.rowcount


def dedup_position_history(conn, dry_run: bool) -> int:
    """
    Keep at most 1 position per vessel per 5-second window.
    For each (mmsi, 5-second bucket) keep the row with the smallest id
    and delete the rest.
    """
    sql = """
    DELETE FROM position_history
    WHERE id IN (
        SELECT ph.id
        FROM position_history ph
        INNER JOIN (
            SELECT mmsi, (timestamp_utc / 5) AS bucket,
                   MIN(id) AS keep_id
            FROM position_history
            GROUP BY mmsi, bucket
            HAVING COUNT(*) > 1
        ) g ON ph.mmsi = g.mmsi
              AND (ph.timestamp_utc / 5) = g.bucket
              AND ph.id != g.keep_id
    )
    """
    if dry_run:
        count_sql = """
        SELECT COUNT(*)
        FROM position_history ph
        INNER JOIN (
            SELECT mmsi, (timestamp_utc / 5) AS bucket,
                   MIN(id) AS keep_id
            FROM position_history
            GROUP BY mmsi, bucket
            HAVING COUNT(*) > 1
        ) g ON ph.mmsi = g.mmsi
              AND (ph.timestamp_utc / 5) = g.bucket
              AND ph.id != g.keep_id
        """
        return conn.execute(count_sql).fetchone()[0]
    else:
        # Also clean the R*Tree for deleted rows
        deleted_ids_sql = """
        SELECT ph.id
        FROM position_history ph
        INNER JOIN (
            SELECT mmsi, (timestamp_utc / 5) AS bucket,
                   MIN(id) AS keep_id
            FROM position_history
            GROUP BY mmsi, bucket
            HAVING COUNT(*) > 1
        ) g ON ph.mmsi = g.mmsi
              AND (ph.timestamp_utc / 5) = g.bucket
              AND ph.id != g.keep_id
        """
        # Delete from R*Tree first
        conn.execute(
            f"DELETE FROM position_history_rtree WHERE id IN ({deleted_ids_sql})"
        )
        cur = conn.execute(sql)
        conn.commit()
        return cur.rowcount


def main():
    parser = argparse.ArgumentParser(description="Deduplicate AIS history tables")
    parser.add_argument("--db", default=DEFAULT_DB, help="Path to SQLite database")
    parser.add_argument("--dry-run", action="store_true", help="Only count, don't delete")
    args = parser.parse_args()

    if not os.path.isfile(args.db):
        print(f"❌  Database not found: {args.db}")
        sys.exit(1)

    conn = get_connection(args.db)
    mode = "DRY RUN" if args.dry_run else "LIVE"
    print(f"🔧  Deduplication ({mode}) — {args.db}\n")

    tasks = [
        ("draught_history", dedup_draught_history),
        ("navigational_status_history", dedup_nav_status_history),
        ("ship_name_history", dedup_ship_name_history),
        ("destination_history", dedup_destination_history),
        ("position_history", dedup_position_history),
    ]

    total_removed = 0
    for table_name, func in tasks:
        before = count_rows(conn, table_name)
        t0 = time.time()
        removed = func(conn, args.dry_run)
        elapsed = time.time() - t0
        after = before - removed if not args.dry_run else before
        verb = "would remove" if args.dry_run else "removed"
        print(
            f"  {table_name:40s}  {before:>12,} → {after:>12,}  "
            f"({verb} {removed:,} rows in {elapsed:.1f}s)"
        )
        total_removed += removed

    if not args.dry_run and total_removed > 0:
        print(f"\n  🗜️  Running VACUUM …")
        t0 = time.time()
        conn.execute("VACUUM")
        print(f"  Done in {time.time() - t0:.1f}s")

    print(f"\n✅  Total rows {'to remove' if args.dry_run else 'removed'}: {total_removed:,}")
    conn.close()


if __name__ == "__main__":
    main()
