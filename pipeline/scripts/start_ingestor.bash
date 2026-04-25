#!/usr/bin/env bash
# Start the AIS Ingestor in the background.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PIPELINE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="$PIPELINE_DIR/target/release/ais_ingestor"
PID_FILE="$HOME/.barquitos/run/ingestor.pid"
LOG_FILE="$HOME/.barquitos/logs/ingestor.log"

if [ ! -f "$BINARY" ]; then
    echo "Binary not found: $BINARY"
    echo "Run 'cargo build --release' inside $PIPELINE_DIR first."
    exit 1
fi

if [ -f "$PID_FILE" ] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    echo "Ingestor is already running (PID $(cat "$PID_FILE"))."
    exit 0
fi

mkdir -p "$(dirname "$PID_FILE")" "$(dirname "$LOG_FILE")"

cd "$PIPELINE_DIR"
nohup "$BINARY" >> "$LOG_FILE" 2>&1 &
echo $! > "$PID_FILE"
echo "Ingestor started (PID $!). Logs: $LOG_FILE"
