#!/bin/bash
# Launch the Rust AIS ingestor in watch mode (background).
# Equivalent to the Python: nohup python3 ./ais_ingestor/ingest.py --watch

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$SCRIPT_DIR/target/release/ais_ingestor"

if [ ! -f "$BINARY" ]; then
    echo "Binary not found. Building release…"
    cd "$SCRIPT_DIR" && cargo build --release
fi

nohup "$BINARY" --watch > /mnt/wdblack/ais_ingestor.log 2>&1 &
echo "Started AIS ingestor (PID $!)"
