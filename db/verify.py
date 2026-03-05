#!/usr/bin/env python3
# filepath: /home/msi/Desktop/barquitos/db/verify.py
"""Quick verification of the barquitos database."""

import os
import sqlite3
import sys

# ---------------------------------------------------------------------------
# Allow running from repo root: python -m db.verify
# or directly:                  python db/verify.py
# ---------------------------------------------------------------------------
_project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _project_root)

from db.connection import get_connection  # noqa: E402

# Default DB path — can be overridden with first CLI argument
DEFAULT_DB = os.path.join("/mnt", "wdblack", "barquitos.db")


def main():
    db_path = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_DB
    if not os.path.exists(db_path):
        print(f"Database not found: {db_path}")
        sys.exit(1)

    conn = get_connection(db_path)

    print("=== ROW COUNTS ===")
    for t in ["ship_types", "vessels", "ports", "voyages", "position_reports", "position_reports_rtree"]:
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
        "SELECT mmsi, timestamp_utc, latitude, longitude, sog, cog, voyage_id FROM position_reports LIMIT 5"
    ):
        print(f"  MMSI={r[0]}  ts={r[1]}  lat={r[2]:.4f}  lon={r[3]:.4f}  sog={r[4]}  cog={r[5]}  voy={r[6]}")

    print("\n=== SAMPLE VOYAGES ===")
    for r in conn.execute(
        "SELECT mmsi, destination, eta, cargo_status, draught, reported_utc FROM voyages LIMIT 5"
    ):
        print(f"  MMSI={r[0]}  dest={r[1]!r}  eta={r[2]}  cargo={r[3]}  draught={r[4]}  at={r[5]}")

    print("\n=== SPATIAL: vessels near Barcelona ===")
    for r in conn.execute("""
        SELECT DISTINCT v.mmsi, v.ship_name, p.latitude, p.longitude
        FROM position_reports_rtree AS rt
        JOIN position_reports AS p ON p.id = rt.id
        JOIN vessels AS v ON v.mmsi = p.mmsi
        WHERE rt.min_lat >= 41.0 AND rt.max_lat <= 41.5
          AND rt.min_lon >= 2.0  AND rt.max_lon <= 2.5
        LIMIT 5
    """):
        print(f"  MMSI={r[0]}  name={r[1]!r}  lat={r[2]:.4f}  lon={r[3]:.4f}")

    print("\n=== VESSEL STATUS VIEW (sample) ===")
    for r in conn.execute(
        """SELECT mmsi, ship_name, ship_type_label, ship_type_category,
                  last_lat, last_lon, last_sog, destination, eta, cargo_status
           FROM v_vessel_status
           WHERE destination IS NOT NULL LIMIT 5"""
    ):
        print(f"  MMSI={r[0]}  {r[1]!r}  [{r[2]}/{r[3]}]  "
              f"pos=({r[4]:.4f},{r[5]:.4f})  sog={r[6]}  "
              f"dest={r[7]!r}  eta={r[8]}  cargo={r[9]}")

    sz = os.path.getsize(db_path)
    print(f"\n=== DB SIZE: {sz/1024/1024:.1f} MB ===")
    conn.close()


if __name__ == "__main__":
    main()
