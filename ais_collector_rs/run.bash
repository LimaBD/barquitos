#!/usr/bin/env bash
# ─── Start the AIS Collector (Rust) ──────────────────────────────────────────
#
# Builds the release binary (if needed) and starts the collector daemon.
# Stores the PID in /mnt/wdblack/ais_collector.pid for management scripts.
# Logs to /mnt/wdblack/ais_collector.log.
# ────────────────────────────────────────────────────────────────────────────

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR"
PID_FILE="/mnt/wdblack/ais_collector.pid"
LOG_FILE="/mnt/wdblack/ais_collector.log"
BINARY="$PROJECT_DIR/target/release/ais_collector"

# Check if /mnt/wdblack is mounted
if ! mountpoint -q /mnt/wdblack 2>/dev/null; then
    echo "❌ /mnt/wdblack is not mounted! Cannot start collector."
    exit 1
fi

# Check if already running
if [[ -f "$PID_FILE" ]]; then
    OLD_PID=$(cat "$PID_FILE")
    if kill -0 "$OLD_PID" 2>/dev/null; then
        echo "❌ Collector is already running (PID $OLD_PID)."
        echo "   Use ./stop.bash to stop it first."
        exit 1
    else
        echo "⚠️  Stale PID file found (PID $OLD_PID not running). Cleaning up."
        rm -f "$PID_FILE"
    fi
fi

# Build release binary
echo "🔨 Building release binary …"
cd "$PROJECT_DIR"
cargo build --release --bin ais_collector 2>&1
echo "✅ Build complete."

# Ensure output directory exists
mkdir -p /mnt/wdblack/ais

# Start the collector in the background
# ⭐ ionice -c2 -n0 = best-effort, highest priority — collector MUST keep up with the stream
# ⭐ No nice adjustment — collector needs responsive CPU for WebSocket processing
echo "🚀 Starting AIS collector daemon (high I/O priority) …"
RUST_LOG=info nohup ionice -c2 -n0 "$BINARY" >> "$LOG_FILE" 2>&1 &
COLLECTOR_PID=$!

echo "$COLLECTOR_PID" > "$PID_FILE"
echo "✅ Collector started (PID $COLLECTOR_PID)."
echo "   PID file: $PID_FILE"
echo "   Log file: $LOG_FILE"
echo ""
echo "   Use 'tail -f $LOG_FILE' to follow logs."
echo "   Use './stop.bash' to stop."
