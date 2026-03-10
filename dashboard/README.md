# 🚢 Barquitos Dashboard

A custom ship-tracking dashboard with a world map and Dahua-style timeline scrubber.

## Architecture

- **Backend**: Python FastAPI server (`server.py`) — queries the Parquet Data Lake via DuckDB
- **Frontend**: React + TypeScript + Vite — Leaflet map with canvas-based timeline

The backend reads Hive-partitioned Parquet files directly using DuckDB
(in-memory connection, no persistent database). All data lives under
`LAKE_ROOT/bronze/source=ais/` in 5 fact tables + 1 dimension table.

## Features

- **World Map** — dark-themed Leaflet map showing all vessels as coloured arrow markers (colour = category: cargo, tanker, passenger, etc.)
- **Dahua-style Timeline** — security-camera-style time bar at the bottom:
  - **Green bars** = time periods with AIS data
  - **Dark/grey** = no data
  - **Red playhead** = current position in time
  - **Drag** to pan, **scroll** to zoom, **click** to seek
- **Live / Playback modes** — switch between real-time latest positions and historical playback
- **Playback transport controls** — play/pause, skip ±1m/±5m/±1h, adjustable speed (1×–600×)
- **Click any ship** — opens side panel with full vessel details:
  - Name, MMSI, IMO, call sign, type, dimensions
  - Destination history, nav status history, draught history
  - Full track drawn on map (dashed blue line, green start → red end marker)
- **Search** — search vessels by name or MMSI with debounced autocomplete

## Running

### Backend

```bash
# From the repo root:
pip install -r dashboard/requirements.txt
python -m dashboard.server
```

The server starts on `http://0.0.0.0:8080` by default.

Environment variables:
- `LAKE_ROOT` — root of the Parquet Data Lake (default: `/mnt/wdblack/trading_lake`)
- `DASHBOARD_HOST` — bind address (default: `0.0.0.0`)
- `DASHBOARD_PORT` — port (default: `8080`)

### Frontend (development)

```bash
cd dashboard/frontend
npm install
npm run dev
```

Vite dev server starts on `http://localhost:5173` and proxies `/api/*` to the FastAPI backend.

### Frontend (production build)

```bash
cd dashboard/frontend
npm run build
```

Outputs to `dashboard/static/` which the FastAPI server serves automatically.

## API Endpoints

| Endpoint | Description |
|---|---|
| `GET /api/time-range` | Min/max timestamps in positions |
| `GET /api/coverage?bucket_seconds=3600` | Hourly buckets with position counts (timeline) |
| `GET /api/positions?ts=<epoch>&window=30` | Vessel positions at a point in time |
| `GET /api/latest-positions` | Most recent position of every vessel |
| `GET /api/vessel/{mmsi}` | Full vessel metadata + history |
| `GET /api/vessel/{mmsi}/track` | Complete track for a vessel |
| `GET /api/vessel/{mmsi}/track?start=<epoch>&end=<epoch>` | Partial track |
| `GET /api/search?q=<text>` | Search vessels by name/MMSI |
| `GET /api/ship-categories` | Vessel type categories with counts |
| `GET /api/stats` | Quick stats (total vessels, positions, by type) |
| `GET /api/db-metrics` | Comprehensive Data Lake metrics |
| `GET /api/ais-coverage` | Ingestion health timeline |

## Data Lake Tables

The backend reads from these Parquet glob patterns:

| Glob | Source |
|---|---|
| `{LAKE_ROOT}/bronze/source=ais/fact_position/**/*.parquet` | GPS pings (lat, lon, SOG, COG) |
| `{LAKE_ROOT}/bronze/source=ais/dim_vessel/vessel_static.parquet` | Ship IMO, call sign, type, dimensions (edge-filtered) |
| `{LAKE_ROOT}/bronze/source=ais/fact_draught/**/*.parquet` | Edge-filtered draught changes |
| `{LAKE_ROOT}/bronze/source=ais/fact_navigational_status/**/*.parquet` | Edge-filtered nav status changes |
| `{LAKE_ROOT}/bronze/source=ais/fact_destination/**/*.parquet` | Edge-filtered destination changes |
| `{LAKE_ROOT}/bronze/source=ais/metric_system_health/**/*.parquet` | Ingestion throughput per window |
