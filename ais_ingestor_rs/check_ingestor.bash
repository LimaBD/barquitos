#!/usr/bin/env bash
# ─── Check AIS Ingestor Status & Review Logs ─────────────────────────────────
#
# Shows whether the ingestor is running, then tails the last 50 lines of logs.
# ────────────────────────────────────────────────────────────────────────────

set -euo pipefail

PID_FILE="/mnt/wdblack/ais_ingestor.pid"
LOG_FILE="/mnt/wdblack/ais_ingestor.log"

echo "═══════════════════════════════════════════════════════════════"
echo "  AIS Data Lake Ingestor — Status"
echo "═══════════════════════════════════════════════════════════════"

# Process status
if [[ -f "$PID_FILE" ]]; then
    PID=$(cat "$PID_FILE")
    if kill -0 "$PID" 2>/dev/null; then
        echo "  Status:  ✅ RUNNING (PID $PID)"
        # Show memory/CPU usage
        ps -p "$PID" -o pid,rss,vsz,%mem,%cpu,etime --no-headers 2>/dev/null | \
            awk '{printf "  PID: %s | RSS: %d MB | VSZ: %d MB | MEM: %s%% | CPU: %s%% | Uptime: %s\n", $1, $2/1024, $3/1024, $4, $5, $6}'
    else
        echo "  Status:  ❌ NOT RUNNING (stale PID $PID)"
    fi
else
    echo "  Status:  ❌ NOT RUNNING (no PID file)"
fi

# Data Lake stats
echo ""
echo "── Data Lake ──────────────────────────────────────────────────"
LAKE="/mnt/wdblack/trading_lake"
if [[ -d "$LAKE" ]]; then
    TOTAL_SIZE=$(du -sh "$LAKE" 2>/dev/null | cut -f1)
    PARQUET_COUNT=$(find "$LAKE" -name "*.parquet" 2>/dev/null | wc -l)
    echo "  Location:      $LAKE"
    echo "  Total size:    $TOTAL_SIZE"
    echo "  Parquet files: $PARQUET_COUNT"
else
    echo "  Data Lake not found at $LAKE"
fi

# Input queue
echo ""
echo "── Input Queue ────────────────────────────────────────────────"
INPUT_DIR="/mnt/wdblack/ais"
if [[ -d "$INPUT_DIR" ]]; then
    PENDING=$(ls "$INPUT_DIR"/*.jsonl 2>/dev/null | wc -l)
    echo "  Pending JSONL files: $PENDING"
else
    echo "  Input dir not found at $INPUT_DIR"
fi

# Processed files
PROC_DIR="/mnt/wdblack/ais_processed"
if [[ -d "$PROC_DIR" ]]; then
    PROCESSED=$(ls "$PROC_DIR"/*.jsonl 2>/dev/null | wc -l)
    echo "  Processed JSONL files: $PROCESSED"
fi

# Recent logs
echo ""
echo "── Recent Logs (last 30 lines) ──────────────────────────────"
if [[ -f "$LOG_FILE" ]]; then
    tail -30 "$LOG_FILE"
else
    echo "  No log file found at $LOG_FILE"
fi
echo ""
echo "═══════════════════════════════════════════════════════════════"
