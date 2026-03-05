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
├── data_processor/      # Higher-level data processing pipeline
│   ├── ingest_ais.py    # JSONL → SQLite ingestion script
│   ├── config.py        # Paths, batch size, derived from ais_collector config
│   ├── schema.py        # Database table definitions
│   ├── db.py            # SQLite connection helper (tuned for Raspberry Pi)
│   └── barquitos.db     # The SQLite database (created on first run)
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

# 2. Ingest collected data into the database
cd ../data_processor
python ingest_ais.py                           # one-shot
python ingest_ais.py --watch                   # continuous mode
python ingest_ais.py --input-dir ../data       # override input folder
```

## Database

SQLite with WAL mode, optimised for Raspberry Pi 3B (low RAM, SD card I/O).
See [`data_processor/README.md`](data_processor/README.md) for schema details
and example queries.
