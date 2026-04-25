#!/usr/bin/env bash
# Stop the full AIS pipeline: ingestor → mover → collector (reverse order).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "══ Stopping AIS Pipeline ══"
bash "$SCRIPT_DIR/stop_ingestor.bash"
bash "$SCRIPT_DIR/stop_mover.bash"
bash "$SCRIPT_DIR/stop_collector.bash"
echo "══ All processes stopped ══"
