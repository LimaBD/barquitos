#!/usr/bin/env bash
# ─── Debug: ingest a single JSONL file with verbose output ───────────────────
#
# Usage:
#   ./run_debug.bash <path_to_jsonl>
#   ./run_debug.bash                     # defaults to first file in test_data/
#
# Builds the debug binary, processes one file, writes Parquet, moves the
# file to ais_processed/, prints a summary, and exits.
# ────────────────────────────────────────────────────────────────────────────

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$SCRIPT_DIR/target/debug/ais_ingestor"

# Pick the file to process
if [[ $# -ge 1 ]]; then
    FILE="$1"
else
    FILE=$(find "$SCRIPT_DIR/test_data" -name '*.jsonl' | sort | head -1)
    if [[ -z "$FILE" ]]; then
        echo "❌ No JSONL file found in test_data/ and none provided as argument."
        exit 1
    fi
    echo "ℹ️  No file specified — using: $FILE"
fi

if [[ ! -f "$FILE" ]]; then
    echo "❌ File not found: $FILE"
    exit 1
fi

# Build debug binary
echo "🔨 Building debug binary …"
cargo build --bin ais_ingestor --manifest-path "$SCRIPT_DIR/Cargo.toml"

echo "🚀 Running: $BINARY --debug-file $FILE"
"$BINARY" --debug-file "$FILE"
