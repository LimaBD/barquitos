"""
Configuration for the data processing pipeline.

All paths and settings used by the ingestion scripts and future enrichment
modules live here.
"""

import importlib.util
import os

# ---------------------------------------------------------------------------
# Resolve the AIS collector config so we reuse its DATA_OUTPUT_DIR.
# We load it by file path (importlib) to avoid name clashes with this module.
# ---------------------------------------------------------------------------
_project_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

_spec = importlib.util.spec_from_file_location(
    "ais_collector_config",
    os.path.join(_project_root, "ais_collector", "config.py"),
)
_collector_cfg = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_collector_cfg)

_COLLECTOR_OUTPUT_DIR = _collector_cfg.DATA_OUTPUT_DIR

# =============================================================================
# Paths
# =============================================================================

# Where the AIS collector writes raw JSONL files
AIS_RAW_DIR = _COLLECTOR_OUTPUT_DIR

# After a JSONL file is ingested it is moved here
AIS_BACKUP_DIR = os.path.join(os.path.dirname(AIS_RAW_DIR), "ais_processed")

# SQLite database file
DATABASE_PATH = os.path.join(_project_root, "data_processor", "barquitos.db")

# =============================================================================
# Ingestion settings
# =============================================================================

# How many rows to INSERT in a single transaction batch
BATCH_SIZE = 5000

# JSONL filename glob pattern
JSONL_GLOB = "ais_*.jsonl"
