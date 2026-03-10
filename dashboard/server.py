"""
Ship-tracking dashboard — FastAPI backend.

Reads from the Parquet-based Data Lake (Medallion Architecture) via DuckDB.
All timestamps in the Data Lake are UTC epoch seconds stored as Arrow
Timestamp(Second, "UTC").  DuckDB reads these as TIMESTAMP WITH TIME ZONE;
we use ``epoch(col)::BIGINT`` to convert back to integer seconds.

Endpoints
─────────
GET /                           → serves the SPA (index.html)
GET /api/time-range             → min/max timestamp in positions
GET /api/coverage               → hourly buckets with position counts (for timeline)
GET /api/positions?ts=<epoch>   → vessel positions at a point in time (±30 s)
GET /api/vessel/{mmsi}          → vessel metadata + destination/draught/nav history
GET /api/vessel/{mmsi}/track    → full track for one vessel
GET /api/vessel/{mmsi}/track?start=<epoch>&end=<epoch> → partial track
GET /api/search?q=<text>        → search vessels by name / MMSI
GET /api/latest-positions       → most recent position per vessel
GET /api/ship-categories        → vessel type categories
GET /api/stats                  → quick stats
GET /api/db-metrics             → comprehensive lake metrics
GET /api/ais-coverage           → ingestion health timeline
"""

import os
from contextlib import asynccontextmanager
from pathlib import Path

import duckdb
from fastapi import FastAPI, Query, HTTPException
from fastapi.responses import HTMLResponse
from fastapi.staticfiles import StaticFiles

from dashboard.config import LAKE_ROOT, HOST, PORT

# ── globals ────────────────────────────────────────────────────────────────
_conn: duckdb.DuckDBPyConnection | None = None

STATIC_DIR = Path(__file__).parent / "static"

# Parquet glob patterns (Hive-partitioned facts, flat dim_vessel)
POS_GLOB = f"'{LAKE_ROOT}/bronze/source=ais/fact_position/**/*.parquet'"
DRAUGHT_GLOB = f"'{LAKE_ROOT}/bronze/source=ais/fact_draught/**/*.parquet'"
NAV_GLOB = f"'{LAKE_ROOT}/bronze/source=ais/fact_navigational_status/**/*.parquet'"
DEST_GLOB = f"'{LAKE_ROOT}/bronze/source=ais/fact_destination/**/*.parquet'"
STATIC_GLOB = f"'{LAKE_ROOT}/bronze/source=ais/dim_vessel/vessel_static.parquet'"  # flat file, no hive partitioning
NAME_GLOB = f"'{LAKE_ROOT}/bronze/source=ais/fact_vessel_name_history/**/*.parquet'"
HEALTH_GLOB = f"'{LAKE_ROOT}/bronze/source=ais/metric_system_health/**/*.parquet'"


def _get_conn() -> duckdb.DuckDBPyConnection:
    """Lazy-initialise DuckDB in-memory connection reading Parquet files."""
    global _conn
    if _conn is None:
        _conn = duckdb.connect(database=":memory:")
        _conn.execute("SET timezone='UTC';")
    return _conn


def _q(sql: str, params: list | None = None) -> list[dict]:
    """Run a query and return a list of dicts."""
    conn = _get_conn()
    if params:
        result = conn.execute(sql, params)
    else:
        result = conn.execute(sql)
    columns = [desc[0] for desc in result.description]
    rows = result.fetchall()
    return [dict(zip(columns, row)) for row in rows]


def _q1(sql: str, params: list | None = None):
    """Run a query and return the first row as a dict, or None."""
    rows = _q(sql, params)
    return rows[0] if rows else None


def _has_data(glob_pattern: str) -> bool:
    """Check if any parquet files exist for a glob pattern."""
    import glob as g
    clean = glob_pattern.strip("'")
    return len(g.glob(clean, recursive=True)) > 0


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Open DuckDB on startup, close on shutdown."""
    _get_conn()
    yield
    if _conn:
        _conn.close()


app = FastAPI(title="Barquitos Dashboard", lifespan=lifespan)


# ── NAV STATUS LABELS (AIS standard) ──────────────────────────────────────
NAV_STATUS_LABELS = {
    0: "Under way using engine",
    1: "At anchor",
    2: "Not under command",
    3: "Restricted manoeuvrability",
    4: "Constrained by draught",
    5: "Moored",
    6: "Aground",
    7: "Engaged in fishing",
    8: "Under way sailing",
    9: "Reserved (HSC)",
    10: "Reserved (WIG)",
    11: "Power-driven vessel towing astern",
    12: "Power-driven vessel pushing/towing alongside",
    13: "Reserved",
    14: "AIS-SART, MOB-AIS, EPIRB-AIS",
    15: "Undefined (default)",
}

# ── AIS SHIP TYPE CATEGORIES ─────────────────────────────────────────────
def _ship_type_label(code: int | None) -> str:
    if code is None:
        return "Unknown"
    if 20 <= code <= 29: return "Wing in ground"
    if code == 30: return "Fishing"
    if code == 31: return "Towing"
    if code == 32: return "Towing (large)"
    if code == 33: return "Dredging"
    if code == 34: return "Diving operations"
    if code == 35: return "Military operations"
    if code == 36: return "Sailing"
    if code == 37: return "Pleasure craft"
    if 40 <= code <= 49: return "High speed craft"
    if code == 50: return "Pilot vessel"
    if code == 51: return "Search & rescue"
    if code == 52: return "Tug"
    if code == 53: return "Port tender"
    if code == 54: return "Anti-pollution"
    if code == 55: return "Law enforcement"
    if 56 <= code <= 57: return "Spare (local)"
    if code == 58: return "Medical transport"
    if code == 59: return "Noncombatant ship"
    if 60 <= code <= 69: return "Passenger"
    if 70 <= code <= 79: return "Cargo"
    if 80 <= code <= 89: return "Tanker"
    if 90 <= code <= 99: return "Other"
    return f"Type {code}"

def _ship_type_category(code: int | None) -> str:
    if code is None:
        return "Unknown"
    if 60 <= code <= 69: return "Passenger"
    if 70 <= code <= 79: return "Cargo"
    if 80 <= code <= 89: return "Tanker"
    if code == 30: return "Fishing"
    if code == 36: return "Sailing"
    if code == 37: return "Pleasure"
    if 31 <= code <= 35 or code == 52 or code == 53: return "Tug/Special"
    if 40 <= code <= 49: return "High Speed"
    if 50 <= code <= 59: return "Service"
    if 90 <= code <= 99: return "Other"
    return "Other"


# ── routes ─────────────────────────────────────────────────────────────────

@app.get("/", response_class=HTMLResponse)
async def index():
    return (STATIC_DIR / "index.html").read_text()


@app.get("/api/time-range")
async def time_range():
    """Return the earliest and latest timestamps in positions."""
    if not _has_data(POS_GLOB):
        raise HTTPException(404, "No position data")
    row = _q1(f"""
        SELECT
            epoch(MIN(timestamp_utc))::BIGINT AS ts_min,
            epoch(MAX(timestamp_utc))::BIGINT AS ts_max
        FROM read_parquet({POS_GLOB}, hive_partitioning=true)
    """)
    if row is None or row["ts_min"] is None:
        raise HTTPException(404, "No position data")
    return {"ts_min": int(row["ts_min"]), "ts_max": int(row["ts_max"])}


@app.get("/api/coverage")
async def coverage(
    bucket_seconds: int = Query(3600, description="Bucket size in seconds"),
):
    """Return time buckets with position counts (for the timeline)."""
    if not _has_data(POS_GLOB):
        return []
    rows = _q(f"""
        SELECT
            (epoch(timestamp_utc)::BIGINT / $1) * $1 AS bucket_start,
            COUNT(*) AS cnt
        FROM read_parquet({POS_GLOB}, hive_partitioning=true)
        GROUP BY bucket_start
        ORDER BY bucket_start
    """, [bucket_seconds])
    return rows


@app.get("/api/positions")
async def positions_at(
    ts: int = Query(..., description="Unix epoch seconds"),
    window: int = Query(900, description="Look-back seconds before ts (default 15 min)"),
    ship_type: int | None = Query(None, description="Filter by ship type"),
    categories: str | None = Query(None, description="Comma-separated categories"),
):
    """Return one position per vessel closest to `ts` within [ts-window, ts].

    Uses a look-back window (default 15 min) so vessels whose last report
    was a few minutes before the selected time still appear on the map.
    Only the report closest to `ts` is returned per vessel.
    """
    if not _has_data(POS_GLOB):
        return []

    target_start = ts - window
    target_end = ts

    rows = _q(f"""
        WITH candidates AS (
            SELECT
                p.mmsi,
                epoch(p.timestamp_utc)::BIGINT AS timestamp_utc,
                p.latitude,
                p.longitude,
                p.speed_over_ground,
                p.course_over_ground,
                p.true_heading,
                ABS(epoch(p.timestamp_utc)::BIGINT - $1) AS diff_seconds,
                ROW_NUMBER() OVER (PARTITION BY p.mmsi ORDER BY ABS(epoch(p.timestamp_utc)::BIGINT - $1)) AS rn
            FROM read_parquet({POS_GLOB}, hive_partitioning=true) p
            WHERE epoch(p.timestamp_utc)::BIGINT BETWEEN $2 AND $3
        )
        SELECT
            c.mmsi,
            NULL AS ship_name,
            NULL AS ship_type,
            NULL AS ship_type_label,
            'Unknown' AS category,
            c.timestamp_utc,
            c.latitude,
            c.longitude,
            c.speed_over_ground,
            c.course_over_ground,
            c.true_heading,
            c.diff_seconds
        FROM candidates c
        WHERE c.rn = 1
        ORDER BY random()
        LIMIT 1000
    """, [ts, target_start, target_end])

    # Enrich with vessel static data if available
    if _has_data(STATIC_GLOB):
        rows = _enrich_positions(rows)

    # Apply ship_type filter
    if ship_type is not None:
        rows = [r for r in rows if r.get("ship_type") == ship_type]

    # Apply category filter
    if categories:
        cat_set = {c.strip() for c in categories.split(",") if c.strip()}
        rows = [r for r in rows if r.get("category") in cat_set]

    return rows


def _enrich_positions(positions: list[dict]) -> list[dict]:
    """Enrich position rows with ship_type from vessel static data and name from name history."""
    if not positions:
        return positions
    mmsi_list = [p["mmsi"] for p in positions]
    if not mmsi_list:
        return positions

    # Get latest static info per MMSI
    static_map = {}
    if _has_data(STATIC_GLOB):
        static_rows = _q(f"""
            SELECT mmsi, ship_type, call_sign, imo_number,
                   length_overall, beam
            FROM (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
                FROM read_parquet({STATIC_GLOB})
            )
            WHERE rn = 1
        """)
        for r in static_rows:
            static_map[r["mmsi"]] = r

    # Get latest name per MMSI from name history
    name_map = {}
    if _has_data(NAME_GLOB):
        name_rows = _q(f"""
            SELECT mmsi, ship_name
            FROM (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
                FROM read_parquet({NAME_GLOB}, hive_partitioning=true)
            )
            WHERE rn = 1
        """)
        for r in name_rows:
            name_map[r["mmsi"]] = r["ship_name"]

    for p in positions:
        s = static_map.get(p["mmsi"])
        if s:
            st = s.get("ship_type")
            p["ship_type"] = st
            p["ship_type_label"] = _ship_type_label(st)
            p["category"] = _ship_type_category(st)
        else:
            p["ship_type_label"] = _ship_type_label(p.get("ship_type"))
            p["category"] = _ship_type_category(p.get("ship_type"))
        # Set ship name from name history
        if not p.get("ship_name"):
            p["ship_name"] = name_map.get(p["mmsi"])
    return positions


@app.get("/api/vessel/{mmsi}")
async def vessel_detail(mmsi: int):
    """Vessel metadata + recent destination/draught/nav history."""
    # Get static info
    vessel = None
    if _has_data(STATIC_GLOB):
        vessel = _q1(f"""
            SELECT mmsi, call_sign, imo_number, ship_type,
                   length_overall, beam,
                   epoch(MIN(timestamp_utc))::BIGINT AS first_seen_utc,
                   epoch(MAX(timestamp_utc))::BIGINT AS last_seen_utc
            FROM read_parquet({STATIC_GLOB})
            WHERE mmsi = $1
            GROUP BY mmsi, call_sign, imo_number, ship_type,
                     length_overall, beam
            ORDER BY last_seen_utc DESC
            LIMIT 1
        """, [mmsi])

    if vessel is None:
        # Try to get from position data
        if _has_data(POS_GLOB):
            vessel = _q1(f"""
                SELECT mmsi, NULL AS call_sign, NULL AS imo_number,
                       NULL AS ship_type,
                       NULL AS length_overall, NULL AS beam,
                       epoch(MIN(timestamp_utc))::BIGINT AS first_seen_utc,
                       epoch(MAX(timestamp_utc))::BIGINT AS last_seen_utc
                FROM read_parquet({POS_GLOB}, hive_partitioning=true)
                WHERE mmsi = $1
                GROUP BY mmsi
                ORDER BY last_seen_utc DESC
                LIMIT 1
            """, [mmsi])

    if vessel is None:
        raise HTTPException(404, "Vessel not found")

    # Get latest name from name history
    vessel["ship_name"] = None
    if _has_data(NAME_GLOB):
        name_row = _q1(f"""
            SELECT ship_name
            FROM read_parquet({NAME_GLOB}, hive_partitioning=true)
            WHERE mmsi = $1
            ORDER BY timestamp_utc DESC
            LIMIT 1
        """, [mmsi])
        if name_row:
            vessel["ship_name"] = name_row["ship_name"]

    # Compute dimensions
    st = vessel.get("ship_type")
    vessel["length_m"] = vessel.get("length_overall")
    vessel["beam_m"] = vessel.get("beam")
    vessel["ship_type_label"] = _ship_type_label(st)
    vessel["category"] = _ship_type_category(st)

    # Name history
    name_history = []
    if _has_data(NAME_GLOB):
        name_history = _q(f"""
            SELECT ship_name, epoch(timestamp_utc)::BIGINT AS timestamp_utc
            FROM read_parquet({NAME_GLOB}, hive_partitioning=true)
            WHERE mmsi = $1
            ORDER BY timestamp_utc DESC
            LIMIT 20
        """, [mmsi])

    # Destinations
    dest = []
    if _has_data(DEST_GLOB):
        dest = _q(f"""
            SELECT destination,
                   epoch(eta_utc)::BIGINT AS eta_utc,
                   epoch(timestamp_utc)::BIGINT AS timestamp_utc
            FROM read_parquet({DEST_GLOB}, hive_partitioning=true)
            WHERE mmsi = $1
            ORDER BY timestamp_utc DESC
            LIMIT 10
        """, [mmsi])

    # Nav statuses
    nav = []
    if _has_data(NAV_GLOB):
        nav_raw = _q(f"""
            SELECT navigational_status, epoch(timestamp_utc)::BIGINT AS timestamp_utc
            FROM read_parquet({NAV_GLOB}, hive_partitioning=true)
            WHERE mmsi = $1
            ORDER BY timestamp_utc DESC
            LIMIT 10
        """, [mmsi])
        nav = [
            {
                "navigational_status": r["navigational_status"],
                "label": NAV_STATUS_LABELS.get(r["navigational_status"], "Unknown"),
                "timestamp_utc": r["timestamp_utc"],
            }
            for r in nav_raw
        ]

    # Draughts
    draught = []
    if _has_data(DRAUGHT_GLOB):
        draught = _q(f"""
            SELECT draught, epoch(timestamp_utc)::BIGINT AS timestamp_utc
            FROM read_parquet({DRAUGHT_GLOB}, hive_partitioning=true)
            WHERE mmsi = $1
            ORDER BY timestamp_utc DESC
            LIMIT 10
        """, [mmsi])
        # Round draught to centimeters (2 decimal places)
        for d in draught:
            if d.get("draught") is not None:
                d["draught"] = round(d["draught"], 2)

    # Max draught from draught history (rounded)
    vessel["max_draught"] = None
    if draught:
        max_d = max((d["draught"] for d in draught if d.get("draught") is not None), default=None)
        vessel["max_draught"] = round(max_d, 2) if max_d is not None else None

    # Position count
    pos_count = 0
    if _has_data(POS_GLOB):
        r = _q1(f"""
            SELECT COUNT(*) AS cnt
            FROM read_parquet({POS_GLOB}, hive_partitioning=true)
            WHERE mmsi = $1
        """, [mmsi])
        pos_count = r["cnt"] if r else 0

    return {
        "vessel": vessel,
        "name_history": name_history,
        "destinations": dest,
        "nav_statuses": nav,
        "draughts": draught,
        "position_count": pos_count,
    }


@app.get("/api/vessel/{mmsi}/track")
async def vessel_track(
    mmsi: int,
    start: int | None = Query(None, description="Start epoch"),
    end: int | None = Query(None, description="End epoch"),
):
    """Return the track for a vessel, optionally time-bounded."""
    if not _has_data(POS_GLOB):
        raise HTTPException(404, "No track data for this vessel")

    if start is not None and end is not None:
        rows = _q(f"""
            SELECT epoch(timestamp_utc)::BIGINT AS timestamp_utc,
                   latitude, longitude,
                   speed_over_ground, course_over_ground, true_heading
            FROM read_parquet({POS_GLOB}, hive_partitioning=true)
            WHERE mmsi = $1
              AND epoch(timestamp_utc)::BIGINT BETWEEN $2 AND $3
            ORDER BY timestamp_utc
        """, [mmsi, start, end])
    else:
        rows = _q(f"""
            SELECT epoch(timestamp_utc)::BIGINT AS timestamp_utc,
                   latitude, longitude,
                   speed_over_ground, course_over_ground, true_heading
            FROM read_parquet({POS_GLOB}, hive_partitioning=true)
            WHERE mmsi = $1
            ORDER BY timestamp_utc
        """, [mmsi])

    if not rows:
        raise HTTPException(404, "No track data for this vessel")
    return rows


@app.get("/api/search")
async def search_vessels(q: str = Query(..., min_length=1)):
    """Search vessels by name or MMSI (prefix match)."""
    results = []

    # Search in vessel name history + static data
    if _has_data(NAME_GLOB):
        results = _q(f"""
            WITH latest_names AS (
                SELECT mmsi, ship_name,
                       ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
                FROM read_parquet({NAME_GLOB}, hive_partitioning=true)
                WHERE ship_name ILIKE '%' || $1 || '%'
                   OR CAST(mmsi AS VARCHAR) LIKE '%' || $1 || '%'
            )
            SELECT n.mmsi, n.ship_name,
                   s.call_sign, s.imo_number, s.ship_type
            FROM latest_names n
            LEFT JOIN (
                SELECT mmsi, call_sign, imo_number, ship_type,
                       ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
                FROM read_parquet({STATIC_GLOB})
            ) s ON s.mmsi = n.mmsi AND s.rn = 1
            WHERE n.rn = 1
            ORDER BY n.ship_name
            LIMIT 50
        """, [q])
    elif _has_data(STATIC_GLOB):
        results = _q(f"""
            SELECT mmsi, NULL AS ship_name, call_sign, imo_number, ship_type
            FROM (
                SELECT *, ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
                FROM read_parquet({STATIC_GLOB})
                WHERE CAST(mmsi AS VARCHAR) LIKE '%' || $1 || '%'
            ) sub
            WHERE rn = 1
            LIMIT 50
        """, [q])

    for r in results:
        r["ship_type_label"] = _ship_type_label(r.get("ship_type"))
        r["category"] = _ship_type_category(r.get("ship_type"))
    return results


@app.get("/api/latest-positions")
async def latest_positions(
    ship_types: str | None = Query(None, description="Comma-separated ship type codes"),
    categories: str | None = Query(None, description="Comma-separated categories"),
    min_lat: float | None = Query(None),
    max_lat: float | None = Query(None),
    min_lon: float | None = Query(None),
    max_lon: float | None = Query(None),
    limit: int = Query(1000, ge=1, le=5000),
):
    """Return the most recent known position of each vessel."""
    if not _has_data(POS_GLOB):
        return []

    # Build latest positions
    rows = _q(f"""
        SELECT p.mmsi, NULL AS ship_name,
               epoch(p.timestamp_utc)::BIGINT AS timestamp_utc,
               p.latitude, p.longitude,
               p.speed_over_ground, p.course_over_ground, p.true_heading
        FROM (
            SELECT *, ROW_NUMBER() OVER (PARTITION BY mmsi ORDER BY timestamp_utc DESC) AS rn
            FROM read_parquet({POS_GLOB}, hive_partitioning=true)
        ) p
        WHERE p.rn = 1
        ORDER BY random()
        LIMIT $1
    """, [limit])

    # Enrich with static data
    rows = _enrich_positions(rows)

    # Apply filters in Python (simpler than dynamic SQL with DuckDB globs)
    if ship_types:
        type_set = {int(t.strip()) for t in ship_types.split(",") if t.strip().isdigit()}
        rows = [r for r in rows if r.get("ship_type") in type_set]

    if categories:
        cat_set = {c.strip() for c in categories.split(",") if c.strip()}
        rows = [r for r in rows if r.get("category") in cat_set]

    if all(v is not None for v in [min_lat, max_lat, min_lon, max_lon]):
        rows = [
            r for r in rows
            if min_lat <= r["latitude"] <= max_lat
            and min_lon <= r["longitude"] <= max_lon
        ]

    return rows[:limit]


@app.get("/api/ship-categories")
async def ship_categories():
    """Return all ship type categories with vessel counts."""
    if not _has_data(STATIC_GLOB):
        return []

    rows = _q(f"""
        SELECT ship_type, COUNT(DISTINCT mmsi) AS vessel_count
        FROM read_parquet({STATIC_GLOB})
        WHERE ship_type IS NOT NULL
        GROUP BY ship_type
        ORDER BY vessel_count DESC
    """)
    result = []
    for r in rows:
        st = r["ship_type"]
        result.append({
            "category": _ship_type_category(st),
            "ship_type_code": st,
            "ship_type_label": _ship_type_label(st),
            "vessel_count": r["vessel_count"],
        })
    return result


@app.get("/api/ais-coverage")
async def ais_coverage():
    """Return ingestion health metrics for the coverage timeline."""
    if not _has_data(HEALTH_GLOB):
        return []
    rows = _q(f"""
        SELECT
            epoch(window_start)::BIGINT AS bucket_start,
            total_messages AS message_count
        FROM read_parquet({HEALTH_GLOB}, hive_partitioning=true)
        ORDER BY window_start
    """)
    return rows


@app.get("/api/stats")
async def stats():
    """Quick stats for the dashboard header."""
    total_vessels = 0
    total_positions = 0

    if _has_data(POS_GLOB):
        r = _q1(f"""
            SELECT COUNT(DISTINCT mmsi) AS vessels, COUNT(*) AS positions
            FROM read_parquet({POS_GLOB}, hive_partitioning=true)
        """)
        if r:
            total_vessels = r["vessels"]
            total_positions = r["positions"]

    by_type = []
    if _has_data(STATIC_GLOB):
        raw = _q(f"""
            SELECT ship_type, COUNT(DISTINCT mmsi) AS vessel_count
            FROM read_parquet({STATIC_GLOB})
            WHERE ship_type IS NOT NULL
            GROUP BY ship_type
            ORDER BY vessel_count DESC
            LIMIT 20
        """)
        by_type = [
            {
                "ship_type_label": _ship_type_label(r["ship_type"]),
                "category": _ship_type_category(r["ship_type"]),
                "ship_type_code": r["ship_type"],
                "vessel_count": r["vessel_count"],
            }
            for r in raw
        ]

    return {
        "total_vessels": total_vessels,
        "total_positions": total_positions,
        "vessels_by_type": by_type,
    }


@app.get("/api/db-metrics")
async def db_metrics():
    """Comprehensive Data Lake metrics."""
    total_vessels = 0
    total_positions = 0

    if _has_data(POS_GLOB):
        r = _q1(f"""
            SELECT COUNT(DISTINCT mmsi) AS vessels, COUNT(*) AS positions
            FROM read_parquet({POS_GLOB}, hive_partitioning=true)
        """)
        if r:
            total_vessels = r["vessels"]
            total_positions = r["positions"]

    total_destinations = 0
    if _has_data(DEST_GLOB):
        r = _q1(f"SELECT COUNT(*) AS cnt FROM read_parquet({DEST_GLOB}, hive_partitioning=true)")
        total_destinations = r["cnt"] if r else 0

    total_draughts = 0
    if _has_data(DRAUGHT_GLOB):
        r = _q1(f"SELECT COUNT(*) AS cnt FROM read_parquet({DRAUGHT_GLOB}, hive_partitioning=true)")
        total_draughts = r["cnt"] if r else 0

    total_nav_statuses = 0
    if _has_data(NAV_GLOB):
        r = _q1(f"SELECT COUNT(*) AS cnt FROM read_parquet({NAV_GLOB}, hive_partitioning=true)")
        total_nav_statuses = r["cnt"] if r else 0

    total_static = 0
    if _has_data(STATIC_GLOB):
        r = _q1(f"SELECT COUNT(*) AS cnt FROM read_parquet({STATIC_GLOB})")
        total_static = r["cnt"] if r else 0

    total_name_changes = 0
    if _has_data(NAME_GLOB):
        r = _q1(f"SELECT COUNT(*) AS cnt FROM read_parquet({NAME_GLOB}, hive_partitioning=true)")
        total_name_changes = r["cnt"] if r else 0

    # Vessels by type
    vessels_by_type = []
    if _has_data(STATIC_GLOB):
        raw = _q(f"""
            SELECT ship_type, COUNT(DISTINCT mmsi) AS vessel_count
            FROM read_parquet({STATIC_GLOB})
            WHERE ship_type IS NOT NULL
            GROUP BY ship_type
            ORDER BY vessel_count DESC
        """)
        vessels_by_type = [
            {
                "ship_type_label": _ship_type_label(r["ship_type"]),
                "category": _ship_type_category(r["ship_type"]),
                "ship_type_code": r["ship_type"],
                "vessel_count": r["vessel_count"],
            }
            for r in raw
        ]

    # Vessels by category
    category_counts: dict[str, int] = {}
    for vt in vessels_by_type:
        cat = vt["category"]
        category_counts[cat] = category_counts.get(cat, 0) + vt["vessel_count"]
    vessels_by_category = [
        {"category": cat, "vessel_count": cnt}
        for cat, cnt in sorted(category_counts.items(), key=lambda x: -x[1])
    ]

    # Time range
    time_range_val = None
    if _has_data(POS_GLOB):
        tr = _q1(f"""
            SELECT epoch(MIN(timestamp_utc))::BIGINT AS ts_min,
                   epoch(MAX(timestamp_utc))::BIGINT AS ts_max
            FROM read_parquet({POS_GLOB}, hive_partitioning=true)
        """)
        if tr and tr["ts_min"]:
            time_range_val = {"ts_min": int(tr["ts_min"]), "ts_max": int(tr["ts_max"])}

    # Positions per day
    positions_per_day = []
    if _has_data(POS_GLOB):
        positions_per_day = _q(f"""
            SELECT CAST(timestamp_utc AS DATE) AS day, COUNT(*) AS cnt
            FROM read_parquet({POS_GLOB}, hive_partitioning=true)
            GROUP BY CAST(timestamp_utc AS DATE)
            ORDER BY day DESC
            LIMIT 30
        """)
        # Convert date to string
        for r in positions_per_day:
            r["day"] = str(r["day"])

    # Data Lake size on disk
    import glob as g
    db_size_bytes = 0
    for pf in g.glob(f"{LAKE_ROOT}/**/*.parquet", recursive=True):
        try:
            db_size_bytes += os.path.getsize(pf)
        except OSError:
            pass

    # Table sizes
    table_sizes = [
        {"table": "fact_position", "row_count": total_positions},
        {"table": "dim_vessel", "row_count": total_static},
        {"table": "fact_vessel_name_history", "row_count": total_name_changes},
        {"table": "fact_destination", "row_count": total_destinations},
        {"table": "fact_draught", "row_count": total_draughts},
        {"table": "fact_navigational_status", "row_count": total_nav_statuses},
    ]

    return {
        "total_vessels": total_vessels,
        "total_positions": total_positions,
        "total_destinations": total_destinations,
        "total_draughts": total_draughts,
        "total_nav_statuses": total_nav_statuses,
        "total_name_changes": total_name_changes,
        "vessels_by_type": vessels_by_type,
        "vessels_by_category": vessels_by_category,
        "time_range": time_range_val,
        "positions_per_day": positions_per_day,
        "db_size_bytes": db_size_bytes,
        "table_sizes": table_sizes,
    }


# ── Serve built Vite assets (JS, CSS) — must be BEFORE the SPA catch-all ───
app.mount("/assets", StaticFiles(directory=str(STATIC_DIR / "assets")), name="assets")


# ── SPA catch-all: serve index.html for all non-API paths (client-side routing) ──
@app.get("/{path:path}")
async def spa_catch_all(path: str):
    """Serve index.html for any non-API path to support client-side routing."""
    if path.startswith("api/"):
        raise HTTPException(404)
    html_file = STATIC_DIR / "index.html"
    if html_file.exists():
        return HTMLResponse(html_file.read_text())
    raise HTTPException(404)


# ── main ───────────────────────────────────────────────────────────────────
if __name__ == "__main__":
    import uvicorn
    uvicorn.run("dashboard.server:app", host=HOST, port=PORT, reload=True)
