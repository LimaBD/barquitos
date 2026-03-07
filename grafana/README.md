# 🚢 Grafana Ship Tracker — Debug Dashboard

AIS vessel tracking debug dashboard powered by **database views** — no embedded SQL logic in the dashboard panels.

## Dashboard Layout

### Row 1 — 📈 KPI Overview
| Panel | View used | Description |
|-------|-----------|-------------|
| **Total Vessels** (stat) | `v_total_vessels` | Single number: how many distinct MMSIs in the DB |
| **Total Position Reports** (stat) | `v_total_positions` | Single number: total rows in `position_history` |
| **Moving / Stationary / Total** (stat) | `v_latest_positions` | Split by `speed_threshold` variable |

### Row 2 — 📍 Ship Positions — World Map
| Panel | View used | Description |
|-------|-----------|-------------|
| **World Map** (geomap) | `v_latest_positions` | Two layers: **dots** for stationary, **arrows** for moving (rotated by course, sized by speed). Color-coded by category. Filtered by Ship Category / Ship Type dropdowns. |

### Row 3 — 📊 Fleet Breakdown
| Panel | View used | Description |
|-------|-----------|-------------|
| **Vessels by Category** (donut) | `v_vessels_by_type` | Aggregated by `category` |
| **Vessels by Ship Type** (bar chart) | `v_vessels_by_type` | Top 20 types, horizontal bars |
| **Fleet Breakdown table** | `v_vessels_by_type` | Full sortable table with AIS code, label, category, count and totals footer |

### Row 4 — 📋 Latest Position Reports
| Panel | View used | Description |
|-------|-----------|-------------|
| **Latest Position Reports** (table) | `v_latest_positions` | Sortable table: name, MMSI, type, category, lat, lon, speed, course, heading, timestamp. Filtered by dropdowns. |

### Row 5 — 🔍 Vessel Track Search
| Panel | View used | Description |
|-------|-----------|-------------|
| **Vessel Track — All Positions** (table) | `v_vessel_track` | All historical positions for a vessel. Activated by the `vessel_search` textbox (MMSI, IMO, or partial name match). |
| **Vessel Track — Map** (geomap) | `v_vessel_track` | Map of the searched vessel's track. Points sized by speed, colored orange. Auto-fits to data bounds. |

### Row 6 — 🗄️ Database Health
| Panel | View used | Description |
|-------|-----------|-------------|
| **Database Table Row Counts** (table) | Direct `COUNT(*)` | Row count for every table — quick health check for ingestion |
| **Position Ingestion Timeline** (time series) | Direct query on `position_history` | Bar chart of positions/minute over the last 24h |

### Filters (top of dashboard)

| Variable | Type | Purpose |
|----------|------|---------|
| **Ship Category** | Multi-select query | Populated from `v_vessels_by_type.category` |
| **Ship Type** | Multi-select query | Populated from `v_vessels_by_type.ship_type_label`, cascaded by category |
| **Moving threshold (kn)** | Custom | Speed cutoff for moving vs stationary (default 0.5) |
| **Vessel search** | Textbox | MMSI, IMO, or vessel name — drives the Track panels |

## Database views used

All views are created automatically by `db.init_db()`:

| View | Purpose |
|------|---------|
| `v_total_vessels` | `SELECT COUNT(*) FROM vessels` |
| `v_total_positions` | `SELECT COUNT(*) FROM position_history` |
| `v_vessels_by_type` | Vessel count grouped by ship type (label, category, AIS code) |
| `v_latest_positions` | Most recent position per vessel, joined with vessel info & ship type |
| `v_vessel_track` | All positions for any vessel, joined with vessel info (filter by `mmsi` or `imo_number`) |

## Quick start

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

## Re-importing after changes

```bash
bash import_dashboard.sh
```

The script uses `overwrite: true`, so it safely replaces the existing dashboard.

## Architecture

```
grafana/
├── import_dashboard.sh             # Imports dashboard via Grafana HTTP API
├── dashboards/
│   └── ship-tracker.json           # The main dashboard definition
└── README.md                       # This file
```

System-level provisioning (managed separately):
```
/etc/grafana/provisioning/datasources/barquitos-sqlite.yml   → SQLite datasource
/var/lib/grafana/plugins/frser-sqlite-datasource/            → SQLite plugin
```
