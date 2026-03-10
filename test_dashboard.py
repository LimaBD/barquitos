#!/usr/bin/env python3
"""
Smoke-test every dashboard API endpoint against a synthetic test data lake.

Creates Parquet fixtures that exactly match the Arrow schemas produced by
ais_ingestor_rs (Timestamp(Second, "UTC"), UInt32 mmsi, etc.), then calls
each endpoint function directly (no HTTP needed).
"""

import os, sys, shutil, asyncio, traceback

# ── Test data lake location ───────────────────────────────────────────────
LAKE = "/tmp/test_trading_lake"
os.environ["LAKE_ROOT"] = LAKE

# ── Build fixtures BEFORE importing dashboard (which reads LAKE_ROOT) ─────
import pyarrow as pa
import pyarrow.parquet as pq

TS = pa.timestamp("s", tz="UTC")

# Two test vessels
MMSI_A = 211000001  # cargo
MMSI_B = 244000002  # tanker

T0 = 1741100000  # 2025-03-04 ~14:53 UTC
T1 = T0 + 60
T2 = T0 + 120


def _clean():
    if os.path.exists(LAKE):
        shutil.rmtree(LAKE)


def _mkdir(rel: str):
    p = os.path.join(LAKE, rel)
    os.makedirs(p, exist_ok=True)
    return p


def _write_fixtures():
    _clean()

    # ── fact_position (Hive: year/month/day) ──────────────────────────
    pos_dir = _mkdir("bronze/source=ais/fact_position/year=2025/month=03/day=04")
    schema = pa.schema([
        pa.field("mmsi",                pa.uint32(),  nullable=False),
        pa.field("timestamp_utc",       TS,           nullable=False),
        pa.field("latitude",            pa.float64(), nullable=False),
        pa.field("longitude",           pa.float64(), nullable=False),
        pa.field("speed_over_ground",   pa.float32(), nullable=True),
        pa.field("course_over_ground",  pa.float32(), nullable=True),
        pa.field("true_heading",        pa.uint16(),  nullable=True),
        pa.field("rate_of_turn",        pa.int16(),   nullable=True),
    ])
    tbl = pa.table({
        "mmsi":               pa.array([MMSI_A, MMSI_A, MMSI_B], type=pa.uint32()),
        "timestamp_utc":      pa.array([T0, T1, T2],             type=TS),
        "latitude":           pa.array([51.50000, 51.50100, 36.72000]),
        "longitude":          pa.array([-0.10000, -0.09900,  -4.42000]),
        "speed_over_ground":  pa.array([12.3, 12.5, 8.1],       type=pa.float32()),
        "course_over_ground": pa.array([90.0, 91.0, 180.0],     type=pa.float32()),
        "true_heading":       pa.array([89, 90, 179],            type=pa.uint16()),
        "rate_of_turn":       pa.array([0, 1, -2],               type=pa.int16()),
    }, schema=schema)
    pq.write_table(tbl, os.path.join(pos_dir, "1453_to_1508.parquet"))

    # ── dim_vessel (flat file) ────────────────────────────────────────
    vs_dir = _mkdir("bronze/source=ais/dim_vessel")
    schema = pa.schema([
        pa.field("mmsi",            pa.uint32(),  nullable=False),
        pa.field("timestamp_utc",   TS,           nullable=False),
        pa.field("call_sign",       pa.utf8(),    nullable=True),
        pa.field("imo_number",      pa.uint32(),  nullable=True),
        pa.field("ship_type",       pa.uint8(),   nullable=True),
        pa.field("length_overall",  pa.uint16(),  nullable=True),
        pa.field("beam",            pa.uint16(),  nullable=True),
    ])
    tbl = pa.table({
        "mmsi":           pa.array([MMSI_A, MMSI_B],            type=pa.uint32()),
        "timestamp_utc":  pa.array([T0, T0],                    type=TS),
        "call_sign":      ["DABC", "PBXY"],
        "imo_number":     pa.array([9800001, 9800002],          type=pa.uint32()),
        "ship_type":      pa.array([70, 80],                    type=pa.uint8()),
        "length_overall": pa.array([200, 300],                  type=pa.uint16()),
        "beam":           pa.array([30, 50],                    type=pa.uint16()),
    }, schema=schema)
    pq.write_table(tbl, os.path.join(vs_dir, "vessel_static.parquet"))

    # ── fact_vessel_name_history (Hive: year/month) ───────────────────
    nh_dir = _mkdir("bronze/source=ais/fact_vessel_name_history/year=2025/month=03")
    schema = pa.schema([
        pa.field("mmsi",          pa.uint32(), nullable=False),
        pa.field("timestamp_utc", TS,          nullable=False),
        pa.field("ship_name",     pa.utf8(),   nullable=False),
    ])
    tbl = pa.table({
        "mmsi":          pa.array([MMSI_A, MMSI_B],  type=pa.uint32()),
        "timestamp_utc": pa.array([T0, T0],          type=TS),
        "ship_name":     ["EVER GIVEN", "KNOCK NEVIS"],
    }, schema=schema)
    pq.write_table(tbl, os.path.join(nh_dir, "name_history.parquet"))

    # ── fact_draught (Hive: year/month) ───────────────────────────────
    dr_dir = _mkdir("bronze/source=ais/fact_draught/year=2025/month=03")
    schema = pa.schema([
        pa.field("mmsi",          pa.uint32(),  nullable=False),
        pa.field("timestamp_utc", TS,           nullable=False),
        pa.field("draught",       pa.float32(), nullable=False),
    ])
    tbl = pa.table({
        "mmsi":          pa.array([MMSI_A, MMSI_B], type=pa.uint32()),
        "timestamp_utc": pa.array([T0, T0],         type=TS),
        "draught":       pa.array([10.5, 22.3],     type=pa.float32()),
    }, schema=schema)
    pq.write_table(tbl, os.path.join(dr_dir, "draught_changes.parquet"))

    # ── fact_navigational_status (Hive: year/month) ───────────────────
    ns_dir = _mkdir("bronze/source=ais/fact_navigational_status/year=2025/month=03")
    schema = pa.schema([
        pa.field("mmsi",                 pa.uint32(), nullable=False),
        pa.field("timestamp_utc",        TS,          nullable=False),
        pa.field("navigational_status",  pa.uint8(),  nullable=False),
    ])
    tbl = pa.table({
        "mmsi":                pa.array([MMSI_A, MMSI_B], type=pa.uint32()),
        "timestamp_utc":       pa.array([T0, T0],         type=TS),
        "navigational_status": pa.array([0, 5],           type=pa.uint8()),
    }, schema=schema)
    pq.write_table(tbl, os.path.join(ns_dir, "status_changes.parquet"))

    # ── fact_destination (Hive: year/month) ───────────────────────────
    de_dir = _mkdir("bronze/source=ais/fact_destination/year=2025/month=03")
    schema = pa.schema([
        pa.field("mmsi",          pa.uint32(), nullable=False),
        pa.field("timestamp_utc", TS,          nullable=False),
        pa.field("destination",   pa.utf8(),   nullable=False),
        pa.field("eta_utc",       TS,          nullable=True),
    ])
    tbl = pa.table({
        "mmsi":          pa.array([MMSI_A, MMSI_B],   type=pa.uint32()),
        "timestamp_utc": pa.array([T0, T0],            type=TS),
        "destination":   ["ROTTERDAM", "SINGAPORE"],
        "eta_utc":       pa.array([T0 + 86400, None],  type=TS),
    }, schema=schema)
    pq.write_table(tbl, os.path.join(de_dir, "destination_changes.parquet"))

    # ── metric_system_health (Hive: year/month) ──────────────────────
    he_dir = _mkdir("bronze/source=ais/metric_system_health/year=2025/month=03")
    schema = pa.schema([
        pa.field("window_start",         TS,          nullable=False),
        pa.field("window_end",           TS,          nullable=False),
        pa.field("position_reports",     pa.uint32(), nullable=False),
        pa.field("static_data_reports",  pa.uint32(), nullable=False),
        pa.field("total_messages",       pa.uint32(), nullable=False),
        pa.field("unique_mmsi_count",    pa.uint32(), nullable=False),
        pa.field("parse_errors",         pa.uint32(), nullable=False),
    ])
    tbl = pa.table({
        "window_start":        pa.array([T0],       type=TS),
        "window_end":          pa.array([T0 + 900], type=TS),
        "position_reports":    pa.array([100],       type=pa.uint32()),
        "static_data_reports": pa.array([10],        type=pa.uint32()),
        "total_messages":      pa.array([110],       type=pa.uint32()),
        "unique_mmsi_count":   pa.array([50],        type=pa.uint32()),
        "parse_errors":        pa.array([2],         type=pa.uint32()),
    }, schema=schema)
    pq.write_table(tbl, os.path.join(he_dir, "health_metrics.parquet"))


# ── Write fixtures, then import dashboard ─────────────────────────────────
_write_fixtures()

from dashboard.config import LAKE_ROOT
from dashboard import server as srv

print(f"LAKE_ROOT  = {LAKE_ROOT}")
print(f"POS_GLOB   = {srv.POS_GLOB}")
print(f"positions  : {srv._has_data(srv.POS_GLOB)}")
print(f"draught    : {srv._has_data(srv.DRAUGHT_GLOB)}")
print(f"nav        : {srv._has_data(srv.NAV_GLOB)}")
print(f"dest       : {srv._has_data(srv.DEST_GLOB)}")
print(f"static     : {srv._has_data(srv.STATIC_GLOB)}")
print(f"names      : {srv._has_data(srv.NAME_GLOB)}")
print(f"health     : {srv._has_data(srv.HEALTH_GLOB)}")
print()

# ── Test runner ───────────────────────────────────────────────────────────
passed = 0
failed = 0


async def test(name, coro):
    global passed, failed
    try:
        result = await coro
        if isinstance(result, dict):
            summary = {k: type(v).__name__ for k, v in result.items()}
        elif isinstance(result, list):
            summary = f"list[{len(result)} items]"
        else:
            summary = repr(result)[:120]
        print(f"  ✓ {name}: {summary}")
        passed += 1
        return result
    except Exception as e:
        print(f"  ✗ {name}: {e}")
        traceback.print_exc()
        failed += 1
        return None


async def main():
    global passed, failed

    print("=" * 60)
    print("Testing dashboard endpoints")
    print("=" * 60)

    # 1. time-range
    tr = await test("time-range", srv.time_range())
    assert tr is not None, "time-range must succeed"
    assert tr["ts_min"] == T0, f"ts_min={tr['ts_min']} expected {T0}"
    assert tr["ts_max"] == T2, f"ts_max={tr['ts_max']} expected {T2}"

    # 2. coverage
    cov = await test("coverage", srv.coverage(bucket_seconds=3600))
    assert cov and len(cov) > 0, "coverage should have buckets"

    # 3. stats
    st = await test("stats", srv.stats())
    assert st["total_vessels"] == 2
    assert st["total_positions"] == 3

    # 4. db-metrics
    dm = await test("db-metrics", srv.db_metrics())
    assert dm["total_vessels"] == 2
    assert dm["total_positions"] == 3
    assert dm["total_destinations"] == 2
    assert dm["total_draughts"] == 2
    assert dm["total_nav_statuses"] == 2
    assert dm["total_name_changes"] == 2

    # 5. ship-categories
    cats = await test("ship-categories", srv.ship_categories())
    assert len(cats) == 2  # type 70 (Cargo) + type 80 (Tanker)

    # 6. ais-coverage
    ac = await test("ais-coverage", srv.ais_coverage())
    assert len(ac) == 1
    assert ac[0]["message_count"] == 110

    # 7. positions at T1 (window covers T0–T1)
    pos = await test(
        f"positions(ts={T1})",
        srv.positions_at(ts=T1, window=300, ship_type=None, categories=None),
    )
    assert pos is not None and len(pos) >= 1, "should find positions"
    mmsi_set = {p["mmsi"] for p in pos}
    assert MMSI_A in mmsi_set, "vessel A should appear"

    # 8. vessel detail
    vd = await test(f"vessel({MMSI_A})", srv.vessel_detail(MMSI_A))
    assert vd["vessel"]["mmsi"] == MMSI_A
    assert vd["vessel"]["ship_name"] == "EVER GIVEN"
    assert vd["vessel"]["ship_type"] == 70
    assert vd["vessel"]["category"] == "Cargo"
    assert len(vd["destinations"]) == 1
    assert vd["destinations"][0]["destination"] == "ROTTERDAM"
    assert len(vd["nav_statuses"]) == 1
    assert vd["nav_statuses"][0]["navigational_status"] == 0
    assert len(vd["draughts"]) == 1
    assert vd["position_count"] == 2  # two positions for MMSI_A

    # 9. vessel track
    tk = await test(
        f"vessel/{MMSI_A}/track",
        srv.vessel_track(MMSI_A, start=None, end=None),
    )
    assert len(tk) == 2, f"track should have 2 points, got {len(tk)}"

    # 10. vessel track (bounded)
    tk2 = await test(
        f"vessel/{MMSI_A}/track?start,end",
        srv.vessel_track(MMSI_A, start=T0, end=T0),
    )
    assert len(tk2) == 1, f"bounded track should have 1 point, got {len(tk2)}"

    # 11. latest-positions
    lp = await test(
        "latest-positions",
        srv.latest_positions(
            ship_types=None, categories=None,
            min_lat=None, max_lat=None, min_lon=None, max_lon=None,
            limit=10,
        ),
    )
    assert len(lp) == 2, f"latest-positions should have 2 vessels, got {len(lp)}"

    # 12. latest-positions with category filter
    lp2 = await test(
        "latest-positions(Cargo)",
        srv.latest_positions(
            ship_types=None, categories="Cargo",
            min_lat=None, max_lat=None, min_lon=None, max_lon=None,
            limit=10,
        ),
    )
    assert all(p["category"] == "Cargo" for p in lp2)

    # 13. latest-positions with bbox filter
    lp3 = await test(
        "latest-positions(bbox)",
        srv.latest_positions(
            ship_types=None, categories=None,
            min_lat=50.0, max_lat=53.0, min_lon=-1.0, max_lon=1.0,
            limit=10,
        ),
    )
    assert all(50.0 <= p["latitude"] <= 53.0 for p in lp3)

    # 14. search by name
    sr = await test("search(EVER)", srv.search_vessels(q="EVER"))
    assert len(sr) >= 1
    assert sr[0]["ship_name"] == "EVER GIVEN"

    # 15. search by MMSI substring
    sr2 = await test(f"search({str(MMSI_B)[:5]})", srv.search_vessels(q=str(MMSI_B)[:5]))
    assert any(r["mmsi"] == MMSI_B for r in sr2)

    print()
    print("=" * 60)
    print(f"Results: {passed} passed, {failed} failed")
    print("=" * 60)

    if failed > 0:
        sys.exit(1)


asyncio.run(main())
