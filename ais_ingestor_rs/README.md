# AIS Ingestor (Rust)

High-performance Rust port of the Python `ais_ingestor` module. Drop-in
replacement — same CLI flags, same database schema, same file-move behaviour.

## Performance

Benchmarked on the same 22,786-line JSONL file:

| Version | Time  | Speedup |
|---------|-------|---------|
| Python  | 1.23s |   1×    |
| **Rust**| 0.32s | **~4×** |

## Build

```bash
cd ais_ingestor_rs
cargo build --release
```

The binary is at `target/release/ais_ingestor`.

## Usage

```bash
# One-shot: process all pending files then exit
./target/release/ais_ingestor

# Watch mode: keep checking for new files every 30 seconds
./target/release/ais_ingestor --watch

# Custom interval
./target/release/ais_ingestor --watch --interval 60

# Override directories
./target/release/ais_ingestor --input-dir ./data --backup-dir /tmp/processed
```

Or use the convenience script:

```bash
./run.bash   # starts watch-mode in background, logs to /mnt/wdblack/ais_ingestor.log
```

## Configuration

Edit `src/config.rs` and rebuild. Defaults:

| Setting             | Default                          | Description                        |
|---------------------|----------------------------------|------------------------------------|
| `AIS_RAW_DIR`       | `/mnt/wdblack/ais`               | Where JSONL files are written      |
| `AIS_BACKUP_DIR`    | `/mnt/wdblack/ais_processed`     | Where processed files are moved    |
| `DATABASE_PATH`     | `/mnt/wdblack/barquitos.db`      | SQLite database location           |
| `BATCH_SIZE`        | `5000`                           | Rows per INSERT batch              |
| `EXCLUDED_SHIP_TYPES` | `{36, 37}`                     | AIS type codes to skip             |

## Project Structure

```
src/
  main.rs      — CLI entry point (clap)
  config.rs    — constants (paths, batch size, filters)
  db.rs        — schema creation, seed data, connection pragmas
  models.rs    — serde structs for AIS JSON deserialization
  ingest.rs    — parsing, extraction, batch DB writes, file management
```
