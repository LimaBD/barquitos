#!/usr/bin/env bash
# Stop the AIS Mover gracefully (SIGTERM → SIGKILL after 10 s).
set -euo pipefail

PID_FILE="$HOME/.barquitos/run/mover.pid"
NAME="Mover"

if [ ! -f "$PID_FILE" ]; then
    echo "$NAME is not running (no PID file found)."
    exit 0
fi

PID=$(cat "$PID_FILE")

if ! kill -0 "$PID" 2>/dev/null; then
    echo "$NAME is not running (stale PID $PID). Removing PID file."
    rm -f "$PID_FILE"
    exit 0
fi

echo "Stopping $NAME (PID $PID) …"
kill -TERM "$PID"

for i in $(seq 1 10); do
    sleep 1
    if ! kill -0 "$PID" 2>/dev/null; then
        rm -f "$PID_FILE"
        echo "$NAME stopped."
        exit 0
    fi
done

echo "$NAME did not stop in 10 s — sending SIGKILL …"
kill -KILL "$PID" 2>/dev/null || true
rm -f "$PID_FILE"
echo "$NAME killed."
