#!/usr/bin/env python3
"""Quick verification of the barquitos database."""
import sqlite3, os

DB = os.path.join(os.path.dirname(os.path.abspath(__file__)), "barquitos.db")
conn = sqlite3.connect(DB)

print("=== ROW COUNTS ===")
for t in ["vessels", "position_reports", "position_reports_rtree", "voyages", "processed_files"]:
    n = conn.execute(f"SELECT COUNT(*) FROM {t}").fetchone()[0]
    print(f"  {t:35s} {n:>10,}")

print("\n=== SAMPLE VESSELS ===")
for r in conn.execute("SELECT mmsi, ship_name, ship_type, imo_number FROM vessels WHERE ship_name IS NOT NULL LIMIT 5"):
    print(f"  MMSI={r[0]}  name={r[1]!r}  type={r[2]}  imo={r[3]}")

print("\n=== SAMPLE POSITIONS ===")
for r in conn.execute("SELECT mmsi, timestamp_utc, latitude, longitude, sog, cog FROM position_reports LIMIT 5"):
    print(f"  MMSI={r[0]}  ts={r[1]}  lat={r[2]:.4f}  lon={r[3]:.4f}  sog={r[4]}  cog={r[5]}")

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

print("\n=== PROCESSED FILES ===")
for r in conn.execute("SELECT filename, row_count, positions_added, voyages_added, vessels_upserted FROM processed_files"):
    print(f"  {r[0]:40s}  rows={r[1]:>6}  pos={r[2]:>6}  voy={r[3]:>5}  ves={r[4]:>5}")

sz = os.path.getsize(DB)
print(f"\n=== DB SIZE: {sz/1024/1024:.1f} MB ===")
conn.close()
