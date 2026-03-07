#!/usr/bin/env python3
"""Quick test: create DB, seed, ingest one sample file."""
import os
import sys
import tempfile
from datetime import datetime, timezone

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, PROJECT_ROOT)

from db import init_db


def _ts(epoch):
    """Format a Unix epoch timestamp for display."""
    if epoch is None:
        return "N/A"
    return datetime.fromtimestamp(epoch, tz=timezone.utc).isoformat()

tmp = tempfile.mkdtemp()
db_path = os.path.join(tmp, "test.db")

conn = init_db(db_path)

# 1. List tables
rows = conn.execute(
    "SELECT type, name FROM sqlite_master WHERE type='table' ORDER BY name"
).fetchall()
print("=== TABLES ===")
for t, n in rows:
    print(f"  [{t}] {n}")

# 2. Ship types
n = conn.execute("SELECT COUNT(*) FROM ship_types").fetchone()[0]
print(f"\nShip types seeded: {n}")
row = conn.execute("SELECT * FROM ship_types WHERE ship_type=80").fetchone()
print(f"Type 80 (Tanker): {row}")

# 3. Try ingesting a sample file
from ais_ingestor.ingest import process_file

sample = os.path.join(PROJECT_ROOT, "ais_collector", "data_samples", "ais_20260304_0434_0000.jsonl")
backup_dir = os.path.join(tmp, "backup")
os.makedirs(backup_dir, exist_ok=True)

ok = process_file(conn, sample, backup_dir)
print(f"\nIngest result: {'OK' if ok else 'FAILED'}")

# 4. Show counts
for table in [
    "vessels", "position_history", "destination_history",
    "draught_history", "navigational_status_history",
    "ship_name_history", "nav_statuses", "ship_types",
]:
    cnt = conn.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
    print(f"  {table}: {cnt}")

# 5. Sample vessel with dimensions
row = conn.execute(
    "SELECT mmsi, ship_name, length_m, beam_m FROM vessels WHERE length_m IS NOT NULL LIMIT 3"
).fetchall()
print("\nVessels with dimensions:")
for r in row:
    print(f"  MMSI={r[0]} name={r[1]!r} length={r[2]}m beam={r[3]}m")

# 6. Sample destinations
row = conn.execute(
    "SELECT mmsi, destination, estimated_arrival, timestamp_utc FROM destination_history LIMIT 3"
).fetchall()
print("\nDestinations:")
for r in row:
    print(f"  MMSI={r[0]} dest={r[1]!r} eta={_ts(r[2])} at={_ts(r[3])}")

# 7. Sample draughts
row = conn.execute(
    "SELECT mmsi, draught, timestamp_utc FROM draught_history LIMIT 3"
).fetchall()
print("\nDraughts:")
for r in row:
    print(f"  MMSI={r[0]} draught={r[1]} at={_ts(r[2])}")

# 8. Nav status history
row = conn.execute(
    "SELECT mmsi, navigational_status, timestamp_utc FROM navigational_status_history LIMIT 3"
).fetchall()
print("\nNav status history:")
for r in row:
    print(f"  MMSI={r[0]} status={r[1]} at={_ts(r[2])}")

# 9. Ship name history
row = conn.execute(
    "SELECT mmsi, ship_name, timestamp_utc FROM ship_name_history LIMIT 3"
).fetchall()
print("\nShip name history:")
for r in row:
    print(f"  MMSI={r[0]} name={r[1]!r} at={_ts(r[2])}")

conn.close()

# Cleanup
import shutil
shutil.rmtree(tmp)
print("\nAll tests passed ✅")
