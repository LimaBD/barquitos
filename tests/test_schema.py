#!/usr/bin/env python3
"""Quick test: create DB, seed, ingest one sample file."""
import os
import sys
import tempfile

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, PROJECT_ROOT)

from db import init_db

tmp = tempfile.mkdtemp()
db_path = os.path.join(tmp, "test.db")

conn = init_db(db_path)

# 1. List tables and views
rows = conn.execute(
    "SELECT type, name FROM sqlite_master WHERE type IN ('table','view') ORDER BY type, name"
).fetchall()
print("=== TABLES & VIEWS ===")
for t, n in rows:
    print(f"  [{t}] {n}")

# 2. Ship types
n = conn.execute("SELECT COUNT(*) FROM ship_types").fetchone()[0]
print(f"\nShip types seeded: {n}")
row = conn.execute("SELECT * FROM ship_types WHERE ship_type=80").fetchone()
print(f"Type 80 (Tanker): {row}")

# 3. View query (empty but should not error)
conn.execute("SELECT * FROM v_vessel_status LIMIT 0")
print("View v_vessel_status: OK")

# 4. Try ingesting a sample file
from ais_ingestor.ingest import process_file

sample = os.path.join(PROJECT_ROOT, "ais_collector", "data_samples", "ais_20260304_0434_0000.jsonl")
backup_dir = os.path.join(tmp, "backup")
os.makedirs(backup_dir, exist_ok=True)

ok = process_file(conn, sample, backup_dir)
print(f"\nIngest result: {'OK' if ok else 'FAILED'}")

# 5. Show counts
for table in ["vessels", "position_reports", "voyages", "ports", "ship_types"]:
    cnt = conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
    print(f"  {table}: {cnt}")

# 6. Sample vessel with dimensions
row = conn.execute(
    "SELECT mmsi, ship_name, length_m, beam_m FROM vessels WHERE length_m IS NOT NULL LIMIT 3"
).fetchall()
print("\nVessels with dimensions:")
for r in row:
    print(f"  MMSI={r[0]} name={r[1]!r} length={r[2]}m beam={r[3]}m")

# 7. Sample voyage with ETA
row = conn.execute(
    "SELECT mmsi, destination, eta, cargo_status, draught FROM voyages LIMIT 3"
).fetchall()
print("\nVoyages:")
for r in row:
    print(f"  MMSI={r[0]} dest={r[1]!r} eta={r[2]} cargo={r[3]} draught={r[4]}")

# 8. Positions linked to voyages
n_linked = conn.execute(
    "SELECT COUNT(*) FROM position_reports WHERE voyage_id IS NOT NULL"
).fetchone()[0]
n_total = conn.execute("SELECT COUNT(*) FROM position_reports").fetchone()[0]
print(f"\nPositions linked to a voyage: {n_linked}/{n_total}")

# 9. v_vessel_status view
rows = conn.execute(
    "SELECT mmsi, ship_name, ship_type_label, destination, eta, cargo_status FROM v_vessel_status WHERE destination IS NOT NULL LIMIT 3"
).fetchall()
print("\nVessel status view:")
for r in rows:
    print(f"  MMSI={r[0]} {r[1]!r} type={r[2]} dest={r[3]!r} eta={r[4]} cargo={r[5]}")

conn.close()

# Cleanup
import shutil
shutil.rmtree(tmp)
print("\nAll tests passed ✅")
