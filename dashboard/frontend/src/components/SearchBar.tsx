
import { useState, useRef, useEffect, useCallback } from 'react';
import { searchVessels } from '../api';
import type { SearchResult } from '../types';

interface Props {
  onSelect: (mmsi: number) => void;
}

export default function SearchBar({ onSelect }: Props) {
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<SearchResult[]>([]);
  const [open, setOpen] = useState(false);
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);

  // Close on outside click
  useEffect(() => {
    const handler = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, []);

  const handleInput = useCallback((val: string) => {
    setQuery(val);
    if (debounceRef.current) clearTimeout(debounceRef.current);
    if (val.trim().length < 2) {
      setResults([]);
      setOpen(false);
      return;
    }
    debounceRef.current = setTimeout(async () => {
      try {
        const res = await searchVessels(val.trim());
        setResults(res);
        setOpen(res.length > 0);
      } catch {
        setResults([]);
      }
    }, 300);
  }, []);

  const handleSelect = (mmsi: number) => {
    setOpen(false);
    setQuery('');
    onSelect(mmsi);
  };

  return (
    <div className="search-box" ref={containerRef}>
      <span className="search-icon">🔍</span>
      <input
        type="text"
        placeholder="Search vessel name or MMSI..."
        value={query}
        onChange={e => handleInput(e.target.value)}
        onFocus={() => { if (results.length > 0) setOpen(true); }}
      />
      {open && (
        <div className="search-results">
          {results.map(r => (
            <div
              key={r.mmsi}
              className="result-item"
              onClick={() => handleSelect(r.mmsi)}
            >
              <div className="name">{r.ship_name || 'Unknown'}</div>
              <div className="meta">
                MMSI {r.mmsi} · {r.ship_type_label} · {r.category}
                {r.imo_number ? ` · IMO ${r.imo_number}` : ''}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
