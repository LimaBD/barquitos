import { useState, useEffect, useCallback, useRef } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import {
  fetchTimeRange,
  fetchCoverage,
  fetchPositionsAt,
  fetchLatestPositions,
  fetchVesselDetail,
  fetchVesselTrack,
} from '../api';
import type {
  TimeRange,
  CoverageBucket,
  VesselPosition,
  TrackPoint,
  VesselDetail,
  LoadingStep,
} from '../types';
import ShipMap from './ShipMap';
import Timeline from './Timeline';
import VesselPanel from './VesselPanel';
import SearchBar from './SearchBar';
import LoadingScreen from './LoadingScreen';
import { formatDateTime } from '../utils';

type Mode = 'realtime' | 'playback';

export default function MapPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();

  // Read category filters from URL
  const selectedCategories = searchParams.get('categories')?.split(',').filter(Boolean) ?? [];

  // ── global state ─────────────────────────────────────────────────
  const [mode, setMode] = useState<Mode>('realtime');
  const [loading, setLoading] = useState(true);
  const [loadingSteps, setLoadingSteps] = useState<LoadingStep[]>([
    { label: 'Connecting to server', status: 'pending' },
    { label: 'Loading time range', status: 'pending' },
    { label: 'Loading timeline coverage', status: 'pending', detail: 'Scanning position reports…' },
    { label: 'Loading vessel positions', status: 'pending', detail: `Filtered: ${selectedCategories.join(', ') || 'all'} (max 1,000)` },
    { label: 'Rendering map', status: 'pending' },
  ]);

  // time range of data
  const [timeRange, setTimeRange] = useState<TimeRange | null>(null);
  // coverage buckets for timeline green/grey
  const [coverage, setCoverage] = useState<CoverageBucket[]>([]);

  // current playback timestamp (epoch seconds)
  const [currentTs, setCurrentTs] = useState<number>(0);
  // playing
  const [playing, setPlaying] = useState(false);
  // playback speed multiplier
  const [speed, setSpeed] = useState(1);

  // vessel positions on map
  const [positions, setPositions] = useState<VesselPosition[]>([]);

  // selected vessel
  const [selectedMmsi, setSelectedMmsi] = useState<number | null>(null);
  const [vesselDetail, setVesselDetail] = useState<VesselDetail | null>(null);
  const [vesselTrack, setVesselTrack] = useState<TrackPoint[]>([]);

  // total counts
  const [totalVessels, setTotalVessels] = useState(0);

  // Map bounds for viewport-based refresh
  const mapBoundsRef = useRef<{ minLat: number; maxLat: number; minLon: number; maxLon: number } | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  // Refs for playback interval
  const playRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const currentTsRef = useRef(currentTs);
  const speedRef = useRef(speed);
  const timeRangeRef = useRef(timeRange);

  useEffect(() => { currentTsRef.current = currentTs; }, [currentTs]);
  useEffect(() => { speedRef.current = speed; }, [speed]);
  useEffect(() => { timeRangeRef.current = timeRange; }, [timeRange]);

  // ── helpers ──────────────────────────────────────────────────────
  const loadLatestWithFilters = useCallback(async (bounds?: { minLat: number; maxLat: number; minLon: number; maxLon: number }) => {
    const pos = await fetchLatestPositions({
      categories: selectedCategories.length > 0 ? selectedCategories : undefined,
      bounds: bounds ?? undefined,
      limit: 1000,
    });
    setPositions(pos);
    setTotalVessels(pos.length);
    return pos;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedCategories.join(',')]);

  // ── init ─────────────────────────────────────────────────────────
  useEffect(() => {
    const updateStep = (index: number, patch: Partial<LoadingStep>) => {
      setLoadingSteps(prev => prev.map((s, i) => i === index ? { ...s, ...patch } : s));
    };

    (async () => {
      const t0 = performance.now();

      // Step 0: Connecting
      updateStep(0, { status: 'running' });
      try {
        // Step 1: Time range
        const t1 = performance.now();
        updateStep(0, { status: 'done', durationMs: Math.round(performance.now() - t0) });
        updateStep(1, { status: 'running' });

        const tr = await fetchTimeRange();
        setTimeRange(tr);
        setCurrentTs(tr.ts_max);
        updateStep(1, {
          status: 'done',
          durationMs: Math.round(performance.now() - t1),
          detail: `${new Date(tr.ts_min * 1000).toISOString().slice(0, 10)} → ${new Date(tr.ts_max * 1000).toISOString().slice(0, 10)}`,
        });

        // Step 2: Coverage
        const t2 = performance.now();
        updateStep(2, { status: 'running' });
        const cov = await fetchCoverage();
        setCoverage(cov);
        updateStep(2, {
          status: 'done',
          durationMs: Math.round(performance.now() - t2),
          detail: `${cov.length.toLocaleString()} time buckets loaded`,
        });

        // Step 3: Positions (filtered, max 1000)
        const t3 = performance.now();
        updateStep(3, { status: 'running' });
        const pos = await loadLatestWithFilters();
        updateStep(3, {
          status: 'done',
          durationMs: Math.round(performance.now() - t3),
          detail: `${pos.length.toLocaleString()} vessels on map`,
        });

        // Step 4: Rendering
        const t4 = performance.now();
        updateStep(4, { status: 'running' });
        await new Promise(r => setTimeout(r, 100));
        updateStep(4, {
          status: 'done',
          durationMs: Math.round(performance.now() - t4),
          detail: `Total: ${((performance.now() - t0) / 1000).toFixed(1)}s`,
        });

        await new Promise(r => setTimeout(r, 600));
      } catch (err) {
        console.error('Init error:', err);
        setLoadingSteps(prev => prev.map(s =>
          s.status === 'running' ? { ...s, status: 'error', detail: String(err) } : s
        ));
        await new Promise(r => setTimeout(r, 3000));
      } finally {
        setLoading(false);
      }
    })();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── Refresh positions for current viewport ───────────────────────
  const handleRefreshViewport = useCallback(async () => {
    setRefreshing(true);
    try {
      await loadLatestWithFilters(mapBoundsRef.current ?? undefined);
    } catch (err) {
      console.error('Refresh error:', err);
    } finally {
      setRefreshing(false);
    }
  }, [loadLatestWithFilters]);

  // ── Map bounds changed callback ──────────────────────────────────
  const handleBoundsChange = useCallback((bounds: { minLat: number; maxLat: number; minLon: number; maxLon: number }) => {
    mapBoundsRef.current = bounds;
  }, []);

  // ── mode switch ──────────────────────────────────────────────────
  const switchToRealtime = useCallback(async () => {
    setMode('realtime');
    setPlaying(false);
    setSelectedMmsi(null);
    setVesselDetail(null);
    setVesselTrack([]);
    try {
      await loadLatestWithFilters(mapBoundsRef.current ?? undefined);
      if (timeRange) setCurrentTs(timeRange.ts_max);
    } catch (err) {
      console.error(err);
    }
  }, [timeRange, loadLatestWithFilters]);

  const switchToPlayback = useCallback(() => {
    setMode('playback');
    setPlaying(false);
  }, []);

  // ── load positions at ts ─────────────────────────────────────────
  const loadPositionsAt = useCallback(async (ts: number) => {
    try {
      const pos = await fetchPositionsAt(ts, 30);
      setPositions(pos);
      setTotalVessels(pos.length);
    } catch (err) {
      console.error(err);
    }
  }, []);

  // ── when currentTs changes in playback mode, load new positions ──
  const lastFetchTs = useRef(0);
  useEffect(() => {
    if (mode !== 'playback') return;
    if (Math.abs(currentTs - lastFetchTs.current) < 10) return;
    lastFetchTs.current = currentTs;
    loadPositionsAt(currentTs);
  }, [currentTs, mode, loadPositionsAt]);

  // ── playback tick ────────────────────────────────────────────────
  useEffect(() => {
    if (playing && mode === 'playback') {
      playRef.current = setInterval(() => {
        setCurrentTs(prev => {
          const tr = timeRangeRef.current;
          if (!tr) return prev;
          const next = prev + speedRef.current;
          if (next >= tr.ts_max) {
            setPlaying(false);
            return tr.ts_max;
          }
          return next;
        });
      }, 100);
    }
    return () => {
      if (playRef.current) clearInterval(playRef.current);
    };
  }, [playing, mode]);

  // ── seek to timestamp (timeline click) ───────────────────────────
  const handleSeek = useCallback((ts: number) => {
    setCurrentTs(ts);
    if (mode === 'realtime') {
      setMode('playback');
    }
  }, [mode]);

  // ── skip forward / backward ──────────────────────────────────────
  const skip = useCallback((seconds: number) => {
    if (!timeRange) return;
    setCurrentTs(prev => {
      const next = prev + seconds;
      return Math.max(timeRange.ts_min, Math.min(timeRange.ts_max, next));
    });
    if (mode === 'realtime') setMode('playback');
  }, [timeRange, mode]);

  // ── speed control ────────────────────────────────────────────────
  const cycleSpeed = useCallback(() => {
    const speeds = [1, 2, 5, 10, 30, 60, 300, 600];
    const idx = speeds.indexOf(speed);
    const next = idx >= 0 ? speeds[(idx + 1) % speeds.length] : 1;
    setSpeed(next);
  }, [speed]);

  // ── select vessel ────────────────────────────────────────────────
  const selectVessel = useCallback(async (mmsi: number) => {
    setSelectedMmsi(mmsi);
    try {
      const [detail, track] = await Promise.all([
        fetchVesselDetail(mmsi),
        fetchVesselTrack(mmsi),
      ]);
      setVesselDetail(detail);
      setVesselTrack(track);
    } catch (err) {
      console.error(err);
    }
  }, []);

  const closePanel = useCallback(() => {
    setSelectedMmsi(null);
    setVesselDetail(null);
    setVesselTrack([]);
  }, []);

  // ── search ───────────────────────────────────────────────────────
  const handleSearchSelect = useCallback((mmsi: number) => {
    selectVessel(mmsi);
  }, [selectVessel]);

  // ── render ───────────────────────────────────────────────────────
  const speedLabel = speed >= 60 ? `${speed / 60}m/tick` : `${speed}s/tick`;

  return (
    <div className="app">
      {/* Top bar */}
      <div className="topbar">
        <button className="topbar-home-btn" onClick={() => navigate('/')} title="Back to Home">
          🚢
        </button>
        <div className="logo">
          Barquitos<span>Ship Tracker</span>
        </div>

        <SearchBar onSelect={handleSearchSelect} />

        <div className="mode-buttons">
          <button
            className={`mode-btn ${mode === 'realtime' ? 'active' : ''}`}
            onClick={switchToRealtime}
          >
            ● LIVE
          </button>
          <button
            className={`mode-btn ${mode === 'playback' ? 'active' : ''}`}
            onClick={switchToPlayback}
          >
            ▶ PLAYBACK
          </button>
        </div>

        <div className="stats">
          <span>
            Vessels<span className="stat-val">{totalVessels.toLocaleString()}</span>
          </span>
          <span>
            {mode === 'realtime' ? 'LIVE' : formatDateTime(currentTs)}
          </span>
        </div>
      </div>

      {/* Main content */}
      <div className="main-area">
        {loading && (
          <LoadingScreen steps={loadingSteps} />
        )}

        <div className="map-container">
          <ShipMap
            positions={positions}
            selectedMmsi={selectedMmsi}
            vesselTrack={vesselTrack}
            onSelectVessel={selectVessel}
            onBoundsChange={handleBoundsChange}
          />
          <div className="vessel-count-badge">
            <span className="count">{totalVessels}</span> vessels
          </div>
          {/* Refresh viewport button */}
          <button
            className="refresh-viewport-btn"
            onClick={handleRefreshViewport}
            disabled={refreshing}
            title="Refresh vessels in current view"
          >
            {refreshing ? '⟳ Loading…' : '⟳ Refresh View'}
          </button>
          {/* Back to type selector */}
          <button
            className="change-types-btn"
            onClick={() => navigate('/select-types')}
            title="Change ship type filters"
          >
            🔧 Filters
          </button>
        </div>

        {selectedMmsi && vesselDetail && (
          <VesselPanel
            detail={vesselDetail}
            trackLength={vesselTrack.length}
            onClose={closePanel}
          />
        )}
      </div>

      {/* Timeline */}
      <Timeline
        timeRange={timeRange}
        coverage={coverage}
        currentTs={currentTs}
        playing={playing}
        speed={speed}
        speedLabel={speedLabel}
        mode={mode}
        onSeek={handleSeek}
        onTogglePlay={() => setPlaying(p => !p)}
        onSkip={skip}
        onCycleSpeed={cycleSpeed}
      />
    </div>
  );
}
