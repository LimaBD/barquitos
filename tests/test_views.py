#!/usr/bin/env python3
"""Test the database views and parametric query."""
import os
import sys
import tempfile
import shutil

PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, PROJECT_ROOT)

from db import init_db, VESSEL_POSITIONS_AT_SQL
from ais_ingestor.ingest import process_file

tmp = tempfile.mkdtemp()
db_path = os.path.join(tmp, "test.db")

conn = init_db(db_path)

# Ingest a sample file so the views have data to work with
sample = os.path.join(PROJECT_ROOT, "ais_collector", "data_samples", "ais_20260304_0434_0000.jsonl")
backup_dir = os.path.join(tmp, "backup")
os.makedirs(backup_dir, exist_ok=True)
process_file(conn, sample, backup_dir)

# ------------------------------------------------------------------
# 1. Verify views exist
# ------------------------------------------------------------------
views = [r[0] for r in conn.execute(
    "SELECT name FROM sqlite_master WHERE type='view' ORDER BY name"
).fetchall()]
print("=== VIEWS ===")
for v in views:
    print(f"  {v}")

expected_views = {"v_total_vessels", "v_vessels_by_type", "v_total_positions",
                  "v_latest_positions", "v_vessel_track"}
assert expected_views.issubset(set(views)), f"Missing views: {expected_views - set(views)}"
print("  ✅ All expected views exist")

# ------------------------------------------------------------------
# 2. v_total_vessels
# ------------------------------------------------------------------
total = conn.execute("SELECT total_vessels FROM v_total_vessels").fetchone()[0]
print(f"\nv_total_vessels → {total}")
assert total > 0, "Expected at least one vessel"
print("  ✅ OK")

# ------------------------------------------------------------------
# 3. v_vessels_by_type
# ------------------------------------------------------------------
rows = conn.execute("SELECT * FROM v_vessels_by_type").fetchall()
total_by_type = sum(r[3] for r in rows)  # vessel_count column
print(f"\nv_vessels_by_type → {len(rows)} type groups, {total_by_type} total")
assert total_by_type == total, "Sum by type should equal total vessels"
for r in rows[:5]:
    print(f"  {r[0]:30s} ({r[1]:12s})  code={r[2]}  count={r[3]}")
print("  ✅ OK")

# ------------------------------------------------------------------
# 4. v_total_positions
# ------------------------------------------------------------------
total_pos = conn.execute("SELECT total_positions FROM v_total_positions").fetchone()[0]
print(f"\nv_total_positions → {total_pos}")
assert total_pos > 0, "Expected at least one position"
print("  ✅ OK")

# ------------------------------------------------------------------
# 5. v_latest_positions
# ------------------------------------------------------------------
latest = conn.execute(
    "SELECT mmsi, ship_name, ship_type_label, latitude, longitude FROM v_latest_positions LIMIT 5"
).fetchall()
print(f"\nv_latest_positions (first 5 of {conn.execute('SELECT COUNT(*) FROM v_latest_positions').fetchone()[0]}):")
for r in latest:
    print(f"  MMSI={r[0]}  name={r[1]!r:25s}  type={r[2]:20s}  lat={r[3]:.4f}  lon={r[4]:.4f}")
assert len(latest) > 0, "Expected at least one latest position"
print("  ✅ OK")

# ------------------------------------------------------------------
# 6. v_vessel_track  (by mmsi)
# ------------------------------------------------------------------
sample_mmsi = conn.execute("SELECT mmsi FROM position_history LIMIT 1").fetchone()[0]
track = conn.execute(
    "SELECT timestamp_utc, latitude, longitude FROM v_vessel_track WHERE mmsi = ?",
    (sample_mmsi,)
).fetchall()
print(f"\nv_vessel_track (MMSI={sample_mmsi}) → {len(track)} positions")
for r in track[:3]:
    print(f"  {r[0]}  lat={r[1]:.4f}  lon={r[2]:.4f}")
assert len(track) > 0, "Expected at least one position for vessel"
print("  ✅ OK")

# ------------------------------------------------------------------
# 6b. v_vessel_track  (by imo_number, if available)
# ------------------------------------------------------------------
imo_row = conn.execute(
    "SELECT imo_number FROM vessels WHERE imo_number IS NOT NULL AND imo_number != 0 LIMIT 1"
).fetchone()
if imo_row:
    imo = imo_row[0]
    track_imo = conn.execute(
        "SELECT mmsi, timestamp_utc, latitude, longitude FROM v_vessel_track WHERE imo_number = ? LIMIT 3",
        (imo,)
    ).fetchall()
    print(f"\nv_vessel_track (IMO={imo}) → {len(track_imo)} positions (showing ≤3)")
    for r in track_imo:
        print(f"  MMSI={r[0]}  {r[1]}  lat={r[2]:.4f}  lon={r[3]:.4f}")
    print("  ✅ OK")
else:
    print("\n  ⚠️  No vessel with IMO number found; skipping IMO filter test")

# ------------------------------------------------------------------
# 7. VESSEL_POSITIONS_AT_SQL  (parametric – all types)
# ------------------------------------------------------------------
ts = conn.execute(
    "SELECT timestamp_utc FROM position_history ORDER BY timestamp_utc LIMIT 1"
).fetchone()[0]
rows = conn.execute(VESSEL_POSITIONS_AT_SQL, {"target_ts": ts, "ship_type": None}).fetchall()
print(f"\nVESSEL_POSITIONS_AT_SQL  target={ts}")
print(f"  All types → {len(rows)} vessels")
assert len(rows) > 0, "Expected at least one vessel within ±5s of a known position timestamp"
r = rows[0]
print(f"  First: MMSI={r[0]}  name={r[1]!r}  lat={r[5]:.4f}  lon={r[6]:.4f}  diff={r[10]}s")
# All diffs should be ≤5 seconds
assert all(r[10] <= 5 for r in rows), "All diffs should be ≤5 seconds"
print("  ✅ OK")

# ------------------------------------------------------------------
# 8. VESSEL_POSITIONS_AT_SQL  (parametric – filter by ship type)
# ------------------------------------------------------------------
rows_tanker = conn.execute(VESSEL_POSITIONS_AT_SQL, {"target_ts": ts, "ship_type": 80}).fetchall()
print(f"  Tankers (type=80) → {len(rows_tanker)} vessels")
# All returned vessels should be type 80
for r in rows_tanker:
    assert r[2] == 80, f"Expected ship_type=80, got {r[2]}"
print("  ✅ OK")

# ------------------------------------------------------------------
# 9. VESSEL_POSITIONS_AT_SQL  (no data within window → empty)
# ------------------------------------------------------------------
rows_empty = conn.execute(
    VESSEL_POSITIONS_AT_SQL, {"target_ts": "1999-01-01T00:00:00Z", "ship_type": None}
).fetchall()
print(f"  Far-away timestamp → {len(rows_empty)} vessels")
assert len(rows_empty) == 0, "Expected no results for distant timestamp"
print("  ✅ OK")

# ------------------------------------------------------------------
conn.close()
shutil.rmtree(tmp)
print("\n🎉 All view tests passed ✅")
