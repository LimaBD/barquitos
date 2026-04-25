#!/usr/bin/env bash
# Print the last 50 lines of the AIS Ingestor log.
# Pass -f / --follow to tail continuously.
set -euo pipefail

LOG_FILE="$HOME/.barquitos/logs/ingestor.log"

if [ ! -f "$LOG_FILE" ]; then
    echo "No log file found at $LOG_FILE"
    echo "Has the ingestor been started yet?"
    exit 1
fi

if [[ "${1:-}" == "-f" || "${1:-}" == "--follow" ]]; then
    tail -n 50 -f "$LOG_FILE"
else
    tail -n 50 "$LOG_FILE"
fi
