# Vessel Enrichment Collector

Fetches additional vessel metadata from public APIs to enrich the database with
specifications not available through AIS messages alone.

## Data collected

| Field | Description |
|---|---|
| `gross_tonnage` | Gross tonnage (GT) |
| `net_tonnage` | Net tonnage (NT) |
| `deadweight_tonnage` | Deadweight tonnage (DWT) |
| `year_built` | Year the vessel was built |
| `flag_country` | Country of registration |
| `flag_code` | ISO flag code |
| `class_society` | Classification society |
| `registered_owner` | Registered owner name |
| `operator` | Ship operator |
| `vessel_category` | Detailed vessel category |
| `hull_type` | Hull type |
| `engine_type` | Engine type |
| `max_speed_knots` | Maximum speed in knots |

## Data sources

1. **ITU MARS** — Free, no API key required. MMSI lookup.
2. **VesselFinder** — Public click API. Rate-limited, used as fallback.

## Usage

```bash
# Enrich 50 vessels (one-shot):
python vessel_enrichment/enrich.py --batch-size 50

# Run continuously (enrich 50 every 5 minutes):
python vessel_enrichment/enrich.py --watch --batch-size 50 --interval 300

# Custom database path:
python vessel_enrichment/enrich.py --db /path/to/barquitos.db
```

## Notes

- Vessels are processed in order of most recently seen first.
- Already-enriched vessels are skipped (safe to re-run).
- Vessels where no data was found are marked so they aren't retried immediately.
- Rate limit: default 2-second delay between API calls (`--delay` flag).
