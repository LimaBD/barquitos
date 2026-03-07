#!/usr/bin/env python3
import os, sys, tempfile, shutil
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, PROJECT_ROOT)
from db import init_db
from ais_ingestor.ingest import process_file

tmp = tempfile.mkdtemp()
db_path = os.path.join(tmp, "test.db")
backup_dir = os.path.join(tmp, "backup")
os.makedirs(backup_dir, exist_ok=True)
conn = init_db(db_path)

data_dir = os.path.join(PROJECT_ROOT, "ais_collector", "data_samples")
samples = sorted(os.listdir(data_dir))[:3]
for f in samples:
    fp = os.path.join(data_dir, f)
    process_file(conn, fp, backup_dir)

n_total = conn.execute("SELECT COUNT(*) FROM position_history").fetchone()[0]
n_dest = conn.execute("SELECT COUNT(*) FROM destination_history").fetchone()[0]
n_nav = conn.execute("SELECT COUNT(*) FROM navigational_status_history").fetchone()[0]
print(f"\nPositions: {n_total}  Destinations: {n_dest}  Nav statuses: {n_nav}")

rows = conn.execute("""
    SELECT p.mmsi, p.timestamp_utc, d.destination, d.estimated_arrival
    FROM position_history p
    JOIN destination_history d ON d.mmsi = p.mmsi
    GROUP BY p.mmsi
    LIMIT 5
""").fetchall()
print("Positions with destination (via mmsi join):")
for r in rows:
    print(f"  MMSI={r[0]} ts={r[1]} dest={r[2]!r} eta={r[3]}")

conn.close()
shutil.rmtree(tmp)
print("\nMulti-file test passed")
