
import type { VesselDetail } from '../types';
import { formatDateTime } from '../utils';

interface Props {
  detail: VesselDetail;
  trackLength: number;
  onClose: () => void;
}

export default function VesselPanel({ detail, trackLength, onClose }: Props) {
  const { vessel, destinations, nav_statuses, draughts, position_count } = detail;

  const categoryClass = (vessel.category || 'other').toLowerCase().replace(/\s/g, '');

  return (
    <div className="vessel-panel">
      <div className="panel-header">
        <div>
          <h2>{vessel.ship_name || 'Unknown Vessel'}</h2>
          <div className="subtitle">
            MMSI {vessel.mmsi}
            {vessel.imo_number ? ` · IMO ${vessel.imo_number}` : ''}
            {vessel.call_sign ? ` · ${vessel.call_sign}` : ''}
          </div>
        </div>
        <button className="close-btn" onClick={onClose}>✕</button>
      </div>

      <div className="panel-body">
        {/* Type badge */}
        <div style={{ marginBottom: 16 }}>
          <span className={`category-badge ${categoryClass}`}>
            {vessel.category}
          </span>
          <span style={{ marginLeft: 8, color: 'var(--text-secondary)', fontSize: 12 }}>
            {vessel.ship_type_label}
          </span>
        </div>

        {/* Info grid */}
        <div className="info-grid">
          <InfoItem label="Length" value={vessel.length_m ? `${vessel.length_m} m` : '—'} />
          <InfoItem label="Beam" value={vessel.beam_m ? `${vessel.beam_m} m` : '—'} />
          <InfoItem label="Max Draught" value={vessel.max_draught ? `${vessel.max_draught} m` : '—'} />
          <InfoItem label="AIS Type" value={vessel.ship_type != null ? `${vessel.ship_type}` : '—'} />
          <InfoItem label="First Seen" value={formatDateTime(vessel.first_seen_utc)} />
          <InfoItem label="Last Seen" value={formatDateTime(vessel.last_seen_utc)} />
          <InfoItem label="Positions" value={position_count.toLocaleString()} />
          <InfoItem label="Track Points" value={trackLength.toLocaleString()} />
        </div>

        {/* Destinations */}
        {destinations.length > 0 && (
          <>
            <div className="section-title">Recent Destinations</div>
            {destinations.map((d, i) => (
              <div key={i} className="history-item">
                <div className="hist-val">{d.destination || '—'}</div>
                <div className="hist-time">
                  {formatDateTime(d.timestamp_utc)}
                  {d.estimated_arrival ? ` · ETA ${formatDateTime(d.estimated_arrival)}` : ''}
                </div>
              </div>
            ))}
          </>
        )}

        {/* Nav Status */}
        {nav_statuses.length > 0 && (
          <>
            <div className="section-title">Navigation Status</div>
            {nav_statuses.map((n, i) => (
              <div key={i} className="history-item">
                <div className="hist-val">{n.label}</div>
                <div className="hist-time">{formatDateTime(n.timestamp_utc)}</div>
              </div>
            ))}
          </>
        )}

        {/* Draughts */}
        {draughts.length > 0 && (
          <>
            <div className="section-title">Draught History</div>
            {draughts.map((d, i) => (
              <div key={i} className="history-item">
                <div className="hist-val">{d.draught} m</div>
                <div className="hist-time">{formatDateTime(d.timestamp_utc)}</div>
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

function InfoItem({ label, value }: { label: string; value: string }) {
  return (
    <div className="info-item">
      <span className="label">{label}</span>
      <span className="value">{value}</span>
    </div>
  );
}
