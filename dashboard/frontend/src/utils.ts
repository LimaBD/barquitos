/** Format a Unix epoch timestamp as a readable date string. */
export function formatTimestamp(epoch: number): string {
  const d = new Date(epoch * 1000);
  return d.toISOString().replace('T', ' ').replace(/\.\d+Z$/, ' UTC');
}

/** Format a short time string HH:MM */
export function formatTime(epoch: number): string {
  const d = new Date(epoch * 1000);
  return d.toISOString().substring(11, 16);
}

/** Format date as YYYY-MM-DD */
export function formatDate(epoch: number): string {
  const d = new Date(epoch * 1000);
  return d.toISOString().substring(0, 10);
}

/** Format date+time compactly */
export function formatDateTime(epoch: number): string {
  const d = new Date(epoch * 1000);
  return d.toISOString().replace('T', ' ').substring(0, 19) + ' UTC';
}

/** Get ship category colour for map markers */
export function getCategoryColor(category: string): string {
  switch (category?.toLowerCase()) {
    case 'cargo':        return '#60a5fa';
    case 'tanker':       return '#f87171';
    case 'passenger':    return '#4ade80';
    case 'fishing':      return '#facc15';
    case 'service':      return '#a78bfa';
    case 'military':     return '#fb923c';
    case 'recreational': return '#22d3ee';
    default:             return '#94a3b8';
  }
}

/**
 * Build the SVG for a ship arrow marker.
 * The arrow points in the direction of `heading` (degrees, 0=N, clockwise).
 */
export function shipSvg(heading: number | null, color: string, size = 12): string {
  const rot = heading != null && heading < 360 ? heading : 0;
  return `<svg width="${size}" height="${size}" viewBox="0 0 24 24" style="transform:rotate(${rot}deg)">
    <path d="M12 2 L18 20 L12 16 L6 20 Z" fill="${color}" stroke="#000" stroke-width="1" stroke-linejoin="round"/>
  </svg>`;
}

/** Clamp a number between min and max */
export function clamp(v: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, v));
}
