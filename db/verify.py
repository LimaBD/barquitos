#!/usr/bin/env python3
"""Quick verification of the barquitos database."""

import os
import sqlite3
import sys
from datetime import datetime, timezone

# ---------------------------------------------------------------------------
# Allow running from repo root: python -m db.verify
# or directly:                  python db/verify.py
# ---------------------------------------------------------------------------
_project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _project_root)

from db.connection import get_connection  # noqa: E402

# Default DB path — can be overridden with first CLI argument
DEFAULT_DB = os.path.join("/mnt", "wdblack", "barquitos.db")


def _ts(epoch: int | None) -> str:
    """Format a Unix epoch timestamp as ISO-8601 for display."""
    if epoch is None:
        return "N/A"
    return datetime.fromtimestamp(epoch, tz=timezone.utc).isoformat()


def main():
    db_path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_DB
    if not os.path.exists(db_path):
        print(f"Database not found: {db_path}")
        sys.exit(1)

    conn = get_connection(db_path)

    print("=== ROW COUNTS ===")
    for t in [
        "ship_types", "nav_statuses", "vessels",
        "ship_name_history", "destination_history", "draught_history",
        "navigational_status_history", "position_history", "position_history_rtree",
    ]:
        n = conn.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
        print(f"  {t:35s} {n:>10,}")

    print("\n=== SAMPLE VESSELS ===")
    for r in conn.execute(
        """SELECT v.mmsi, v.ship_name, st.label, v.imo_number, v.length_m, v.beam_m
           FROM vessels v
           LEFT JOIN ship_types st ON st.ship_type = v.ship_type
           WHERE v.ship_name IS NOT NULL LIMIT 5"""
    ):
        print(f"  MMSI={r[0]}  name={r[1]!r}  type={r[2]}  imo={r[3]}  L={r[4]}m  B={r[5]}m")

    print("\n=== SAMPLE POSITIONS ===")
    for r in conn.execute(
        "SELECT mmsi, timestamp_utc, latitude, longitude, speed_over_ground, course_over_ground FROM position_history LIMIT 5"
    ):
        print(f"  MMSI={r[0]}  ts={_ts(r[1])}  lat={r[2]:.4f}  lon={r[3]:.4f}  sog={r[4]}  cog={r[5]}")

    print("\n=== SAMPLE DESTINATIONS ===")
    for r in conn.execute(
        "SELECT mmsi, destination, estimated_arrival, timestamp_utc FROM destination_history LIMIT 5"
    ):
        print(f"  MMSI={r[0]}  dest={r[1]!r}  eta={_ts(r[2])}  at={_ts(r[3])}")

    print("\n=== SAMPLE DRAUGHTS ===")
    for r in conn.execute(
        "SELECT mmsi, draught, timestamp_utc FROM draught_history LIMIT 5"
    ):
        print(f"  MMSI={r[0]}  draught={r[1]}  at={_ts(r[2])}")

    print("\n=== SAMPLE NAV STATUS CHANGES ===")
    for r in conn.execute(
        """SELECT h.mmsi, ns.label, h.timestamp_utc
           FROM navigational_status_history h
           LEFT JOIN nav_statuses ns ON ns.navigational_status = h.navigational_status
           LIMIT 5"""
    ):
        print(f"  MMSI={r[0]}  status={r[1]!r}  at={_ts(r[2])}")

    print("\n=== SPATIAL: vessels near Barcelona ===")
    for r in conn.execute("""
        SELECT DISTINCT v.mmsi, v.ship_name, p.latitude, p.longitude
        FROM position_history_rtree AS rt
        JOIN position_history AS p ON p.id = rt.id
        JOIN vessels AS v ON v.mmsi = p.mmsi
        WHERE rt.min_lat >= 41.0 AND rt.max_lat <= 41.5
          AND rt.min_lon >= 2.0  AND rt.max_lon <= 2.5
        LIMIT 5
    """):
        print(f"  MMSI={r[0]}  name={r[1]!r}  lat={r[2]:.4f}  lon={r[3]:.4f}")

    sz = os.path.getsize(db_path)
    print(f"\n=== DB SIZE: {sz/1024/1024:.1f} MB ===")
    conn.close()


if __name__ == "__main__":
    main()
