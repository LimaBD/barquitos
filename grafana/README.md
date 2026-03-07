# 🚢 Grafana Ship Tracker Dashboard

Real-time AIS vessel tracking on a world map, powered by Grafana + SQLite.

## What you get

| Panel | Description |
|-------|-------------|
| **World Map** | Full-screen Geomap — **dots** for stationary ships, **arrows** for moving ships (arrow points in direction of travel, size scales with speed) |
| **Vessels by Category** | Donut chart breakdown (Cargo, Tanker, Passenger, Service, etc.) |
| **Moving vs Stationary** | Quick stat cards |
| **Latest Position Reports** | Sortable table of last-known positions |

### Filters (top of dashboard)

| Variable | Purpose |
|----------|---------|
| **Ship Category** | Multi-select: Cargo, Tanker, Passenger, Service, Fishing, Military, Recreational, Other |
| **Ship Type** | Multi-select: specific AIS ship-type labels, filtered by selected category |
| **Moving threshold** | Speed (knots) above which a ship is considered "moving" (default 0.5 kn) |

## Quick start

The dashboard runs on the **native Grafana server** (systemd service).

### 1. Prerequisites

- Grafana 12+ installed as a system service (`grafana-server`)
- The **frser-sqlite-datasource** plugin:
  ```bash
  sudo grafana-cli plugins install frser-sqlite-datasource
  sudo systemctl restart grafana-server
  ```
- Datasource provisioned at `/etc/grafana/provisioning/datasources/barquitos-sqlite.yml`
- The SQLite database at `/mnt/wdblack/barquitos.db` (created by the AIS ingestor)

### 2. Import the dashboard

```bash
cd /home/msi/Desktop/barquitos/grafana
bash import_dashboard.sh
```

Then open **http://localhost:3000** in your browser.

- **User:** `admin`
- **Password:** *(whatever you set during first Grafana login)*

## Architecture

```
grafana/
├── import_dashboard.sh         # Imports dashboard via Grafana HTTP API
├── dashboards/
│   └── ship-tracker.json       # The main dashboard definition
└── README.md                   # This file
```

System-level provisioning (managed separately):
```
/etc/grafana/provisioning/datasources/barquitos-sqlite.yml   → SQLite datasource
/var/lib/grafana/plugins/frser-sqlite-datasource/            → SQLite plugin
```

## How it works

1. The **frser-sqlite-datasource** plugin connects Grafana to `/mnt/wdblack/barquitos.db`.
2. The `import_dashboard.sh` script imports `ship-tracker.json` via the Grafana HTTP API.
3. The dashboard runs **two separate queries** on the Geomap panel:

### Map queries

- **Stationary layer:** fetches the latest position per vessel where `speed_over_ground < threshold`, renders as **circle** markers (dots).
- **Moving layer:** fetches the latest position per vessel where `speed_over_ground >= threshold`, renders as **arrow** markers rotated by `course_over_ground`.

Both layers are color-coded by ship category and respond to the dashboard filter variables.

## Customization

- **Change the speed threshold:** use the "Moving threshold" dropdown at the top of the dashboard.
- **Change the basemap:** edit the panel → Basemap layer (CartoDB Dark Matter is the default).
- **Add more panels:** edit the JSON or use the Grafana UI, then re-import with `bash import_dashboard.sh`.

## Re-importing after changes

```bash
bash import_dashboard.sh
```

The script uses `overwrite: true`, so it safely replaces the existing dashboard.
