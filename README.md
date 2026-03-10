# Barquitos — Trade Nowcasting from AIS Ship Data

Collects global AIS (Automatic Identification System) ship data, processes it
into a Parquet Data Lake, and will use it to predict market prices and trade
flows.

Runs on a **Raspberry Pi 3B** with a USB mechanical HDD (`/mnt/wdblack/`).

## Project Structure

```
barquitos/
├── ais_collector/            # Raw AIS data collector (WebSocket → JSONL)
│   ├── collect_ais.py        # Streaming collector script
│   └── config.py             # API key, filters, output directory
│
├── ais_ingestor_rs/          # Rust: JSONL → Parquet Data Lake pipeline
│   ├── src/
│   │   ├── main.rs           # Daemon — scan → parse → edge-filter → flush Parquet
│   │   ├── backfill.rs       # One-shot historical JSONL reprocessing
│   │   ├── config.rs         # Paths, batch sizes, Parquet tuning constants
│   │   ├── parser.rs         # JSONL → PositionRecord / StaticRecord
│   │   ├── schema.rs         # Arrow schemas for 6 fact tables
│   │   ├── edge_state.rs     # HashMap<MMSI, VesselEdge> for pivot detection
│   │   └── writer.rs         # Hive-partitioned Parquet writer (ZSTD, bloom filters)
│   ├── run_ingestor.bash     # Start daemon in background
│   ├── stop_ingestor.bash    # Stop daemon via PID file
│   └── check_ingestor.bash   # Check daemon status
│
├── dashboard/                # Ship-tracking web dashboard
│   ├── server.py             # FastAPI backend — DuckDB queries on Parquet
│   ├── config.py             # LAKE_ROOT, host, port
│   ├── frontend/             # React + TypeScript + Vite source
│   └── static/               # Built SPA (served by FastAPI)
│
└── .env                      # API keys (not committed)
```

## Architecture

```
┌──────────────────┐   JSONL    ┌──────────────────┐  Parquet  ┌─────────────┐
│  ais_collector    │ ────────→ │  ais_ingestor_rs  │ ───────→ │  Data Lake   │
│  (Python)         │  /mnt/    │  (Rust daemon)    │  /mnt/   │  (Parquet)   │
│  WebSocket→JSONL  │  wdblack/ │  edge-filtering   │  wdblack/│              │
└──────────────────┘  ais/     └──────────────────┘  trading_  └──────┬──────┘
                                                      lake/           │
                                                                 DuckDB reads
                                                                      │
                                                              ┌───────▼──────┐
                                                              │  Dashboard    │
                                                              │  (FastAPI)    │
                                                              │  Leaflet map  │
                                                              └──────────────┘
```

### Data Lake (Medallion Architecture — Bronze Layer)

All data is stored as **Hive-partitioned Parquet** files under
`/mnt/wdblack/trading_lake/bronze/source=ais/`:

| Fact / Dimension Table        | Description                              | Write Frequency   |
|-------------------------------|------------------------------------------|-------------------|
| `fact_position`               | 15-second GPS pings (lat, lon, SOG, COG) | Every message     |
| `dim_vessel`                  | Edge-filtered vessel static (IMO, type, length, beam) | On change only |
| `fact_vessel_name_history`    | Edge-filtered vessel name changes         | On change only    |
| `fact_draught`                | Edge-filtered draught changes             | On change only    |
| `fact_navigational_status`    | Edge-filtered nav status changes          | On change only    |
| `fact_destination`            | Edge-filtered destination changes         | On change only    |
| `metric_system_health`        | Ingestion throughput per time window      | Per flush         |

> **See [`ais_ingestor_rs/DATA_DICTIONARY.md`](ais_ingestor_rs/DATA_DICTIONARY.md)
> for full column-level schema documentation.**

Parquet files are written with:
- **ZSTD compression** for minimal disk usage
- **MMSI bloom filters** for fast single-vessel lookups
- **Sort-by-MMSI** within each row group for predicate pushdown
- **Hive partitioning** (`year=YYYY/month=MM/day=DD`) for time-range pruning (fact tables); `dim_vessel` is a single flat file
- **15-minute time windows** per file

### Three Independent Components

| Component | Language | Responsibility |
|---|---|---|
| **ais_collector** | Python | Connects to AISstream.io WebSocket, writes raw JSONL files to `/mnt/wdblack/ais/`. |
| **ais_ingestor_rs** | Rust | Reads JSONL, parses AIS messages, applies edge-state filtering, writes Hive-partitioned Parquet to the Data Lake. Moves processed files to `/mnt/wdblack/ais_processed/`. |
| **dashboard** | Python (FastAPI) + React/TS | Reads Parquet via DuckDB, serves a Leaflet-based ship-tracking SPA. |

Future data sources (satellite imagery, port schedules, commodity feeds, etc.)
will each get their own `*_collector` + `*_ingestor` pair, all writing into
the same Data Lake.

## Quick Start

### 1. Collect AIS data

```bash
cd ais_collector
pip install -r requirements.txt
python collect_ais.py
```

### 2. Ingest into the Data Lake

```bash
cd ais_ingestor_rs
cargo build --release

# Daemon mode (production — runs in background)
bash run_ingestor.bash
bash check_ingestor.bash    # check status
bash stop_ingestor.bash     # stop

# Backfill historical data (one-shot)
cargo run --release --bin ais_backfill -- /path/to/jsonl_dir /path/to/lake_root
```

### 3. Run the dashboard

```bash
pip install -r dashboard/requirements.txt
python -m dashboard.server
# → http://0.0.0.0:8080
```

## Configuration

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `AISSTREAM_API_KEY` | *(in config.py)* | AISstream.io API key |
| `LAKE_ROOT` | `/mnt/wdblack/trading_lake` | Root of the Parquet Data Lake |
| `DASHBOARD_HOST` | `0.0.0.0` | Dashboard bind address |
| `DASHBOARD_PORT` | `8080` | Dashboard port |

### Rust Ingestor Constants (`ais_ingestor_rs/src/config.rs`)

| Constant | Default | Description |
|---|---|---|
| `LAKE_ROOT` | `/mnt/wdblack/trading_lake` | Parquet output root |
| `AIS_INPUT_DIR` | `/mnt/wdblack/ais` | JSONL input directory |
| `AIS_PROCESSED_DIR` | `/mnt/wdblack/ais_processed` | Processed JSONL archive |
| `SCAN_INTERVAL_SECS` | `30` | Daemon poll interval |
| `FILES_PER_BATCH` | `15` | JSONL files per micro-batch |
| `PARQUET_ROW_GROUP_SIZE` | `8192` | Max row group size |

## Dashboard API Endpoints

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

## Dependencies

### Python
- `websockets` — AISstream WebSocket collector
- `fastapi`, `uvicorn` — Dashboard backend
- `duckdb` — Parquet query engine

### Rust
- `arrow-array`, `arrow-schema`, `arrow-select`, `arrow-ord` — Columnar in-memory data
- `parquet` (with ZSTD + Snappy) — Parquet file writer
- `chrono`, `serde`, `serde_json` — Time handling, JSON parsing
- `log`, `env_logger` — Logging
- `ctrlc` — Graceful daemon shutdown
