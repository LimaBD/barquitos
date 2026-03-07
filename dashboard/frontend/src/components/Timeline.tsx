
import { useRef, useEffect, useCallback, useState } from 'react';
import type { TimeRange, CoverageBucket } from '../types';
import { formatDateTime, formatTime, formatDate, clamp } from '../utils';

interface Props {
  timeRange: TimeRange | null;
  coverage: CoverageBucket[];
  currentTs: number;
  playing: boolean;
  speed: number;
  speedLabel: string;
  mode: 'realtime' | 'playback';
  onSeek: (ts: number) => void;
  onTogglePlay: () => void;
  onSkip: (seconds: number) => void;
  onCycleSpeed: () => void;
}

/**
 * Dahua-style timeline bar.
 *
 * - Green segments = time buckets with AIS data
 * - Grey/dark segments = no data
 * - Red playhead = current timestamp
 * - Drag to pan, scroll to zoom, click to seek
 */
export default function Timeline({
  timeRange,
  coverage,
  currentTs,
  playing,
  speed,
  speedLabel,
  mode,
  onSeek,
  onTogglePlay,
  onSkip,
  onCycleSpeed,
}: Props) {
  const barAreaRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);

  // View window: the visible portion of the timeline
  // [viewStart, viewEnd] in epoch seconds
  const [viewStart, setViewStart] = useState(0);
  const [viewEnd, setViewEnd] = useState(0);

  // Dragging state
  const dragRef = useRef<{ startX: number; startViewStart: number; startViewEnd: number } | null>(null);

  // Coverage as a Set for quick lookup
  const coverageMapRef = useRef<Map<number, number>>(new Map());

  // Build coverage map
  useEffect(() => {
    const m = new Map<number, number>();
    for (const b of coverage) {
      m.set(b.bucket_start, b.cnt);
    }
    coverageMapRef.current = m;
  }, [coverage]);

  // Init view window when timeRange loads
  useEffect(() => {
    if (!timeRange) return;
    // Show last 24h by default, or full range if less
    const span = timeRange.ts_max - timeRange.ts_min;
    const windowSize = Math.min(span, 24 * 3600);
    setViewStart(timeRange.ts_max - windowSize);
    setViewEnd(timeRange.ts_max);
  }, [timeRange]);

  // Keep playhead visible: auto-scroll when playing
  useEffect(() => {
    if (!playing || mode !== 'playback') return;
    if (currentTs > viewEnd - (viewEnd - viewStart) * 0.05) {
      const span = viewEnd - viewStart;
      setViewStart(currentTs - span * 0.1);
      setViewEnd(currentTs - span * 0.1 + span);
    }
  }, [currentTs, playing, mode, viewStart, viewEnd]);

  // ── Draw canvas ──────────────────────────────────────────────────
  const draw = useCallback(() => {
    const canvas = canvasRef.current;
    const area = barAreaRef.current;
    if (!canvas || !area || !timeRange) return;

    const rect = area.getBoundingClientRect();
    const W = rect.width;
    const H = rect.height;

    // Handle high-DPI
    const dpr = window.devicePixelRatio || 1;
    canvas.width = W * dpr;
    canvas.height = H * dpr;
    canvas.style.width = `${W}px`;
    canvas.style.height = `${H}px`;

    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.scale(dpr, dpr);

    const viewSpan = viewEnd - viewStart;
    if (viewSpan <= 0) return;

    // Clear
    ctx.fillStyle = '#0f172a';
    ctx.fillRect(0, 0, W, H);

    // Determine bucket size to draw (adaptive)
    const bucketSize = 3600; // 1 hour buckets from the API
    const pxPerSec = W / viewSpan;

    // Draw coverage bars
    const covMap = coverageMapRef.current;
    const firstBucket = Math.floor(viewStart / bucketSize) * bucketSize;
    const lastBucket = Math.ceil(viewEnd / bucketSize) * bucketSize;

    for (let bs = firstBucket; bs <= lastBucket; bs += bucketSize) {
      const x = (bs - viewStart) * pxPerSec;
      const w = bucketSize * pxPerSec;
      if (x + w < 0 || x > W) continue;

      const cnt = covMap.get(bs) || 0;
      if (cnt > 0) {
        // Green bar — height proportional to log(count), min 40% of bar
        const maxH = H - 14;
        const h = Math.max(maxH * 0.4, maxH * Math.min(1, Math.log10(cnt + 1) / 4));
        ctx.fillStyle = 'rgba(34, 197, 94, 0.6)';
        ctx.fillRect(x, H - h, w - 0.5, h);
        // Brighter top edge
        ctx.fillStyle = 'rgba(34, 197, 94, 0.9)';
        ctx.fillRect(x, H - h, w - 0.5, 2);
      } else {
        // Dark empty area
        ctx.fillStyle = '#1e293b';
        ctx.fillRect(x, 14, w - 0.5, H - 14);
      }
    }

    // Draw time ticks
    const tickInterval = getTickInterval(viewSpan, W);
    const firstTick = Math.ceil(viewStart / tickInterval) * tickInterval;
    ctx.fillStyle = '#64748b';
    ctx.font = '9px Inter, system-ui, sans-serif';
    ctx.textAlign = 'center';

    for (let t = firstTick; t <= viewEnd; t += tickInterval) {
      const x = (t - viewStart) * pxPerSec;
      if (x < 0 || x > W) continue;

      // Tick line
      ctx.strokeStyle = 'rgba(100, 116, 139, 0.3)';
      ctx.beginPath();
      ctx.moveTo(x, 12);
      ctx.lineTo(x, H);
      ctx.stroke();

      // Label
      const label = tickInterval >= 86400 ? formatDate(t) : formatTime(t);
      ctx.fillStyle = '#64748b';
      ctx.fillText(label, x, 10);
    }

    // Draw playhead
    const phX = (currentTs - viewStart) * pxPerSec;
    if (phX >= -5 && phX <= W + 5) {
      ctx.strokeStyle = '#ef4444';
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(phX, 0);
      ctx.lineTo(phX, H);
      ctx.stroke();

      // Playhead dot
      ctx.fillStyle = '#ef4444';
      ctx.beginPath();
      ctx.arc(phX, 6, 5, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = '#fff';
      ctx.beginPath();
      ctx.arc(phX, 6, 2, 0, Math.PI * 2);
      ctx.fill();
    }
  }, [viewStart, viewEnd, currentTs, timeRange, coverage]);

  useEffect(() => {
    draw();
  }, [draw]);

  // Redraw on resize
  useEffect(() => {
    const handler = () => draw();
    window.addEventListener('resize', handler);
    return () => window.removeEventListener('resize', handler);
  }, [draw]);

  // ── Mouse interaction ────────────────────────────────────────────
  const tsFromX = useCallback((clientX: number): number => {
    const area = barAreaRef.current;
    if (!area) return 0;
    const rect = area.getBoundingClientRect();
    const fraction = (clientX - rect.left) / rect.width;
    return viewStart + fraction * (viewEnd - viewStart);
  }, [viewStart, viewEnd]);

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    dragRef.current = {
      startX: e.clientX,
      startViewStart: viewStart,
      startViewEnd: viewEnd,
    };
  }, [viewStart, viewEnd]);

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    if (!dragRef.current || !timeRange) return;
    const dx = e.clientX - dragRef.current.startX;
    const area = barAreaRef.current;
    if (!area) return;
    const pxSpan = area.getBoundingClientRect().width;
    const viewSpan = dragRef.current.startViewEnd - dragRef.current.startViewStart;
    const dtSec = -(dx / pxSpan) * viewSpan;

    let newStart = dragRef.current.startViewStart + dtSec;
    let newEnd = dragRef.current.startViewEnd + dtSec;

    // Clamp
    if (newStart < timeRange.ts_min) {
      newStart = timeRange.ts_min;
      newEnd = newStart + viewSpan;
    }
    if (newEnd > timeRange.ts_max + 3600) {
      newEnd = timeRange.ts_max + 3600;
      newStart = newEnd - viewSpan;
    }

    setViewStart(newStart);
    setViewEnd(newEnd);
  }, [timeRange]);

  const handleMouseUp = useCallback((e: React.MouseEvent) => {
    if (!dragRef.current) return;
    const dx = Math.abs(e.clientX - dragRef.current.startX);
    // If it was a click (not a drag), seek to that position
    if (dx < 4) {
      const ts = tsFromX(e.clientX);
      if (timeRange) {
        onSeek(clamp(Math.round(ts), timeRange.ts_min, timeRange.ts_max));
      }
    }
    dragRef.current = null;
  }, [tsFromX, onSeek, timeRange]);

  const handleWheel = useCallback((e: React.WheelEvent) => {
    if (!timeRange || !barAreaRef.current) return;
    e.preventDefault();

    const area = barAreaRef.current;
    const rect = area.getBoundingClientRect();
    const mouseX = e.clientX - rect.left;
    const fraction = mouseX / rect.width;

    const viewSpan = viewEnd - viewStart;
    const zoomFactor = e.deltaY > 0 ? 1.25 : 0.8;
    const newSpan = clamp(viewSpan * zoomFactor, 60, (timeRange.ts_max - timeRange.ts_min) + 7200);

    // Keep the point under the mouse fixed
    const pivot = viewStart + fraction * viewSpan;
    let newStart = pivot - fraction * newSpan;
    let newEnd = pivot + (1 - fraction) * newSpan;

    // Clamp
    if (newStart < timeRange.ts_min - 3600) newStart = timeRange.ts_min - 3600;
    if (newEnd > timeRange.ts_max + 3600) newEnd = timeRange.ts_max + 3600;

    setViewStart(newStart);
    setViewEnd(newEnd);
  }, [viewStart, viewEnd, timeRange]);

  return (
    <div className="timeline-container">
      {/* Transport controls */}
      <div className="timeline-controls">
        <div className="current-time">
          {timeRange ? formatDateTime(currentTs) : '—'}
        </div>

        {/* Skip back 1h */}
        <button className="transport-btn" title="Skip back 1 hour" onClick={() => onSkip(-3600)}>
          ⏪
        </button>
        {/* Skip back 5m */}
        <button className="transport-btn" title="Skip back 5 min" onClick={() => onSkip(-300)}>
          ◀◀
        </button>
        {/* Skip back 1m */}
        <button className="transport-btn" title="Skip back 1 min" onClick={() => onSkip(-60)}>
          ◀
        </button>
        {/* Play / Pause */}
        <button
          className={`transport-btn ${playing ? 'playing' : ''}`}
          title={playing ? 'Pause' : 'Play'}
          onClick={onTogglePlay}
        >
          {playing ? '⏸' : '▶'}
        </button>
        {/* Skip fwd 1m */}
        <button className="transport-btn" title="Skip forward 1 min" onClick={() => onSkip(60)}>
          ▶
        </button>
        {/* Skip fwd 5m */}
        <button className="transport-btn" title="Skip forward 5 min" onClick={() => onSkip(300)}>
          ▶▶
        </button>
        {/* Skip fwd 1h */}
        <button className="transport-btn" title="Skip forward 1 hour" onClick={() => onSkip(3600)}>
          ⏩
        </button>

        {/* Speed */}
        <button className="transport-btn" title="Playback speed" onClick={onCycleSpeed}>
          ⏱
        </button>
        <span className="speed-display">{speedLabel}</span>

        {/* Zoom controls */}
        <div className="zoom-controls">
          <button className="transport-btn" title="Zoom in timeline" onClick={() => {
            if (!timeRange) return;
            const span = viewEnd - viewStart;
            const center = (viewStart + viewEnd) / 2;
            const newSpan = Math.max(60, span * 0.6);
            setViewStart(center - newSpan / 2);
            setViewEnd(center + newSpan / 2);
          }}>
            ➕
          </button>
          <button className="transport-btn" title="Zoom out timeline" onClick={() => {
            if (!timeRange) return;
            const span = viewEnd - viewStart;
            const center = (viewStart + viewEnd) / 2;
            const maxSpan = timeRange.ts_max - timeRange.ts_min + 7200;
            const newSpan = Math.min(maxSpan, span * 1.6);
            setViewStart(center - newSpan / 2);
            setViewEnd(center + newSpan / 2);
          }}>
            ➖
          </button>
          <button className="transport-btn" title="Fit all" onClick={() => {
            if (!timeRange) return;
            setViewStart(timeRange.ts_min);
            setViewEnd(timeRange.ts_max + 3600);
          }}>
            ⬜
          </button>
        </div>
      </div>

      {/* Canvas timeline bar */}
      <div
        className="timeline-bar-area"
        ref={barAreaRef}
        onMouseDown={handleMouseDown}
        onMouseMove={handleMouseMove}
        onMouseUp={handleMouseUp}
        onMouseLeave={() => { dragRef.current = null; }}
        onWheel={handleWheel}
      >
        <canvas ref={canvasRef} style={{ position: 'absolute', top: 0, left: 0 }} />
      </div>
    </div>
  );
}

/** Choose a nice tick interval based on visible span and pixel width */
function getTickInterval(spanSec: number, widthPx: number): number {
  const targetTicks = widthPx / 100; // ~1 tick every 100px
  const raw = spanSec / targetTicks;

  const intervals = [
    60, 300, 600, 900, 1800, 3600,
    7200, 10800, 21600, 43200, 86400,
    172800, 604800, 2592000,
  ];

  for (const iv of intervals) {
    if (iv >= raw) return iv;
  }
  return intervals[intervals.length - 1];
}
