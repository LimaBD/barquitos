
import { useEffect, useRef, useCallback } from 'react';
import L from 'leaflet';
import 'leaflet/dist/leaflet.css';
import type { VesselPosition, TrackPoint } from '../types';
import { getCategoryColor, shipSvg } from '../utils';

interface Props {
  positions: VesselPosition[];
  selectedMmsi: number | null;
  vesselTrack: TrackPoint[];
  onSelectVessel: (mmsi: number) => void;
}

export default function ShipMap({ positions, selectedMmsi, vesselTrack, onSelectVessel }: Props) {
  const mapRef = useRef<L.Map | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const markersRef = useRef<L.LayerGroup>(L.layerGroup());
  const trackLayerRef = useRef<L.LayerGroup>(L.layerGroup());

  // ── Initialise map ───────────────────────────────────────────────
  useEffect(() => {
    if (mapRef.current || !containerRef.current) return;

    const map = L.map(containerRef.current, {
      center: [20, 0],
      zoom: 3,
      zoomControl: true,
      attributionControl: false,
      maxBounds: [[-90, -360], [90, 360]],
      maxBoundsViscosity: 1.0,
    });

    // Dark tile layer
    L.tileLayer('https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}{r}.png', {
      subdomains: 'abcd',
      maxZoom: 18,
    }).addTo(map);

    markersRef.current.addTo(map);
    trackLayerRef.current.addTo(map);

    mapRef.current = map;

    // Fix tile rendering
    setTimeout(() => map.invalidateSize(), 100);

    return () => {
      map.remove();
      mapRef.current = null;
    };
  }, []);

  // ── Remove CSS filter since we use a dark tile layer ─────────────
  // The CSS .leaflet-tile-pane filter is a fallback; CartoDB dark tiles
  // are already dark, so disable the filter for them:
  useEffect(() => {
    const style = document.createElement('style');
    style.textContent = '.leaflet-tile-pane { filter: none !important; }';
    document.head.appendChild(style);
    return () => { document.head.removeChild(style); };
  }, []);

  // ── Update markers when positions change ─────────────────────────
  useEffect(() => {
    const layer = markersRef.current;
    layer.clearLayers();

    for (const p of positions) {
      const color = getCategoryColor(p.category);
      const isSelected = p.mmsi === selectedMmsi;
      const size = isSelected ? 18 : 10;
      const icon = L.divIcon({
        html: shipSvg(p.true_heading ?? p.course_over_ground, isSelected ? '#ff4444' : color, size),
        className: 'ship-marker',
        iconSize: [size, size],
        iconAnchor: [size / 2, size / 2],
      });

      const marker = L.marker([p.latitude, p.longitude], { icon })
        .bindTooltip(
          `<div class="tooltip-name">${p.ship_name || 'Unknown'}</div>
           <div class="tooltip-meta">MMSI: ${p.mmsi}<br/>
           ${p.ship_type_label} · ${p.category}<br/>
           SOG: ${p.speed_over_ground ?? '—'} kn · COG: ${p.course_over_ground ?? '—'}°</div>`,
          { className: 'ship-tooltip', direction: 'top', offset: [0, -8] }
        );

      marker.on('click', () => onSelectVessel(p.mmsi));
      layer.addLayer(marker);
    }
  }, [positions, selectedMmsi, onSelectVessel]);

  // ── Draw track for selected vessel ───────────────────────────────
  useEffect(() => {
    const layer = trackLayerRef.current;
    layer.clearLayers();

    if (vesselTrack.length < 2) return;

    const latlngs: L.LatLngExpression[] = vesselTrack.map(p => [p.latitude, p.longitude]);

    // Gradient-ish track: main line
    const trackLine = L.polyline(latlngs, {
      color: '#38bdf8',
      weight: 2.5,
      opacity: 0.8,
      dashArray: '6 4',
    });
    layer.addLayer(trackLine);

    // Start marker (green)
    const startPt = vesselTrack[0];
    L.circleMarker([startPt.latitude, startPt.longitude], {
      radius: 6,
      fillColor: '#22c55e',
      fillOpacity: 1,
      color: '#fff',
      weight: 2,
    }).bindTooltip('Track start', { className: 'ship-tooltip', direction: 'top' })
      .addTo(layer);

    // End marker (red)
    const endPt = vesselTrack[vesselTrack.length - 1];
    L.circleMarker([endPt.latitude, endPt.longitude], {
      radius: 6,
      fillColor: '#ef4444',
      fillOpacity: 1,
      color: '#fff',
      weight: 2,
    }).bindTooltip('Track end', { className: 'ship-tooltip', direction: 'top' })
      .addTo(layer);

    // Fit map bounds to track
    if (mapRef.current) {
      mapRef.current.fitBounds(trackLine.getBounds().pad(0.15), { maxZoom: 12 });
    }
  }, [vesselTrack]);

  return (
    <div
      ref={containerRef}
      style={{ width: '100%', height: '100%' }}
    />
  );
}
