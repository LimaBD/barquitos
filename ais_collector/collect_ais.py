"""
AISstream.io Raw Data Collector

Connects to the AISstream.io WebSocket API and stores every incoming
message as-is (raw JSON) into JSONL files (one JSON object per line).

Files are rotated every minute or when they reach a configurable message count.
The script automatically reconnects with exponential backoff on failures.

Usage:
    python collect_ais.py

Configuration:
    Edit config.py to set your API key, filters, and output directory.
"""

import asyncio
import json
import logging
import os
import signal
import sys
from datetime import datetime, timezone
from pathlib import Path

import websockets

import config

# ---------------------------------------------------------------------------
# Logging
# ---------------------------------------------------------------------------
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
)
logger = logging.getLogger("ais_collector")

# ---------------------------------------------------------------------------
# Globals
# ---------------------------------------------------------------------------
shutdown_event = asyncio.Event()


def _handle_signal():
    """Set the shutdown event so the main loop exits gracefully."""
    logger.info("Shutdown signal received, finishing up …")
    shutdown_event.set()


# ---------------------------------------------------------------------------
# File writer helper
# ---------------------------------------------------------------------------
class RotatingFileWriter:
    """
    Writes lines to JSONL files, rotating based on the current UTC minute
    and an optional maximum message count per file.
    """

    def __init__(self, output_dir: str, max_messages: int = 0):
        self.output_dir = Path(output_dir)
        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.max_messages = max_messages
        self._file = None
        self._current_minute: str | None = None
        self._file_index = 0
        self._message_count = 0

    def _minute_tag(self) -> str:
        return datetime.now(timezone.utc).strftime("%Y%m%d_%H%M")

    def _open_new_file(self):
        """Close the current file (if any) and open a new one."""
        self._close()
        minute = self._minute_tag()
        if minute != self._current_minute:
            self._current_minute = minute
            self._file_index = 0
        filename = f"ais_{self._current_minute}_{self._file_index:04d}.jsonl"
        filepath = self.output_dir / filename
        # If file already exists (e.g. restart in the same minute), bump index
        while filepath.exists():
            self._file_index += 1
            filename = f"ais_{self._current_minute}_{self._file_index:04d}.jsonl"
            filepath = self.output_dir / filename
        self._file = open(filepath, "a", encoding="utf-8")
        self._message_count = 0
        logger.info("Writing to %s", filepath)

    def write(self, raw_json: str):
        """Write a single raw JSON message (one line)."""
        minute = self._minute_tag()
        needs_rotation = (
            self._file is None
            or minute != self._current_minute
            or (self.max_messages > 0 and self._message_count >= self.max_messages)
        )
        if needs_rotation:
            self._open_new_file()

        # Write the raw JSON as-is, followed by a newline
        self._file.write(raw_json.rstrip("\n") + "\n")
        self._message_count += 1

        # Flush every write to minimise data loss on crash
        self._file.flush()

    def _close(self):
        if self._file is not None:
            self._file.flush()
            self._file.close()
            self._file = None

    def close(self):
        self._close()


# ---------------------------------------------------------------------------
# AIS Stream connection
# ---------------------------------------------------------------------------
def _build_subscription_message(api_key: str) -> str:
    """Build the JSON subscription message to send on connect."""
    msg = {
        "APIKey": api_key,
        "BoundingBoxes": config.BOUNDING_BOXES,
    }
    if config.FILTER_MMSI:
        msg["FiltersShipMMSI"] = config.FILTER_MMSI
    if config.FILTER_MESSAGE_TYPES:
        msg["FilterMessageTypes"] = config.FILTER_MESSAGE_TYPES
    return json.dumps(msg)


async def _collect(api_key: str, writer: RotatingFileWriter):
    """
    Connect to AISstream, subscribe, and write every incoming message
    to disk until shutdown_event is set.
    """
    subscription = _build_subscription_message(api_key)
    logger.info("Connecting to %s …", config.WS_URL)

    async with websockets.connect(
        config.WS_URL,
        open_timeout=60,
        ping_interval=60,
        ping_timeout=60,
        close_timeout=10,
        max_size=2**22,  # 4 MB max message size
    ) as ws:
        logger.info("Connected. Sending subscription …")
        await ws.send(subscription)

        total = 0
        async for raw in ws:
            if shutdown_event.is_set():
                break

            # raw is either str or bytes; normalise to str
            if isinstance(raw, bytes):
                raw = raw.decode("utf-8")

            writer.write(raw)
            total += 1

            if total % config.LOG_EVERY_N_MESSAGES == 0:
                logger.info("Messages received so far: %s", total)

    logger.info("WebSocket closed. Total messages this session: %s", total)


async def run():
    """Main loop with reconnect logic."""
    # Resolve API key: env var takes precedence over config file
    api_key = os.environ.get("AISSTREAM_API_KEY") or config.API_KEY
    if not api_key or api_key == "<YOUR_API_KEY>":
        logger.error(
            "No API key configured. Set AISSTREAM_API_KEY env var or edit config.py."
        )
        sys.exit(1)

    writer = RotatingFileWriter(
        output_dir=config.DATA_OUTPUT_DIR,
        max_messages=config.MAX_MESSAGES_PER_FILE,
    )

    delay = config.RECONNECT_DELAY_SECONDS

    try:
        while not shutdown_event.is_set():
            try:
                await _collect(api_key, writer)
                # If _collect returned normally (shutdown), break
                if shutdown_event.is_set():
                    break
                delay = config.RECONNECT_DELAY_SECONDS  # reset on clean close
            except (
                websockets.exceptions.ConnectionClosed,
                websockets.exceptions.WebSocketException,
                ConnectionError,
                OSError,
            ) as exc:
                logger.warning("Connection lost (%s). Reconnecting in %ds …", exc, delay)
                try:
                    await asyncio.wait_for(shutdown_event.wait(), timeout=delay)
                    break  # shutdown requested during wait
                except asyncio.TimeoutError:
                    pass  # timeout expired, time to reconnect
                delay = min(delay * 2, config.MAX_RECONNECT_DELAY_SECONDS)
            except Exception:
                logger.exception("Unexpected error")
                try:
                    await asyncio.wait_for(shutdown_event.wait(), timeout=delay)
                    break
                except asyncio.TimeoutError:
                    pass
                delay = min(delay * 2, config.MAX_RECONNECT_DELAY_SECONDS)
    finally:
        writer.close()
        logger.info("Writer closed. Bye!")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
def main():
    loop = asyncio.new_event_loop()

    # Graceful shutdown on SIGINT / SIGTERM
    for sig in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(sig, _handle_signal)

    try:
        loop.run_until_complete(run())
    finally:
        loop.close()


if __name__ == "__main__":
    main()
