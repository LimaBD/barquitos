import { useState, useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { fetchDbMetrics } from '../api';
import type { DbMetrics, LoadingStep } from '../types';
import LoadingScreen from './LoadingScreen';

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return `${(bytes / Math.pow(1024, i)).toFixed(i > 0 ? 2 : 0)} ${units[i]}`;
}

function formatNumber(n: number | undefined | null): string {
  return (n ?? 0).toLocaleString();
}

function formatDuration(tsMin: number, tsMax: number): string {
  const diff = tsMax - tsMin;
  const days = Math.floor(diff / 86400);
  const hours = Math.floor((diff % 86400) / 3600);
  if (days > 0) return `${days}d ${hours}h`;
  return `${hours}h`;
}

export default function DbMetricsPage() {
  const navigate = useNavigate();
  const [metrics, setMetrics] = useState<DbMetrics | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [loadingSteps, setLoadingSteps] = useState<LoadingStep[]>([
    { label: 'Connecting to server', status: 'pending' },
    { label: 'Querying database metrics', status: 'pending', detail: 'Counting vessels, positions, tables…' },
    { label: 'Rendering dashboard', status: 'pending' },
  ]);

  useEffect(() => {
    const updateStep = (index: number, patch: Partial<LoadingStep>) => {
      setLoadingSteps(prev => prev.map((s, i) => i === index ? { ...s, ...patch } : s));
    };

    (async () => {
      const t0 = performance.now();

      // Step 0: Connecting
      updateStep(0, { status: 'running' });
      try {
        updateStep(0, { status: 'done', durationMs: Math.round(performance.now() - t0) });

        // Step 1: Query metrics
        const t1 = performance.now();
        updateStep(1, { status: 'running' });
        const data = await fetchDbMetrics();
        setMetrics(data);
        updateStep(1, {
          status: 'done',
          durationMs: Math.round(performance.now() - t1),
          detail: `${data.total_vessels.toLocaleString()} vessels · ${data.total_positions.toLocaleString()} positions`,
        });

        // Step 2: Rendering
        const t2 = performance.now();
        updateStep(2, { status: 'running' });
        await new Promise(r => setTimeout(r, 100));
        updateStep(2, {
          status: 'done',
          durationMs: Math.round(performance.now() - t2),
          detail: `Total: ${((performance.now() - t0) / 1000).toFixed(1)}s`,
        });

        await new Promise(r => setTimeout(r, 500));
      } catch (err) {
        setError(String(err));
        setLoadingSteps(prev => prev.map(s =>
          s.status === 'running' ? { ...s, status: 'error', detail: String(err) } : s
        ));
        await new Promise(r => setTimeout(r, 3000));
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  if (loading) {
    return (
      <div className="metrics-page">
        <LoadingScreen steps={loadingSteps} />
      </div>
    );
  }

  if (error || !metrics) {
    return (
      <div className="metrics-page">
        <div className="metrics-error">
          <p>⚠️ Failed to load metrics</p>
          <p className="error-detail">{error}</p>
          <button onClick={() => navigate('/')}>← Back</button>
        </div>
      </div>
    );
  }

  const maxCategoryCount = Math.max(...metrics.vessels_by_category.map(c => c.vessel_count), 1);
  const maxTypeCount = Math.max(...metrics.vessels_by_type.map(t => t.vessel_count), 1);
  const maxDayCnt = Math.max(...metrics.positions_per_day.map(d => d.cnt), 1);

  return (
    <div className="metrics-page">
      {/* Header */}
      <div className="metrics-header">
        <button className="back-btn" onClick={() => navigate('/')}>
          ← Home
        </button>
        <h1>📊 Database Metrics</h1>
        <span className="metrics-refresh-hint">Refreshes on page load</span>
      </div>

      {/* KPI cards */}
      <div className="kpi-grid">
        <div className="kpi-card">
          <div className="kpi-icon">🚢</div>
          <div className="kpi-value">{formatNumber(metrics.total_vessels)}</div>
          <div className="kpi-label">Total Vessels</div>
        </div>
        <div className="kpi-card">
          <div className="kpi-icon">📍</div>
          <div className="kpi-value">{formatNumber(metrics.total_positions)}</div>
          <div className="kpi-label">Position Reports</div>
        </div>
        <div className="kpi-card">
          <div className="kpi-icon">💾</div>
          <div className="kpi-value">{formatBytes(metrics.db_size_bytes)}</div>
          <div className="kpi-label">Database Size</div>
        </div>
        <div className="kpi-card">
          <div className="kpi-icon">⏱️</div>
          <div className="kpi-value">
            {metrics.time_range ? formatDuration(metrics.time_range.ts_min, metrics.time_range.ts_max) : 'N/A'}
          </div>
          <div className="kpi-label">Data Span</div>
        </div>
      </div>

      {/* Secondary stats */}
      <div className="secondary-stats">
        <div className="stat-chip">
          <span className="chip-label">Destinations</span>
          <span className="chip-value">{formatNumber(metrics.total_destinations)}</span>
        </div>
        <div className="stat-chip">
          <span className="chip-label">Draught records</span>
          <span className="chip-value">{formatNumber(metrics.total_draughts)}</span>
        </div>
        <div className="stat-chip">
          <span className="chip-label">Nav status changes</span>
          <span className="chip-value">{formatNumber(metrics.total_nav_statuses)}</span>
        </div>
        <div className="stat-chip">
          <span className="chip-label">Name changes</span>
          <span className="chip-value">{formatNumber(metrics.total_name_changes)}</span>
        </div>
      </div>

      <div className="metrics-grid">
        {/* Vessels by Category */}
        <div className="metrics-card">
          <h2>Vessels by Category</h2>
          <div className="bar-chart">
            {metrics.vessels_by_category.map(cat => (
              <div key={cat.category} className="bar-row">
                <span className="bar-label">{cat.category}</span>
                <div className="bar-track">
                  <div
                    className={`bar-fill cat-${cat.category.toLowerCase()}`}
                    style={{ width: `${(cat.vessel_count / maxCategoryCount) * 100}%` }}
                  />
                </div>
                <span className="bar-value">{formatNumber(cat.vessel_count)}</span>
              </div>
            ))}
          </div>
        </div>

        {/* Vessels by Type (top 15) */}
        <div className="metrics-card">
          <h2>Vessels by Type</h2>
          <div className="bar-chart scrollable">
            {metrics.vessels_by_type.slice(0, 15).map(t => (
              <div key={t.ship_type_code ?? t.ship_type_label} className="bar-row">
                <span className="bar-label" title={t.ship_type_label}>
                  {t.ship_type_label.length > 25
                    ? t.ship_type_label.slice(0, 25) + '…'
                    : t.ship_type_label}
                </span>
                <div className="bar-track">
                  <div
                    className={`bar-fill cat-${t.category.toLowerCase()}`}
                    style={{ width: `${(t.vessel_count / maxTypeCount) * 100}%` }}
                  />
                </div>
                <span className="bar-value">{formatNumber(t.vessel_count)}</span>
              </div>
            ))}
          </div>
        </div>

        {/* Positions per day */}
        <div className="metrics-card wide">
          <h2>Positions per Day (last 30 days)</h2>
          <div className="day-chart">
            {[...metrics.positions_per_day].reverse().map(d => (
              <div key={d.day} className="day-bar-col" title={`${d.day}: ${formatNumber(d.cnt)}`}>
                <div
                  className="day-bar"
                  style={{ height: `${(d.cnt / maxDayCnt) * 100}%` }}
                />
                <span className="day-label">{d.day.slice(5)}</span>
              </div>
            ))}
          </div>
        </div>

        {/* Table sizes */}
        <div className="metrics-card">
          <h2>Table Row Counts</h2>
          <table className="metrics-table">
            <thead>
              <tr>
                <th>Table</th>
                <th>Rows</th>
              </tr>
            </thead>
            <tbody>
              {metrics.table_sizes.map(t => (
                <tr key={t.table}>
                  <td className="table-name">{t.table}</td>
                  <td className="table-count">{formatNumber(t.row_count)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

      </div>

      {/* Time range info */}
      {metrics.time_range && (
        <div className="time-range-info">
          Data from{' '}
          <strong>{new Date(metrics.time_range.ts_min * 1000).toISOString().slice(0, 19).replace('T', ' ')} UTC</strong>
          {' '}to{' '}
          <strong>{new Date(metrics.time_range.ts_max * 1000).toISOString().slice(0, 19).replace('T', ' ')} UTC</strong>
        </div>
      )}
    </div>
  );
}
