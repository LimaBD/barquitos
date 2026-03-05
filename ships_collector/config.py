"""
Configuration for AISstream data collector.

Edit these values before running the collector.
"""

# =============================================================================
# AISstream API Configuration
# =============================================================================

# Your API key from https://aisstream.io/authenticate
# You can also set the environment variable AISSTREAM_API_KEY instead.
API_KEY = "9ce24db073071bf1078b798e22e44bd412191101"

# WebSocket endpoint
WS_URL = "wss://stream.aisstream.io/v0/stream"

# =============================================================================
# Subscription Filters
# =============================================================================

# Bounding boxes to filter by geographic area.
# Format: [[[lat_min, lon_min], [lat_max, lon_max]], ...]
# The default covers the entire world.
BOUNDING_BOXES = [[[-90, -180], [90, 180]]]

# Filter by specific vessel MMSIs (leave empty for all vessels)
# Example: ["538007480", "636015988", "316003701"]
FILTER_MMSI = []

# Filter by message types (leave empty for all message types)
# Available types: "PositionReport", "ShipStaticData", "StandardSearchAndRescueAircraftReport",
#                  "StaticDataReport", "AddressedSafetyRelatedMessage", "Interrogation",
#                  "AssignedModeCommand", "CoordinatedUTCInquiry", "BaseStationReport",
#                  "StandardClassBPositionReport", "ExtendedClassBPositionReport", etc.
FILTER_MESSAGE_TYPES = []

# =============================================================================
# Data Storage Configuration
# =============================================================================

# Path where raw AIS data will be stored.
# A new JSONL file is created every minute (or per file size limit).
DATA_OUTPUT_DIR = "/mnt/wdblack/ais"

# Maximum number of messages per file before rotating to a new file.
# Set to 0 for no limit (will only rotate every minute).
MAX_MESSAGES_PER_FILE = 0

# =============================================================================
# Connection / Resilience
# =============================================================================

# Seconds to wait before reconnecting after a disconnection
RECONNECT_DELAY_SECONDS = 5

# Maximum reconnect delay (exponential backoff cap)
MAX_RECONNECT_DELAY_SECONDS = 300

# Log a status line every N messages received
LOG_EVERY_N_MESSAGES = 1000
