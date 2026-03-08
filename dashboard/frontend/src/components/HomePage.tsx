import { useNavigate } from 'react-router-dom';

const menuItems = [
  {
    key: 'map',
    icon: '🗺️',
    title: 'Global Ship Map',
    description: 'Select vessel types and view them on an interactive map. Filter, zoom, and refresh in real-time.',
    path: '/select-types',
    accent: 'var(--accent)',
  },
  {
    key: 'metrics',
    icon: '📊',
    title: 'Database Metrics',
    description: 'Vessel counts, position records, storage usage, and detailed breakdowns by type and category.',
    path: '/metrics',
    accent: 'var(--green)',
  },
  {
    key: 'coming-1',
    icon: '📈',
    title: 'Analytics & Charts',
    description: 'Traffic density heatmaps, speed distribution graphs, and route analysis. Coming soon.',
    path: '',
    accent: 'var(--yellow)',
    disabled: true,
  },
  {
    key: 'coming-2',
    icon: '🔔',
    title: 'Alerts & Monitoring',
    description: 'Set up alerts for specific vessels, zones, or anomalies. Coming soon.',
    path: '',
    accent: 'var(--orange)',
    disabled: true,
  },
];

export default function HomePage() {
  const navigate = useNavigate();

  return (
    <div className="home-page">
      <div className="home-hero">
        <div className="home-logo">🚢</div>
        <h1>Barquitos</h1>
        <p className="home-subtitle">Ship Tracking & Intelligence Dashboard</p>
      </div>

      <div className="home-menu-grid">
        {menuItems.map(item => (
          <button
            key={item.key}
            className={`home-menu-card ${item.disabled ? 'disabled' : ''}`}
            onClick={() => !item.disabled && navigate(item.path)}
            disabled={item.disabled}
            style={{ '--card-accent': item.accent } as React.CSSProperties}
          >
            <div className="card-icon">{item.icon}</div>
            <div className="card-content">
              <h3>{item.title}</h3>
              <p>{item.description}</p>
            </div>
            {item.disabled && <span className="coming-soon-badge">Coming Soon</span>}
            {!item.disabled && <span className="card-arrow">→</span>}
          </button>
        ))}
      </div>
    </div>
  );
}
