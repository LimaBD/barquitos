"""
Ship-tracking dashboard — FastAPI backend.

Endpoints
─────────
GET /                           → serves the SPA (index.html)
GET /api/time-range             → min/max timestamp in position_history
GET /api/coverage               → hourly buckets with position counts (for timeline)
GET /api/positions?ts=<epoch>   → vessel positions at a point in time (±30 s)
GET /api/vessel/{mmsi}          → vessel metadata + destination/draught/nav history
GET /api/vessel/{mmsi}/track    → full track for one vessel
GET /api/vessel/{mmsi}/track?start=<epoch>&end=<epoch> → partial track
GET /api/search?q=<text>        → search vessels by name / MMSI
"""

import os
import sqlite3
import sys
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI, Query, HTTPException
from fastapi.responses import HTMLResponse, FileResponse
from fastapi.staticfiles import StaticFiles

# Allow importing the shared `db` package from the repo root
_project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, _project_root)

from db.connection import get_connection
from dashboard.config import DATABASE_PATH, HOST, PORT

# ── globals ────────────────────────────────────────────────────────────────
_conn: sqlite3.Connection | None = None

STATIC_DIR = Path(__file__).parent / "static"


def _get_conn() -> sqlite3.Connection:
    global _conn
    if _conn is None:
        _conn = get_connection(DATABASE_PATH)
        _conn.row_factory = sqlite3.Row
    return _conn


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Open the DB on startup, close on shutdown."""
    _get_conn()
    yield
    if _conn:
        _conn.close()


app = FastAPI(title="Barquitos Dashboard", lifespan=lifespan)


# ── helpers ────────────────────────────────────────────────────────────────
def _rows_to_dicts(cursor) -> list[dict]:
    return [dict(r) for r in cursor.fetchall()]


# ── routes ─────────────────────────────────────────────────────────────────

@app.get("/", response_class=HTMLResponse)
async def index():
    return (STATIC_DIR / "index.html").read_text()


@app.get("/api/time-range")
async def time_range():
    """Return the earliest and latest timestamps in position_history."""
    conn = _get_conn()
    row = conn.execute(
        "SELECT MIN(timestamp_utc) AS ts_min, MAX(timestamp_utc) AS ts_max "
        "FROM position_history"
    ).fetchone()
    if row is None or row["ts_min"] is None:
        raise HTTPException(404, "No position data")
    return {"ts_min": row["ts_min"], "ts_max": row["ts_max"]}


@app.get("/api/coverage")
async def coverage(
    bucket_seconds: int = Query(3600, description="Bucket size in seconds"),
):
    """
    Return time buckets with the count of position reports in each bucket.
    This powers the Dahua-style green/grey timeline bar.
    """
    conn = _get_conn()
    rows = _rows_to_dicts(conn.execute(
        """
        SELECT
            (timestamp_utc / :b) * :b AS bucket_start,
            COUNT(*)                   AS cnt
        FROM position_history
        GROUP BY bucket_start
        ORDER BY bucket_start
        """,
        {"b": bucket_seconds},
    ))
    return rows


@app.get("/api/positions")
async def positions_at(
    ts: int = Query(..., description="Unix epoch seconds"),
    window: int = Query(30, description="± seconds around ts"),
    ship_type: int | None = Query(None, description="Filter by ship type"),
):
    """
    Return one position per vessel closest to `ts` within ±window seconds.
    """
    conn = _get_conn()
    rows = _rows_to_dicts(conn.execute(
        """
        WITH candidates AS (
            SELECT
                ph.mmsi,
                ph.timestamp_utc,
                ph.latitude,
                ph.longitude,
                ph.speed_over_ground,
                ph.course_over_ground,
                ph.true_heading,
                ABS(ph.timestamp_utc - :target_ts) AS diff_seconds,
                ROW_NUMBER() OVER (
                    PARTITION BY ph.mmsi
                    ORDER BY ABS(ph.timestamp_utc - :target_ts)
                ) AS rn
            FROM position_history ph
            WHERE ph.timestamp_utc BETWEEN (:target_ts - :win) AND (:target_ts + :win)
        )
        SELECT
            c.mmsi,
            v.ship_name,
            v.ship_type,
            COALESCE(st.label, 'Unknown') AS ship_type_label,
            COALESCE(st.category, 'Unknown') AS category,
            c.timestamp_utc,
            c.latitude,
            c.longitude,
            c.speed_over_ground,
            c.course_over_ground,
            c.true_heading,
            c.diff_seconds
        FROM candidates c
        JOIN vessels v ON c.mmsi = v.mmsi
        LEFT JOIN ship_types st ON v.ship_type = st.ship_type
        WHERE c.rn = 1
          AND (:ship_type IS NULL OR v.ship_type = :ship_type)
        ORDER BY c.mmsi
        """,
        {"target_ts": ts, "win": window, "ship_type": ship_type},
    ))
    return rows


@app.get("/api/vessel/{mmsi}")
async def vessel_detail(mmsi: int):
    """Full metadata for a single vessel, plus recent destination and nav status."""
    conn = _get_conn()
    v = conn.execute(
        """
        SELECT v.*, COALESCE(st.label, 'Unknown') AS ship_type_label,
               COALESCE(st.category, 'Unknown') AS category
        FROM vessels v
        LEFT JOIN ship_types st ON v.ship_type = st.ship_type
        WHERE v.mmsi = :mmsi
        """,
        {"mmsi": mmsi},
    ).fetchone()
    if v is None:
        raise HTTPException(404, "Vessel not found")

    dest = _rows_to_dicts(conn.execute(
        "SELECT destination, estimated_arrival, timestamp_utc "
        "FROM destination_history WHERE mmsi = :mmsi "
        "ORDER BY timestamp_utc DESC LIMIT 10",
        {"mmsi": mmsi},
    ))

    nav = _rows_to_dicts(conn.execute(
        """
        SELECT h.navigational_status, ns.label, h.timestamp_utc
        FROM navigational_status_history h
        LEFT JOIN nav_statuses ns ON ns.navigational_status = h.navigational_status
        WHERE h.mmsi = :mmsi
        ORDER BY h.timestamp_utc DESC LIMIT 10
        """,
        {"mmsi": mmsi},
    ))

    draught = _rows_to_dicts(conn.execute(
        "SELECT draught, timestamp_utc FROM draught_history "
        "WHERE mmsi = :mmsi ORDER BY timestamp_utc DESC LIMIT 10",
        {"mmsi": mmsi},
    ))

    pos_count = conn.execute(
        "SELECT COUNT(*) FROM position_history WHERE mmsi = :mmsi",
        {"mmsi": mmsi},
    ).fetchone()[0]

    return {
        "vessel": dict(v),
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
    """Return the track (position list) for a vessel, optionally time-bounded."""
    conn = _get_conn()
    if start is not None and end is not None:
        rows = _rows_to_dicts(conn.execute(
            """
            SELECT timestamp_utc, latitude, longitude,
                   speed_over_ground, course_over_ground, true_heading
            FROM position_history
            WHERE mmsi = :mmsi AND timestamp_utc BETWEEN :s AND :e
            ORDER BY timestamp_utc
            """,
            {"mmsi": mmsi, "s": start, "e": end},
        ))
    else:
        rows = _rows_to_dicts(conn.execute(
            """
            SELECT timestamp_utc, latitude, longitude,
                   speed_over_ground, course_over_ground, true_heading
            FROM position_history
            WHERE mmsi = :mmsi
            ORDER BY timestamp_utc
            """,
            {"mmsi": mmsi},
        ))
    if not rows:
        raise HTTPException(404, "No track data for this vessel")
    return rows


@app.get("/api/search")
async def search_vessels(q: str = Query(..., min_length=1)):
    """Search vessels by name or MMSI (prefix match)."""
    conn = _get_conn()
    rows = _rows_to_dicts(conn.execute(
        """
        SELECT v.mmsi, v.ship_name, v.call_sign, v.imo_number,
               COALESCE(st.label, 'Unknown') AS ship_type_label,
               COALESCE(st.category, 'Unknown') AS category
        FROM vessels v
        LEFT JOIN ship_types st ON v.ship_type = st.ship_type
        WHERE v.ship_name LIKE :q OR CAST(v.mmsi AS TEXT) LIKE :q
        ORDER BY v.ship_name
        LIMIT 50
        """,
        {"q": f"%{q}%"},
    ))
    return rows


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
    """
    Return the most recent known position of vessels, with optional filters.
    Supports bounding-box viewport filtering and ship-type/category filtering.
    Returns at most `limit` random vessels from the filtered set.
    """
    conn = _get_conn()

    where_clauses = []
    params: dict = {}

    # Ship type filter
    if ship_types:
        type_list = [int(t.strip()) for t in ship_types.split(",") if t.strip().isdigit()]
        if type_list:
            placeholders = ",".join(f":st{i}" for i in range(len(type_list)))
            where_clauses.append(f"v.ship_type IN ({placeholders})")
            for i, st in enumerate(type_list):
                params[f"st{i}"] = st

    # Category filter
    if categories:
        cat_list = [c.strip() for c in categories.split(",") if c.strip()]
        if cat_list:
            placeholders = ",".join(f":cat{i}" for i in range(len(cat_list)))
            where_clauses.append(f"COALESCE(stype.category, 'Unknown') IN ({placeholders})")
            for i, cat in enumerate(cat_list):
                params[f"cat{i}"] = cat

    # Bounding box filter
    if all(v is not None for v in [min_lat, max_lat, min_lon, max_lon]):
        where_clauses.append(
            "ph.latitude BETWEEN :min_lat AND :max_lat "
            "AND ph.longitude BETWEEN :min_lon AND :max_lon"
        )
        params["min_lat"] = min_lat
        params["max_lat"] = max_lat
        params["min_lon"] = min_lon
        params["max_lon"] = max_lon

    where_sql = (" WHERE " + " AND ".join(where_clauses)) if where_clauses else ""
    params["lim"] = limit

    sql = f"""
    SELECT
        ph.mmsi,
        v.ship_name,
        v.ship_type,
        COALESCE(stype.label, 'Unknown') AS ship_type_label,
        COALESCE(stype.category, 'Unknown') AS category,
        ph.timestamp_utc,
        ph.latitude,
        ph.longitude,
        ph.speed_over_ground,
        ph.course_over_ground,
        ph.true_heading
    FROM position_history ph
    INNER JOIN (
        SELECT mmsi, MAX(timestamp_utc) AS max_ts
        FROM position_history
        GROUP BY mmsi
    ) latest ON ph.mmsi = latest.mmsi AND ph.timestamp_utc = latest.max_ts
    JOIN vessels v ON ph.mmsi = v.mmsi
    LEFT JOIN ship_types stype ON v.ship_type = stype.ship_type
    {where_sql}
    ORDER BY RANDOM()
    LIMIT :lim
    """
    rows = _rows_to_dicts(conn.execute(sql, params))
    return rows


@app.get("/api/ship-categories")
async def ship_categories():
    """Return all ship type categories with their types and vessel counts."""
    conn = _get_conn()
    rows = _rows_to_dicts(conn.execute("""
        SELECT
            COALESCE(st.category, 'Unknown') AS category,
            v.ship_type AS ship_type_code,
            COALESCE(st.label, 'Unknown') AS ship_type_label,
            COUNT(*) AS vessel_count
        FROM vessels v
        LEFT JOIN ship_types st ON v.ship_type = st.ship_type
        GROUP BY COALESCE(st.category, 'Unknown'), v.ship_type
        ORDER BY category, vessel_count DESC
    """))
    return rows


@app.get("/api/ais-coverage")
async def ais_coverage():
    """Return AIS message counts per minute for the coverage timeline."""
    conn = _get_conn()
    rows = _rows_to_dicts(conn.execute(
        "SELECT bucket_start, message_count FROM ais_message_counts ORDER BY bucket_start"
    ))
    return rows


@app.get("/api/stats")
async def stats():
    """Quick stats for the dashboard header."""
    conn = _get_conn()
    total_vessels = conn.execute("SELECT total_vessels FROM v_total_vessels").fetchone()[0]
    total_positions = conn.execute("SELECT total_positions FROM v_total_positions").fetchone()[0]
    by_type = _rows_to_dicts(conn.execute("SELECT * FROM v_vessels_by_type LIMIT 20"))
    return {
        "total_vessels": total_vessels,
        "total_positions": total_positions,
        "vessels_by_type": by_type,
    }


@app.get("/api/db-metrics")
async def db_metrics():
    """Comprehensive database metrics for the DB Metrics page."""
    conn = _get_conn()

    # ── Counts ─────────────────────────────────────────────────────────
    total_vessels = conn.execute("SELECT total_vessels FROM v_total_vessels").fetchone()[0]
    total_positions = conn.execute("SELECT total_positions FROM v_total_positions").fetchone()[0]

    total_destinations = conn.execute("SELECT COUNT(*) FROM destination_history").fetchone()[0]
    total_draughts = conn.execute("SELECT COUNT(*) FROM draught_history").fetchone()[0]
    total_nav_statuses = conn.execute("SELECT COUNT(*) FROM navigational_status_history").fetchone()[0]
    total_ship_names = conn.execute("SELECT COUNT(*) FROM ship_name_history").fetchone()[0]

    # ── Vessels by type ────────────────────────────────────────────────
    vessels_by_type = _rows_to_dicts(conn.execute(
        "SELECT * FROM v_vessels_by_type"
    ))

    # ── Vessels by category ────────────────────────────────────────────
    vessels_by_category = _rows_to_dicts(conn.execute(
        """
        SELECT
            COALESCE(st.category, 'Unknown') AS category,
            COUNT(*) AS vessel_count
        FROM vessels v
        LEFT JOIN ship_types st ON v.ship_type = st.ship_type
        GROUP BY category
        ORDER BY vessel_count DESC
        """
    ))

    # ── Time range ─────────────────────────────────────────────────────
    time_range_row = conn.execute(
        "SELECT MIN(timestamp_utc) AS ts_min, MAX(timestamp_utc) AS ts_max "
        "FROM position_history"
    ).fetchone()
    time_range = {
        "ts_min": time_range_row["ts_min"],
        "ts_max": time_range_row["ts_max"],
    } if time_range_row and time_range_row["ts_min"] else None

    # ── Positions per day (last 30 days) ───────────────────────────────
    positions_per_day = _rows_to_dicts(conn.execute(
        """
        SELECT
            DATE(timestamp_utc, 'unixepoch') AS day,
            COUNT(*) AS cnt
        FROM position_history
        GROUP BY day
        ORDER BY day DESC
        LIMIT 30
        """
    ))

    # ── Database file size ─────────────────────────────────────────────
    db_size_bytes = 0
    try:
        db_size_bytes = os.path.getsize(DATABASE_PATH)
    except OSError:
        pass

    # ── Table sizes (page counts from SQLite internals) ────────────────
    table_sizes = []
    for tbl_name in [
        "vessels", "position_history", "destination_history",
        "draught_history", "navigational_status_history", "ship_name_history",
        "ship_types", "nav_statuses", "ais_message_counts", "vessel_details",
    ]:
        try:
            row_count = conn.execute(f"SELECT COUNT(*) FROM [{tbl_name}]").fetchone()[0]
            table_sizes.append({"table": tbl_name, "row_count": row_count})
        except Exception:
            pass

    return {
        "total_vessels": total_vessels,
        "total_positions": total_positions,
        "total_destinations": total_destinations,
        "total_draughts": total_draughts,
        "total_nav_statuses": total_nav_statuses,
        "total_ship_names": total_ship_names,
        "vessels_by_type": vessels_by_type,
        "vessels_by_category": vessels_by_category,
        "time_range": time_range,
        "positions_per_day": positions_per_day,
        "db_size_bytes": db_size_bytes,
        "table_sizes": table_sizes,
    }


# ── Serve built Vite assets (JS, CSS) — must be BEFORE the SPA catch-all ───
# Vite outputs assets to static/assets/; this mount makes /assets/* work.
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
