"""
Configuration for the AIS ingestor.

All paths and settings used by the AIS ingestion script live here.
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

# SQLite database file — stored on the external WD Black drive to avoid
# wearing out the Raspberry Pi's SD card.
DATABASE_PATH = os.path.join("/mnt", "wdblack", "barquitos.db")

# =============================================================================
# Ingestion settings
# =============================================================================

# How many rows to INSERT in a single transaction batch
BATCH_SIZE = 5000

# JSONL filename glob pattern
JSONL_GLOB = "ais_*.jsonl"

# =============================================================================
# Vessel filtering — skip non-commercial vessels
# =============================================================================
# AIS ship-type codes to EXCLUDE.  These are pleasure / recreational /
# non-commercial categories that add noise for trade-nowcasting purposes.
#
# Reference: ITU-R M.1371-5, Table 53
#   30 = Fishing
#   36 = Sailing vessel
#   37 = Pleasure craft / yacht
#
# We keep fishing (30) because it has commercial value.
# Add or remove codes here as needed.
EXCLUDED_SHIP_TYPES: set[int] = {
    36,  # Sailing vessel
    37,  # Pleasure craft / yacht
}
