# Barquitos — Trade Nowcasting from AIS Ship Data

Collects global AIS (Automatic Identification System) ship data, processes it
into a Parquet Data Lake, and will use it to predict market prices and trade
flows.

Runs on a **Raspberry Pi 3B** with a USB hard drive mounted at `/mnt/wdblack`.

---

## Project Structure

```
barquitos/
├── pipeline/                   # Rust data pipeline (any data source)
│   ├── Cargo.toml              # Cargo workspace root
│   ├── config/
│   │   ├── collector.toml      # Collector config (api_key, output_dir, …)
│   │   ├── ingestor.toml       # Ingestor config (paths, Parquet tuning, …)
│   │   └── mover.toml          # Mover config (src/dest dirs, interval, …)
│   ├── crates/
│   │   ├── common/             # Shared library (config, mount_check, safe_io)
│   │   ├── collector/          # bin: ais_collector  — WebSocket → JSONL
│   │   ├── ingestor/           # bin: ais_ingestor   — JSONL → Parquet
│   │   │                       # bin: ais_backfill   — one-shot historical reprocess
│   │   └── mover/              # bin: ais_mover      — hourly file move
│   └── scripts/
│       ├── install.sh          # Build + install everything (run as root)
│       ├── mnt-wdblack.mount   # systemd mount unit (/dev/sda1 → /mnt/wdblack)
│       ├── ais-collector.service
│       ├── ais-mover.service
│       └── ais-ingestor.service
│
├── dashboard/                  # Ship-tracking web dashboard
│   ├── server.py               # FastAPI backend — DuckDB queries on Parquet
│   ├── config.py               # LAKE_ROOT, host, port
│   ├── frontend/               # React + TypeScript + Vite source
│   └── static/                 # Built SPA (served by FastAPI)
│
├── README.md
└── .gitignore
```

---

## Architecture & Data Flow

```
AISstream.io
  WebSocket
      │
      ▼
┌─────────────────┐  JSONL files   ┌──────────────┐  JSONL files   ┌─────────────────┐
│  ais_collector  │ ─────────────→ │  ais_mover   │ ─────────────→ │  ais_ingestor   │
│  (system disk)  │  every minute  │  (1h batch)  │  /mnt/wdblack/ │  (WD Black HDD) │
│  ~/.barquitos/  │                └──────────────┘  ais/           └────────┬────────┘
│  ais/           │                                                           │
└─────────────────┘                                                    Parquet (ZSTD)
                                                                               │
                                                                    /mnt/wdblack/
                                                                    trading_lake/
                                                                               │
                                                                    ┌──────────▼──────────┐
                                                                    │     Dashboard        │
                                                                    │ FastAPI + DuckDB     │
                                                                    │ Leaflet map          │
                                                                    └─────────────────────┘
```

### Anti-Crash Design

All disk I/O runs inside timeout-protected threads via `crates/common/src/safe_io.rs`.
If a bad sector or kernel hang causes a read/copy to freeze, the operation times out and
the offending file is **automatically quarantined** (moved to a quarantine directory with
a timestamp prefix). The daemon continues uninterrupted.

A background **watchdog thread** monitors mount health and calls `process::exit` if the
disk becomes unresponsive beyond the configured deadline.

---

## Data Lake (Bronze Layer)

Hive-partitioned Parquet under `/mnt/wdblack/trading_lake/bronze/source=ais/`:

| Table | Description | Write Frequency |
|---|---|---|
| `fact_position` | GPS pings (lat, lon, SOG, COG) | Every message |
| `dim_vessel` | Vessel static (IMO, type, length, beam) | On change only |
| `fact_vessel_name_history` | Name changes | On change only |
| `fact_draught` | Draught changes | On change only |
| `fact_navigational_status` | Nav status changes | On change only |
| `fact_destination` | Destination changes | On change only |
| `metric_system_health` | Ingestion throughput | Per flush |

Parquet files use **ZSTD compression**, **MMSI bloom filters**, **sort-by-MMSI** within
row groups, and **15-minute time windows** per file.

See [`ais_ingestor_rs/DATA_DICTIONARY.md`](ais_ingestor_rs/DATA_DICTIONARY.md) for full
column-level schema documentation.

---

## Quick Start

### Prerequisites

- Rust toolchain (`rustup`) — <https://rustup.rs>
- Python 3.10+ (for dashboard only)
- Node.js 18+ (to rebuild the frontend — optional, pre-built assets are committed)

### Build

```bash
cd pipeline
cargo build --release
# Produces:
#   pipeline/target/release/ais_collector
#   pipeline/target/release/ais_mover
#   pipeline/target/release/ais_ingestor
#   pipeline/target/release/ais_backfill
```

### Configure

Each binary has its own config file under [`pipeline/config/`](pipeline/config/) — edit without recompiling:

| File | Binary | Key settings |
|---|---|---|
| [`config/collector.toml`](pipeline/config/collector.toml) | `ais_collector` | `api_key`, `output_dir`, `bounding_boxes` |
| [`config/mover.toml`](pipeline/config/mover.toml) | `ais_mover` | `source_dir`, `dest_dir`, `interval_secs` |
| [`config/ingestor.toml`](pipeline/config/ingestor.toml) | `ais_ingestor`, `ais_backfill` | `input_dir`, `lake_root`, Parquet tuning |

Config path resolution order (first match wins):
1. `--config <path>` CLI argument
2. Per-crate env var: `BARQUITOS_COLLECTOR_CONFIG` / `BARQUITOS_INGESTOR_CONFIG` / `BARQUITOS_MOVER_CONFIG`
3. `/etc/barquitos/<name>.toml` (system install)
4. `./config/<name>.toml` (cwd — works out of the box when running from `pipeline/`)

### Run (development)

```bash
cd pipeline
# Each in its own terminal, or use a process manager
RUST_LOG=info ./target/release/ais_collector
RUST_LOG=info ./target/release/ais_mover
RUST_LOG=info ./target/release/ais_ingestor

# One-shot backfill
RUST_LOG=info ./target/release/ais_backfill --input /path/to/jsonl --lake /path/to/lake
```

### Install as systemd services (production)

```bash
cd pipeline
cargo build --release
sudo bash scripts/install.sh
```

`install.sh` will:
- Copy binaries to `/usr/local/bin/`
- Install config to `/etc/barquitos/config.toml`
- Create the `server` system user
- Create data directories
- Enable and start the systemd mount unit + 3 service units

### Dashboard

```bash
pip install -r dashboard/requirements.txt
LAKE_ROOT=/mnt/wdblack/trading_lake python -m dashboard.server
# → http://0.0.0.0:8080
```

---

## Dashboard API

| Endpoint | Description |
|---|---|
| `GET /api/time-range` | Min/max timestamps in the lake |
| `GET /api/positions?ts=<epoch>&window=30` | Vessel positions at a point in time |
| `GET /api/latest-positions` | Most recent position of every vessel |
| `GET /api/vessel/{mmsi}` | Full vessel metadata + history |
| `GET /api/vessel/{mmsi}/track` | Complete track for a vessel |
| `GET /api/search?q=<text>` | Search vessels by name/MMSI |
| `GET /api/stats` | Quick stats (total vessels, positions, by type) |
| `GET /api/db-metrics` | Comprehensive Data Lake metrics |

---

## Dependencies

### Rust (workspace)
- `arrow-array / arrow-schema / arrow-select / arrow-ord` — columnar in-memory data
- `parquet` (ZSTD + bloom filters) — Parquet writer
- `tokio` + `tokio-tungstenite` — async WebSocket (collector only)
- `serde` + `toml` — config deserialization
- `chrono` — timestamp handling
- `ctrlc` — graceful shutdown
- `log` + `env_logger` — structured logging

### Python (dashboard)
- `fastapi` + `uvicorn` — HTTP server
- `duckdb` — Parquet query engine

