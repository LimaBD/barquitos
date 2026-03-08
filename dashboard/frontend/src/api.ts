import type {
  TimeRange,
  CoverageBucket,
  VesselPosition,
  TrackPoint,
  VesselDetail,
  SearchResult,
  DbMetrics,
  ShipCategoryInfo,
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
  return get(url);
}

export interface LatestPositionsParams {
  shipTypes?: number[];
  categories?: string[];
  bounds?: { minLat: number; maxLat: number; minLon: number; maxLon: number };
  limit?: number;
}

export function fetchLatestPositions(params?: LatestPositionsParams): Promise<VesselPosition[]> {
  const searchParams = new URLSearchParams();
  if (params?.shipTypes?.length) {
    searchParams.set('ship_types', params.shipTypes.join(','));
  }
  if (params?.categories?.length) {
    searchParams.set('categories', params.categories.join(','));
  }
  if (params?.bounds) {
    searchParams.set('min_lat', String(params.bounds.minLat));
    searchParams.set('max_lat', String(params.bounds.maxLat));
    searchParams.set('min_lon', String(params.bounds.minLon));
    searchParams.set('max_lon', String(params.bounds.maxLon));
  }
  if (params?.limit) {
    searchParams.set('limit', String(params.limit));
  }
  const qs = searchParams.toString();
  return get(`${BASE}/latest-positions${qs ? '?' + qs : ''}`);
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

export function fetchDbMetrics(): Promise<DbMetrics> {
  return get(`${BASE}/db-metrics`);
}

export function fetchShipCategories(): Promise<ShipCategoryInfo[]> {
  return get(`${BASE}/ship-categories`);
}
