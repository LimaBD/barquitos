#!/usr/bin/env bash
# ─── Barquitos AIS Pipeline — Install Script ─────────────────────────────────
#
# Installs all binaries, the systemd mount unit, the three service units, and
# the per-crate config files.  Run as root (or with sudo).
#
# Usage:
#   sudo bash scripts/install.sh
#
# What it does:
#   1. Build all binaries in release mode
#   2. Copy binaries to /usr/local/bin/
#   3. Create config dir /etc/barquitos/ and install collector/ingestor/mover.toml
#   4. Create user 'server' if it doesn't exist
#   5. Create data directories and set ownership
#   6. Install systemd units (mount + 3 services)
#   7. Enable and start everything
# ─────────────────────────────────────────────────────────────────────────────

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_DIR="/etc/barquitos"
BIN_DIR="/usr/local/bin"
SYSTEMD_DIR="/etc/systemd/system"
SERVICE_USER="server"

echo "══ Barquitos AIS Pipeline Installer ══"
echo "  Repo: $REPO_DIR"

# ── 0. System dependencies ────────────────────────────────────────────────────
echo ""
echo "▶ Checking system dependencies …"
MISSING_PKGS=()
command -v pkg-config &>/dev/null || MISSING_PKGS+=(pkg-config)
dpkg -s libssl-dev &>/dev/null 2>&1     || MISSING_PKGS+=(libssl-dev)

if [ ${#MISSING_PKGS[@]} -gt 0 ]; then
    echo "  Installing missing packages: ${MISSING_PKGS[*]}"
    apt-get update -qq
    apt-get install -y "${MISSING_PKGS[@]}"
    echo "  Dependencies installed."
else
    echo "  All dependencies present."
fi

# ── 1. Build ──────────────────────────────────────────────────────────────────
echo ""
echo "▶ Building release binaries (this may take a few minutes) …"

# When run via sudo, ~/.cargo/bin is not in PATH and RUSTUP_HOME is wrong.
if [ -n "${SUDO_USER:-}" ]; then
    USER_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6)"
    export CARGO_HOME="$USER_HOME/.cargo"
    export RUSTUP_HOME="$USER_HOME/.rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
fi
if ! command -v cargo &>/dev/null; then
    echo "  ✗ cargo not found. Install Rust: https://rustup.rs"
    exit 1
fi

cd "$REPO_DIR"
cargo build --release --workspace

# ── 2. Install binaries ───────────────────────────────────────────────────────
echo ""
echo "▶ Installing binaries to $BIN_DIR …"
install -m 755 target/release/ais_collector "$BIN_DIR/ais_collector"
install -m 755 target/release/ais_mover     "$BIN_DIR/ais_mover"
install -m 755 target/release/ais_ingestor  "$BIN_DIR/ais_ingestor"
install -m 755 target/release/ais_backfill  "$BIN_DIR/ais_backfill"
echo "  Installed: ais_collector, ais_mover, ais_ingestor, ais_backfill"

# ── 3. Install config files ───────────────────────────────────────────────────
echo ""
echo "▶ Installing config files to $CONFIG_DIR …"
mkdir -p "$CONFIG_DIR"

for name in collector ingestor mover; do
    src="$REPO_DIR/config/${name}.toml"
    dst="$CONFIG_DIR/${name}.toml"
    if [ -f "$dst" ]; then
        echo "  Config already exists: $dst — creating backup …"
        cp "$dst" "${dst}.bak.$(date +%Y%m%d_%H%M%S)"
    fi
    cp "$src" "$dst"
    chmod 640 "$dst"
    echo "  Installed: $dst"
done
echo "  ⚠  Edit files in $CONFIG_DIR before starting services!"

# ── 4. Create service user ────────────────────────────────────────────────────
echo ""
echo "▶ Ensuring user '$SERVICE_USER' exists …"
if ! id -u "$SERVICE_USER" &>/dev/null; then
    useradd --system --no-create-home --shell /usr/sbin/nologin "$SERVICE_USER"
    echo "  Created system user: $SERVICE_USER"
else
    echo "  User $SERVICE_USER already exists."
fi

# ── 5. Create data directories ────────────────────────────────────────────────
echo ""
echo "▶ Creating data directories …"

# System disk directories (collector output + mover quarantine)
LOCAL_AIS_DIR="/home/server/.barquitos/ais"
LOCAL_QUARANTINE="/home/server/.barquitos/ais_quarantine"
mkdir -p "$LOCAL_AIS_DIR" "$LOCAL_QUARANTINE"
chown -R "$SERVICE_USER:$SERVICE_USER" "/home/server/.barquitos"

# WD Black directories (created here for documentation; mount must exist first)
echo "  Note: /mnt/wdblack directories will be created by services on first run."
echo "  Make sure /dev/sda1 is formatted with: sudo mkfs.ext4 /dev/sda1"

# ── 6. Install systemd units ──────────────────────────────────────────────────
echo ""
echo "▶ Installing systemd units …"
cp "$REPO_DIR/scripts/mnt-wdblack.mount"    "$SYSTEMD_DIR/"
cp "$REPO_DIR/scripts/ais-collector.service" "$SYSTEMD_DIR/"
cp "$REPO_DIR/scripts/ais-mover.service"     "$SYSTEMD_DIR/"
cp "$REPO_DIR/scripts/ais-ingestor.service"  "$SYSTEMD_DIR/"
chmod 644 "$SYSTEMD_DIR/mnt-wdblack.mount" \
          "$SYSTEMD_DIR/ais-collector.service" \
          "$SYSTEMD_DIR/ais-mover.service" \
          "$SYSTEMD_DIR/ais-ingestor.service"

systemctl daemon-reload

# ── 7. Enable and start ───────────────────────────────────────────────────────
echo ""
echo "▶ Enabling and starting services …"

# Mount unit first
systemctl enable mnt-wdblack.mount
echo "  Attempting to mount /dev/sda1 → /mnt/wdblack …"
if systemctl start mnt-wdblack.mount; then
    echo "  ✅ WD Black mounted."
else
    echo "  ⚠  Mount failed. Check that /dev/sda1 exists and is formatted."
    echo "     You can start the ingestor and mover manually once the drive is mounted."
fi

# Enable all three services (they will start on next boot)
systemctl enable ais-collector.service
systemctl enable ais-mover.service
systemctl enable ais-ingestor.service

# Try to start them now
echo ""
echo "▶ Starting services …"

if systemctl start ais-collector.service; then
    echo "  ✅ ais-collector started"
else
    echo "  ⚠  ais-collector failed to start (check: journalctl -u ais-collector)"
fi

if systemctl start ais-mover.service; then
    echo "  ✅ ais-mover started"
else
    echo "  ⚠  ais-mover failed to start (check: journalctl -u ais-mover)"
fi

if systemctl start ais-ingestor.service; then
    echo "  ✅ ais-ingestor started"
else
    echo "  ⚠  ais-ingestor failed to start (check: journalctl -u ais-ingestor)"
fi

# ── Summary ───────────────────────────────────────────────────────────────────
echo ""
echo "══ Installation complete ══"
echo ""
echo "Useful commands:"
echo "  sudo systemctl status ais-collector ais-mover ais-ingestor"
echo "  sudo journalctl -u ais-collector -f"
echo "  sudo journalctl -u ais-ingestor -f"
echo "  sudo journalctl -u ais-mover -f"
echo ""
echo "Config dir:   $CONFIG_DIR"
echo "  Edit it any time — no recompile needed, just restart the service:"
echo "  sudo systemctl restart ais-ingestor"
echo ""
echo "Backfill historical data:"
echo "  BARQUITOS_CONFIG=$CONFIG_DIR/ingestor.toml ais_backfill /path/to/jsonl/files"
