"""
Thin wrapper around the SQLite database.

Provides helpers for:
  - Creating / migrating the schema
  - Getting a connection with recommended pragmas for RPi performance
"""

import sqlite3
from pathlib import Path

from schema import SCHEMA_SQL


def get_connection(db_path: str) -> sqlite3.Connection:
    """
    Return a connection with pragmas tuned for a Raspberry Pi 3B:

    - WAL mode:        allows concurrent reads while writing
    - synchronous OFF: much faster writes (acceptable — raw JSONL is our backup)
    - journal_size_limit: keep WAL file from growing unbounded
    - cache_size:      ~16 MB — fits easily in 1 GB RAM
    - temp_store MEMORY: avoids temp I/O on slow SD card
    """
    Path(db_path).parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(db_path)
    conn.execute("PRAGMA journal_mode = WAL;")
    conn.execute("PRAGMA synchronous  = NORMAL;")
    conn.execute("PRAGMA journal_size_limit = 67108864;")   # 64 MB
    conn.execute("PRAGMA cache_size   = -16000;")           # 16 MB
    conn.execute("PRAGMA temp_store   = MEMORY;")
    conn.execute("PRAGMA foreign_keys = ON;")
    return conn


def init_db(db_path: str) -> sqlite3.Connection:
    """Create schema (idempotent) and return the connection."""
    conn = get_connection(db_path)
    conn.executescript(SCHEMA_SQL)
    return conn
