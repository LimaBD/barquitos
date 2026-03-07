"""Dashboard configuration."""

import os

# SQLite database path
DATABASE_PATH = os.environ.get(
    "BARQUITOS_DB",
    os.path.join("/mnt", "wdblack", "barquitos.db"),
)

# Server settings
HOST = os.environ.get("DASHBOARD_HOST", "0.0.0.0")
PORT = int(os.environ.get("DASHBOARD_PORT", "8080"))
