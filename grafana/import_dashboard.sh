#!/usr/bin/env bash
# -------------------------------------------------------------------
# Import the ship-tracker dashboard into the native Grafana instance
# via the HTTP API.
#
# Usage:
#   bash import_dashboard.sh [GRAFANA_URL] [USER:PASS]
#
# Defaults:
#   GRAFANA_URL = http://localhost:3000
#   USER:PASS   = admin:admin
# -------------------------------------------------------------------
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DASHBOARD_JSON="${SCRIPT_DIR}/dashboards/ship-tracker.json"
TMPFILE=$(mktemp /tmp/grafana_import_XXXXXX.json)
trap 'rm -f "${TMPFILE}"' EXIT

GRAFANA_URL="${1:-http://localhost:3000}"
CREDENTIALS="${2:-admin:admin}"

if [[ ! -f "${DASHBOARD_JSON}" ]]; then
    echo "❌ Dashboard JSON not found: ${DASHBOARD_JSON}"
    exit 1
fi

echo "📡 Importing dashboard into ${GRAFANA_URL} ..."

# Wrap the dashboard JSON in the import API envelope and write to temp file
python3 << PYEOF
import json
with open("${DASHBOARD_JSON}") as f:
    dash = json.load(f)
dash["id"] = None
envelope = {
    "dashboard": dash,
    "overwrite": True,
    "message": "Imported by import_dashboard.sh"
}
with open("${TMPFILE}", "w") as f:
    json.dump(envelope, f)
PYEOF

HTTP_CODE=$(curl -s -o /tmp/grafana_import_response.json -w "%{http_code}" \
    -X POST \
    -H "Content-Type: application/json" \
    -u "${CREDENTIALS}" \
    -d @"${TMPFILE}" \
    "${GRAFANA_URL}/api/dashboards/db")

RESPONSE=$(cat /tmp/grafana_import_response.json)

if [[ "${HTTP_CODE}" == "200" ]]; then
    DASH_URL=$(echo "${RESPONSE}" | python3 -c "import json,sys; print(json.load(sys.stdin).get('url',''))" 2>/dev/null)
    echo "✅ Dashboard imported successfully!"
    echo "🔗 Open: ${GRAFANA_URL}${DASH_URL}"
else
    echo "❌ Import failed (HTTP ${HTTP_CODE}):"
    echo "${RESPONSE}" | python3 -m json.tool 2>/dev/null || echo "${RESPONSE}"
    exit 1
fi
