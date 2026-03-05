# AISstream.io Raw Data Collector

Connects to [aisstream.io](https://aisstream.io/) via WebSocket and stores **raw AIS messages** to disk as [JSONL](https://jsonlines.org/) files (one JSON object per line). No processing is done — data is saved exactly as received from the API.

## Setup

### 1. Install dependencies

```bash
pip install -r requirements.txt
```

### 2. Get an API key

Sign up at <https://aisstream.io/authenticate> to get a free API key.

### 3. Configure

Edit `config.py` and set:

| Setting | Description |
|---|---|
| `API_KEY` | Your AISstream API key (or set `AISSTREAM_API_KEY` env var) |
| `BOUNDING_BOXES` | Geographic filter — default is the entire world |
| `FILTER_MMSI` | Optional list of vessel MMSIs to track |
| `FILTER_MESSAGE_TYPES` | Optional list of message types to receive |
| `DATA_OUTPUT_DIR` | Where raw `.jsonl` files are saved |
| `MAX_MESSAGES_PER_FILE` | File rotation threshold (0 = hourly only) |

### 4. Run

```bash
# Option A: API key in config.py
python collect_ais.py

# Option B: API key via environment variable
AISSTREAM_API_KEY="your_key_here" python collect_ais.py
```

## Output

Files are written to `DATA_OUTPUT_DIR` (default: `data/raw/`) with the naming pattern:

```
ais_YYYYMMDD_HHMM_0000.jsonl
ais_YYYYMMDD_HHMM_0001.jsonl   (after rotation)
...
```

A new file is created **every minute**, keeping RAM usage low and limiting data loss to at most ~60 seconds on any error.

Each line is a raw JSON message exactly as received from AISstream. Example:

```json
{"MessageType":"PositionReport","MetaData":{"MMSI":123456789,"ShipName":"EXAMPLE VESSEL",...},"Message":{"PositionReport":{"UserID":123456789,"Latitude":37.123,"Longitude":-122.456,...}}}
```

## Graceful shutdown

Press `Ctrl+C` to stop. The script will flush and close the current file cleanly.

## Reconnection

The script automatically reconnects with exponential backoff (configurable in `config.py`) if the WebSocket connection drops.
