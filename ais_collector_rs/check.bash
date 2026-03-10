#!/usr/bin/env bash
# ─── Check AIS Collector Status & Review Logs ────────────────────────────────
#
# Shows whether the collector is running, then tails the last 50 lines of logs.
# ────────────────────────────────────────────────────────────────────────────

set -euo pipefail

PID_FILE="/mnt/wdblack/ais_collector.pid"
LOG_FILE="/mnt/wdblack/ais_collector.log"

echo "═══════════════════════════════════════════════════════════════"
echo "  AIS Collector (Rust) — Status"
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

# Mount check
echo ""
echo "── Mount Health ───────────────────────────────────────────────"
if mountpoint -q /mnt/wdblack 2>/dev/null; then
    echo "  /mnt/wdblack: ✅ MOUNTED"
    DISK_USAGE=$(df -h /mnt/wdblack | tail -1 | awk '{printf "  Used: %s / %s (%s)", $3, $2, $5}')
    echo "$DISK_USAGE"
else
    echo "  /mnt/wdblack: ❌ NOT MOUNTED"
fi

# Output directory stats
echo ""
echo "── Output Queue ───────────────────────────────────────────────"
OUTPUT_DIR="/mnt/wdblack/ais"
if [[ -d "$OUTPUT_DIR" ]]; then
    FILE_COUNT=$(ls "$OUTPUT_DIR"/*.jsonl 2>/dev/null | wc -l)
    DIR_SIZE=$(du -sh "$OUTPUT_DIR" 2>/dev/null | cut -f1)
    echo "  JSONL files:    $FILE_COUNT"
    echo "  Directory size: $DIR_SIZE"
    # Show newest file
    NEWEST=$(ls -t "$OUTPUT_DIR"/*.jsonl 2>/dev/null | head -1)
    if [[ -n "$NEWEST" ]]; then
        echo "  Newest file:    $(basename "$NEWEST")"
    fi
else
    echo "  Output dir not found at $OUTPUT_DIR"
fi

# Log tail
echo ""
echo "── Recent Logs (last 30 lines) ──────────────────────────────"
if [[ -f "$LOG_FILE" ]]; then
    tail -30 "$LOG_FILE"
else
    echo "  No log file found at $LOG_FILE"
fi
