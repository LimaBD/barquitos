#!/usr/bin/env bash
# ─── Stop the AIS Data Lake Ingestor ─────────────────────────────────────────
#
# Sends SIGTERM for graceful shutdown, waits up to 30 seconds, then SIGKILL.
# ────────────────────────────────────────────────────────────────────────────

set -euo pipefail

PID_FILE="/mnt/wdblack/ais_ingestor.pid"

if [[ ! -f "$PID_FILE" ]]; then
    echo "⚠️  No PID file found at $PID_FILE — ingestor may not be running."
    exit 0
fi

PID=$(cat "$PID_FILE")

if ! kill -0 "$PID" 2>/dev/null; then
    echo "⚠️  Process $PID is not running. Cleaning up PID file."
    rm -f "$PID_FILE"
    exit 0
fi

echo "🛑 Sending SIGTERM to ingestor (PID $PID) …"
kill -TERM "$PID"

# Wait up to 30 seconds for graceful shutdown
TIMEOUT=30
for i in $(seq 1 $TIMEOUT); do
    if ! kill -0 "$PID" 2>/dev/null; then
        echo "✅ Ingestor stopped gracefully after ${i}s."
        rm -f "$PID_FILE"
        exit 0
    fi
    sleep 1
done

echo "⚠️  Ingestor did not stop after ${TIMEOUT}s. Sending SIGKILL …"
kill -9 "$PID" 2>/dev/null || true
rm -f "$PID_FILE"
echo "✅ Ingestor killed."
