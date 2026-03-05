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

n_linked = conn.execute("SELECT COUNT(*) FROM position_reports WHERE voyage_id IS NOT NULL").fetchone()[0]
n_total = conn.execute("SELECT COUNT(*) FROM position_reports").fetchone()[0]
print(f"\nPositions linked to a voyage: {n_linked}/{n_total}")

rows = conn.execute("""
    SELECT p.mmsi, p.timestamp_utc, v.destination, v.eta
    FROM position_reports p
    JOIN voyages v ON v.id = p.voyage_id
    LIMIT 5
""").fetchall()
print("Linked positions:")
for r in rows:
    print(f"  MMSI={r[0]} ts={r[1]} dest={r[2]!r} eta={r[3]}")

conn.close()
shutil.rmtree(tmp)
print("\nMulti-file test passed")
