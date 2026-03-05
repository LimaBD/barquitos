# Barquitos — Trade Nowcasting from AIS Ship Data

Collects global AIS (Automatic Identification System) ship data, processes it
into a structured database, and will use it to predict market prices and trade
flows.

## Project Structure

```
barquitos/
├── ais_collector/       # Raw AIS data collector (WebSocket → JSONL)
│   ├── collect_ais.py   # Streaming collector script
│   └── config.py        # API key, filters, output directory
│
├── ais_ingestor/        # AIS JSONL → SQLite ingestion pipeline
│   ├── ingest.py        # Main ingestion script
│   └── config.py        # Paths, batch size, vessel filtering
│
├── db/                  # Shared database layer (used by all ingestors)
│   ├── connection.py    # SQLite connection helper (tuned for Raspberry Pi)
│   ├── schema.py        # Database table definitions
│   └── verify.py        # Quick DB verification script
│
├── data/                # Landing zone for new raw JSONL files
└── data_processed/      # Backup of already-ingested JSONL files
```

## Quick Start

```bash
# 1. Start collecting AIS data
cd ais_collector
pip install -r requirements.txt
python collect_ais.py

# 2. Ingest collected data into the database (from project root)
python -m ais_ingestor                         # one-shot
python -m ais_ingestor --watch                 # continuous mode
python -m ais_ingestor --input-dir ./data      # override input folder

# 3. Verify the database
python db/verify.py
```

## Architecture

The project is split into three independent concerns:

| Component | Responsibility |
|---|---|
| **ais_collector** | Connects to AISstream.io WebSocket, writes raw JSONL files to disk. |
| **ais_ingestor** | Reads JSONL files, parses AIS messages, writes to the shared DB. Filters out non-commercial vessels. |
| **db** | Shared database layer — schema, connection tuning, verification. Used by all ingestors. |

Future data sources (satellite imagery, port schedules, commodity feeds, etc.)
will each get their own `*_collector` + `*_ingestor` pair, all writing into
the same shared `db`.

## Database

SQLite with WAL mode, optimised for Raspberry Pi 3B (low RAM, SD card I/O).
See [`db/README.md`](db/README.md) for schema details and example queries.
