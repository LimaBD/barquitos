#!/bin/bash
# Launch the Rust AIS ingestor in watch mode (background).
# Equivalent to the Python: nohup python3 ./ais_ingestor/ingest.py --watch
#
# Uses all CPU cores for parallel JSONL parsing by default.
# Override with: THREADS=4 ./run.bash

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$SCRIPT_DIR/target/release/ais_ingestor"
PIDFILE="$SCRIPT_DIR/ais_ingestor.pid"
THREADS="${THREADS:-0}"  # 0 = auto-detect

if [ ! -f "$BINARY" ]; then
    echo "Binary not found. Building release…"
    cd "$SCRIPT_DIR" && cargo build --release
fi

nohup "$BINARY" --watch --threads "$THREADS" > /mnt/wdblack/ais_ingestor.log 2>&1 &
echo $! > "$PIDFILE"
echo "Started AIS ingestor (PID $!) with --threads $THREADS"
