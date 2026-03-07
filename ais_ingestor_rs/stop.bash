#!/bin/bash
# Stop the Rust AIS ingestor gracefully (SIGTERM).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PIDFILE="$SCRIPT_DIR/ais_ingestor.pid"

if [ -f "$PIDFILE" ]; then
    PID=$(cat "$PIDFILE")
    if kill -0 "$PID" 2>/dev/null; then
        kill "$PID"
        echo "Stopped AIS ingestor (PID $PID)"
    else
        echo "Process $PID is not running"
    fi
    rm -f "$PIDFILE"
else
    # Fallback: find by process name
    PID=$(pgrep -f "ais_ingestor.*--watch")
    if [ -n "$PID" ]; then
        kill "$PID"
        echo "Stopped AIS ingestor (PID $PID)"
    else
        echo "No running AIS ingestor found"
    fi
fi
