#!/usr/bin/env bash
# ─── Start the AIS Data Lake Ingestor ────────────────────────────────────────
#
# Builds the release binary (if needed) and starts the ingestor daemon.
# Stores the PID in /mnt/wdblack/ais_ingestor.pid for management scripts.
# Logs to /mnt/wdblack/ais_ingestor.log.
# ────────────────────────────────────────────────────────────────────────────

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR"
PID_FILE="/mnt/wdblack/ais_ingestor.pid"
LOG_FILE="/mnt/wdblack/ais_ingestor.log"
BINARY="$PROJECT_DIR/target/release/ais_ingestor"

# Check if /mnt/wdblack is mounted
if ! mountpoint -q /mnt/wdblack 2>/dev/null; then
    echo "❌ /mnt/wdblack is not mounted! Cannot start ingestor."
    exit 1
fi

# Check if already running
if [[ -f "$PID_FILE" ]]; then
    OLD_PID=$(cat "$PID_FILE")
    if kill -0 "$OLD_PID" 2>/dev/null; then
        echo "❌ Ingestor is already running (PID $OLD_PID)."
        echo "   Use stop_ingestor.bash to stop it first."
        exit 1
    else
        echo "⚠️  Stale PID file found (PID $OLD_PID not running). Cleaning up."
        rm -f "$PID_FILE"
    fi
fi

# Build release binary
echo "🔨 Building release binary …"
cd "$PROJECT_DIR"
cargo build --release --bin ais_ingestor 2>&1
echo "✅ Build complete."

# Ensure directories exist
mkdir -p /mnt/wdblack/ais
mkdir -p /mnt/wdblack/ais_processed
mkdir -p /mnt/wdblack/trading_lake

# Start the ingestor in the background
# ⭐ ionice -c3 = "idle" I/O class — only uses disk when nothing else needs it
# ⭐ nice -n 10  = lower CPU priority so collector + shell stay responsive
echo "🚀 Starting AIS ingestor daemon (low I/O + CPU priority) …"
RUST_LOG=info nohup ionice -c3 nice -n 10 "$BINARY" >> "$LOG_FILE" 2>&1 &
INGESTOR_PID=$!

echo "$INGESTOR_PID" > "$PID_FILE"
echo "✅ Ingestor started (PID $INGESTOR_PID)."
echo "   PID file: $PID_FILE"
echo "   Log file: $LOG_FILE"
echo ""
echo "   Use 'tail -f $LOG_FILE' to follow logs."
echo "   Use './stop_ingestor.bash' to stop."
