
import type {
  TimeRange,
  CoverageBucket,
  VesselPosition,
  TrackPoint,
  VesselDetail,
  SearchResult,
} from './types';

const BASE = '/api';

async function get<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
  return res.json();
}

export function fetchTimeRange(): Promise<TimeRange> {
  return get(`${BASE}/time-range`);
}

export function fetchCoverage(bucketSeconds = 3600): Promise<CoverageBucket[]> {
  return get(`${BASE}/coverage?bucket_seconds=${bucketSeconds}`);
}

export function fetchPositionsAt(
  ts: number,
  window = 30,
  shipType?: number,
): Promise<VesselPosition[]> {
  let url = `${BASE}/positions?ts=${ts}&window=${window}`;
  if (shipType != null) url += `&ship_type=${shipType}`;
  return url ? get(url) : get(url);
}

export function fetchLatestPositions(): Promise<VesselPosition[]> {
  return get(`${BASE}/latest-positions`);
}

export function fetchVesselDetail(mmsi: number): Promise<VesselDetail> {
  return get(`${BASE}/vessel/${mmsi}`);
}

export function fetchVesselTrack(
  mmsi: number,
  start?: number,
  end?: number,
): Promise<TrackPoint[]> {
  let url = `${BASE}/vessel/${mmsi}/track`;
  if (start != null && end != null) url += `?start=${start}&end=${end}`;
  return get(url);
}

export function searchVessels(q: string): Promise<SearchResult[]> {
  return get(`${BASE}/search?q=${encodeURIComponent(q)}`);
}
