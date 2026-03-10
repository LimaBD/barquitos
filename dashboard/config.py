"""Dashboard configuration."""

import os

# Parquet Data Lake root path
LAKE_ROOT = os.environ.get(
    "LAKE_ROOT",
    os.path.join("/mnt", "wdblack", "trading_lake"),
)

# Server settings
HOST = os.environ.get("DASHBOARD_HOST", "0.0.0.0")
PORT = int(os.environ.get("DASHBOARD_PORT", "8080"))
