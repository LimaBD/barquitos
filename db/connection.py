"""
Thin wrapper around the SQLite database.

Provides helpers for:
  - Creating / migrating the schema
  - Getting a connection with recommended pragmas for RPi performance
"""

import sqlite3
from pathlib import Path

from db.schema import SCHEMA_SQL, SHIP_TYPE_SEED, SEED_SHIP_TYPES_SQL, NAV_STATUS_SEED, SEED_NAV_STATUSES_SQL


def get_connection(db_path: str) -> sqlite3.Connection:
    """
    Return a connection with pragmas tuned for a Raspberry Pi 3B:

    - WAL mode:        allows concurrent reads while writing
    - synchronous NORMAL: good write speed with WAL safety
    - journal_size_limit: keep WAL file from growing unbounded
    - cache_size:      ~16 MB — fits easily in 1 GB RAM
    - temp_store MEMORY: avoids temp I/O on slow SD card
    - mmap_size:       memory-mapped I/O for faster reads
    """
    Path(db_path).parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA journal_mode = WAL;")
    conn.execute("PRAGMA synchronous  = NORMAL;")
    conn.execute("PRAGMA journal_size_limit = 67108864;")   # 64 MB
    conn.execute("PRAGMA cache_size   = -16000;")           # 16 MB
    conn.execute("PRAGMA temp_store   = MEMORY;")
    conn.execute("PRAGMA mmap_size    = 268435456;")        # 256 MB
    conn.execute("PRAGMA foreign_keys = ON;")
    return conn


def init_db(db_path: str) -> sqlite3.Connection:
    """Create schema (idempotent), seed reference data, and return the connection."""
    conn = get_connection(db_path)
    conn.executescript(SCHEMA_SQL)
    # Seed ship-type lookup (INSERT OR IGNORE — safe to run every time)
    conn.executemany(SEED_SHIP_TYPES_SQL, SHIP_TYPE_SEED)
    # Seed navigational-status lookup
    conn.executemany(SEED_NAV_STATUSES_SQL, NAV_STATUS_SEED)
    conn.commit()
    return conn
