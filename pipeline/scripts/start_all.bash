#!/usr/bin/env bash
# Start the full AIS pipeline: collector → mover → ingestor (in order).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "══ Starting AIS Pipeline ══"
bash "$SCRIPT_DIR/start_collector.bash"
bash "$SCRIPT_DIR/start_mover.bash"
bash "$SCRIPT_DIR/start_ingestor.bash"
echo "══ All processes started ══"
