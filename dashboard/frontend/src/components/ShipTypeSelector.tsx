import { useState, useEffect, useMemo } from 'react';
import { useNavigate } from 'react-router-dom';
import { fetchShipCategories } from '../api';
import type { ShipCategoryInfo } from '../types';
import LoadingScreen from './LoadingScreen';
import type { LoadingStep } from '../types';

export default function ShipTypeSelector() {
  const navigate = useNavigate();
  const [data, setData] = useState<ShipCategoryInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingSteps, setLoadingSteps] = useState<LoadingStep[]>([
    { label: 'Loading ship categories', status: 'running' },
  ]);
  const [selectedCategories, setSelectedCategories] = useState<Set<string>>(new Set());

  useEffect(() => {
    (async () => {
      try {
        const cats = await fetchShipCategories();
        setData(cats);
        // Select all categories by default
        const allCats = new Set(cats.map(c => c.category));
        setSelectedCategories(allCats);
        setLoadingSteps([{ label: 'Loading ship categories', status: 'done', durationMs: 0 }]);
        await new Promise(r => setTimeout(r, 300));
      } catch (err) {
        setLoadingSteps([{ label: 'Loading ship categories', status: 'error', detail: String(err) }]);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  // Group by category
  const grouped = useMemo(() => {
    const map = new Map<string, { types: ShipCategoryInfo[]; total: number }>();
    for (const item of data) {
      const existing = map.get(item.category);
      if (existing) {
        existing.types.push(item);
        existing.total += item.vessel_count;
      } else {
        map.set(item.category, { types: [item], total: item.vessel_count });
      }
    }
    // Sort by total descending
    return [...map.entries()].sort((a, b) => b[1].total - a[1].total);
  }, [data]);

  const toggleCategory = (cat: string) => {
    setSelectedCategories(prev => {
      const next = new Set(prev);
      if (next.has(cat)) next.delete(cat);
      else next.add(cat);
      return next;
    });
  };

  const selectAll = () => setSelectedCategories(new Set(grouped.map(([cat]) => cat)));
  const selectNone = () => setSelectedCategories(new Set());

  const totalSelected = useMemo(() => {
    return data
      .filter(d => selectedCategories.has(d.category))
      .reduce((sum, d) => sum + d.vessel_count, 0);
  }, [data, selectedCategories]);

  const handleProceed = () => {
    const cats = [...selectedCategories];
    const params = new URLSearchParams();
    params.set('categories', cats.join(','));
    navigate(`/map?${params.toString()}`);
  };

  const categoryColors: Record<string, string> = {
    cargo: '#60a5fa',
    tanker: '#f87171',
    passenger: '#4ade80',
    fishing: '#facc15',
    service: '#a78bfa',
    military: '#fb923c',
    recreational: '#22d3ee',
    other: '#94a3b8',
    unknown: '#64748b',
  };

  const getColor = (cat: string) => categoryColors[cat.toLowerCase()] ?? '#94a3b8';

  if (loading) {
    return (
      <div className="loading-overlay">
        <LoadingScreen steps={loadingSteps} />
      </div>
    );
  }

  return (
    <div className="ship-type-selector">
      <div className="selector-header">
        <button className="back-btn" onClick={() => navigate('/')}>← Home</button>
        <h1>🗺️ Select Ship Types</h1>
        <p className="selector-subtitle">Choose which vessel categories to display on the map</p>
      </div>

      <div className="selector-actions">
        <button className="selector-action-btn" onClick={selectAll}>Select All</button>
        <button className="selector-action-btn" onClick={selectNone}>Deselect All</button>
        <span className="selector-summary">
          {selectedCategories.size} / {grouped.length} categories ·{' '}
          ~{totalSelected.toLocaleString()} vessels
        </span>
      </div>

      <div className="category-grid">
        {grouped.map(([cat, info]) => {
          const isSelected = selectedCategories.has(cat);
          const color = getColor(cat);
          return (
            <button
              key={cat}
              className={`category-card ${isSelected ? 'selected' : ''}`}
              onClick={() => toggleCategory(cat)}
              style={{ '--cat-color': color } as React.CSSProperties}
            >
              <div className="category-card-header">
                <div className="category-check">{isSelected ? '✓' : '○'}</div>
                <span className="category-name">{cat}</span>
                <span className="category-count">{info.total.toLocaleString()}</span>
              </div>
              <div className="category-types">
                {info.types.slice(0, 5).map(t => (
                  <span key={t.ship_type_code} className="type-chip">
                    {t.ship_type_label} ({t.vessel_count})
                  </span>
                ))}
                {info.types.length > 5 && (
                  <span className="type-chip more">+{info.types.length - 5} more</span>
                )}
              </div>
            </button>
          );
        })}
      </div>

      <div className="selector-footer">
        <button
          className="proceed-btn"
          disabled={selectedCategories.size === 0}
          onClick={handleProceed}
        >
          Open Map with {selectedCategories.size} categories →
        </button>
      </div>
    </div>
  );
}
