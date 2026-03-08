export interface TimeRange {
  ts_min: number;
  ts_max: number;
}

export interface CoverageBucket {
  bucket_start: number;
  cnt: number;
}

export interface VesselPosition {
  mmsi: number;
  ship_name: string | null;
  ship_type: number | null;
  ship_type_label: string;
  category: string;
  timestamp_utc: number;
  latitude: number;
  longitude: number;
  speed_over_ground: number | null;
  course_over_ground: number | null;
  true_heading: number | null;
  diff_seconds?: number;
}

export interface TrackPoint {
  timestamp_utc: number;
  latitude: number;
  longitude: number;
  speed_over_ground: number | null;
  course_over_ground: number | null;
  true_heading: number | null;
}

export interface VesselDetail {
  vessel: {
    mmsi: number;
    ship_name: string | null;
    call_sign: string | null;
    imo_number: number | null;
    ship_type: number | null;
    ship_type_label: string;
    category: string;
    length_m: number | null;
    beam_m: number | null;
    max_draught: number | null;
    first_seen_utc: number;
    last_seen_utc: number;
  };
  destinations: Array<{
    destination: string | null;
    estimated_arrival: number | null;
    timestamp_utc: number;
  }>;
  nav_statuses: Array<{
    navigational_status: number;
    label: string;
    timestamp_utc: number;
  }>;
  draughts: Array<{
    draught: number;
    timestamp_utc: number;
  }>;
  position_count: number;
}

export interface SearchResult {
  mmsi: number;
  ship_name: string | null;
  call_sign: string | null;
  imo_number: number | null;
  ship_type_label: string;
  category: string;
}

export interface LoadingStep {
  label: string;
  status: 'pending' | 'running' | 'done' | 'error';
  detail?: string;
  durationMs?: number;
}

export interface ShipCategoryInfo {
  category: string;
  ship_type_code: number | null;
  ship_type_label: string;
  vessel_count: number;
}

export interface DbMetrics {
  total_vessels: number;
  total_positions: number;
  total_destinations: number;
  total_draughts: number;
  total_nav_statuses: number;
  total_ship_names: number;
  vessels_by_type: Array<{
    ship_type_label: string;
    category: string;
    ship_type_code: number | null;
    vessel_count: number;
  }>;
  vessels_by_category: Array<{
    category: string;
    vessel_count: number;
  }>;
  time_range: {
    ts_min: number;
    ts_max: number;
  } | null;
  positions_per_day: Array<{
    day: string;
    cnt: number;
  }>;
  db_size_bytes: number;
  table_sizes: Array<{
    table: string;
    row_count: number;
  }>;
  top_vessels: Array<{
    mmsi: number;
    ship_name: string | null;
    ship_type_label: string;
    category: string;
    position_count: number;
  }>;
}
