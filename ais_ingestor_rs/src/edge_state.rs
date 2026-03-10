// ─── Edge State ─────────────────────────────────────────────────────────────
//
// Maintains a lightweight HashMap<MMSI, LastKnownState> to implement
// edge-filtering (pivotal-only) logic.  A row is emitted to the
// corresponding Parquet buffer ONLY when the value changes.
//
// Example: draught sequence 5 5 5 8 8 8 → emits rows for the first 5
// and the first 8.
// ────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

/// Per-vessel edge state.  We store the "current known" value for each
/// dimension so we can detect pivots.
#[derive(Debug, Default)]
struct VesselEdge {
    /// Last known draught (multiplied by 10 and stored as u32 to avoid
    /// float comparison issues; e.g. 6.4 → 64).
    draught_x10: Option<u32>,
    /// Last known navigational status code.
    nav_status: Option<u8>,
    /// Last known destination string (uppercased, trimmed).
    destination: Option<String>,
    /// Last known ship name (uppercased, trimmed) for name-history tracking.
    ship_name: Option<String>,
    /// Last known vessel static fingerprint (call_sign, imo, type, length, beam).
    vessel_static: Option<VesselStaticKey>,
}

/// Compact fingerprint of vessel static fields for edge-detection.
/// We only emit a new dim_vessel row when any of these fields change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VesselStaticKey {
    pub call_sign: Option<String>,
    pub imo_number: Option<u32>,
    pub ship_type: Option<u8>,
    pub length_overall: Option<u16>,
    pub beam: Option<u16>,
}

/// Central edge-state tracker.
#[derive(Debug)]
pub struct EdgeState {
    vessels: HashMap<u32, VesselEdge>,
}

impl EdgeState {
    pub fn new() -> Self {
        Self {
            vessels: HashMap::with_capacity(16_384),
        }
    }

    /// Feed a draught observation.  Returns Some((new, old)) only on pivot.
    pub fn check_draught(&mut self, mmsi: u32, draught: f32) -> Option<(f32, Option<f32>)> {
        let key = (draught * 10.0).round() as u32;
        let entry = self.vessels.entry(mmsi).or_default();
        match entry.draught_x10 {
            Some(prev) if prev == key => None,          // same value → no event
            prev => {
                let old_val = prev.map(|p| p as f32 / 10.0);
                entry.draught_x10 = Some(key);
                Some((draught, old_val))
            }
        }
    }

    /// Feed a navigational status.  Returns Some((new, old)) only on pivot.
    pub fn check_nav_status(&mut self, mmsi: u32, status: u8) -> Option<(u8, Option<u8>)> {
        let entry = self.vessels.entry(mmsi).or_default();
        match entry.nav_status {
            Some(prev) if prev == status => None,
            prev => {
                entry.nav_status = Some(status);
                Some((status, prev))
            }
        }
    }

    /// Feed a destination string.  Returns Some((new, old)) only on pivot.
    pub fn check_destination(&mut self, mmsi: u32, dest: &str) -> Option<(String, Option<String>)> {
        let trimmed = dest.trim().to_uppercase();
        if trimmed.is_empty() {
            return None;
        }
        let entry = self.vessels.entry(mmsi).or_default();
        match &entry.destination {
            Some(prev) if *prev == trimmed => None,
            prev => {
                let old = prev.clone();
                entry.destination = Some(trimmed.clone());
                Some((trimmed, old))
            }
        }
    }

    /// Feed a ship name.  Returns Some(normalized_name) only on pivot (change).
    /// Empty / whitespace-only names are ignored.
    pub fn check_ship_name(&mut self, mmsi: u32, name: &str) -> Option<String> {
        let trimmed = name.trim().to_uppercase();
        if trimmed.is_empty() {
            return None;
        }
        let entry = self.vessels.entry(mmsi).or_default();
        match &entry.ship_name {
            Some(prev) if *prev == trimmed => None,
            _ => {
                entry.ship_name = Some(trimmed.clone());
                Some(trimmed)
            }
        }
    }

    /// Feed vessel static data.  Returns `true` only when any field differs
    /// from the last known state for this MMSI (or on first observation).
    pub fn check_vessel_static(&mut self, mmsi: u32, key: VesselStaticKey) -> bool {
        let entry = self.vessels.entry(mmsi).or_default();
        match &entry.vessel_static {
            Some(prev) if *prev == key => false,
            _ => {
                entry.vessel_static = Some(key);
                true
            }
        }
    }

    /// Number of tracked vessels (for health metrics).
    pub fn vessel_count(&self) -> usize {
        self.vessels.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_draught_edge() {
        let mut state = EdgeState::new();
        // First observation → always emit
        assert!(state.check_draught(123, 5.0).is_some());
        // Same → no emit
        assert!(state.check_draught(123, 5.0).is_none());
        assert!(state.check_draught(123, 5.0).is_none());
        // Change → emit with previous
        let ev = state.check_draught(123, 8.0).unwrap();
        assert_eq!(ev.0, 8.0);
        assert_eq!(ev.1, Some(5.0));
        // Same again
        assert!(state.check_draught(123, 8.0).is_none());
    }

    #[test]
    fn test_nav_status_edge() {
        let mut state = EdgeState::new();
        let ev = state.check_nav_status(100, 0).unwrap();
        assert_eq!(ev, (0, None));
        assert!(state.check_nav_status(100, 0).is_none());
        let ev = state.check_nav_status(100, 5).unwrap();
        assert_eq!(ev, (5, Some(0)));
    }

    #[test]
    fn test_destination_edge() {
        let mut state = EdgeState::new();
        let ev = state.check_destination(100, "ROTTERDAM").unwrap();
        assert_eq!(ev.0, "ROTTERDAM");
        assert!(ev.1.is_none());
        // Same (case insensitive)
        assert!(state.check_destination(100, "rotterdam").is_none());
        assert!(state.check_destination(100, "  ROTTERDAM  ").is_none());
        // Change
        let ev = state.check_destination(100, "SINGAPORE").unwrap();
        assert_eq!(ev.0, "SINGAPORE");
        assert_eq!(ev.1.as_deref(), Some("ROTTERDAM"));
    }

    #[test]
    fn test_ship_name_edge() {
        let mut state = EdgeState::new();
        // First observation → always emit
        let name = state.check_ship_name(200, "EVER GIVEN").unwrap();
        assert_eq!(name, "EVER GIVEN");
        // Same (case insensitive, trimmed) → no emit
        assert!(state.check_ship_name(200, "ever given").is_none());
        assert!(state.check_ship_name(200, "  EVER GIVEN  ").is_none());
        // Change → emit
        let name = state.check_ship_name(200, "EVER GLORY").unwrap();
        assert_eq!(name, "EVER GLORY");
        // Empty → ignored
        assert!(state.check_ship_name(200, "").is_none());
        assert!(state.check_ship_name(200, "   ").is_none());
        // Different vessel → independent
        let name = state.check_ship_name(300, "MAERSK ALABAMA").unwrap();
        assert_eq!(name, "MAERSK ALABAMA");
    }

    #[test]
    fn test_vessel_static_edge() {
        let mut state = EdgeState::new();
        let key1 = VesselStaticKey {
            call_sign: Some("LFTQ".into()),
            imo_number: Some(9674036),
            ship_type: Some(75),
            length_overall: Some(70),
            beam: Some(15),
        };
        // First observation → always emit
        assert!(state.check_vessel_static(100, key1.clone()));
        // Same → no emit
        assert!(!state.check_vessel_static(100, key1.clone()));
        // Change ship_type → emit
        let key2 = VesselStaticKey { ship_type: Some(80), ..key1.clone() };
        assert!(state.check_vessel_static(100, key2.clone()));
        // Same again → no emit
        assert!(!state.check_vessel_static(100, key2));
        // Different vessel → independent
        assert!(state.check_vessel_static(200, key1));
    }
}
