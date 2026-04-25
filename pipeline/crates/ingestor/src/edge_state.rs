// ─── Edge State ───────────────────────────────────────────────────────────────
// Migrated unchanged from ais_ingestor_rs/src/edge_state.rs
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;

#[derive(Debug, Default)]
struct VesselEdge {
    draught_x10:   Option<u32>,
    nav_status:    Option<u8>,
    destination:   Option<String>,
    ship_name:     Option<String>,
    vessel_static: Option<VesselStaticKey>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VesselStaticKey {
    pub call_sign:      Option<String>,
    pub imo_number:     Option<u32>,
    pub ship_type:      Option<u8>,
    pub length_overall: Option<u16>,
    pub beam:           Option<u16>,
}

#[derive(Debug)]
pub struct EdgeState {
    vessels: HashMap<u32, VesselEdge>,
}

impl EdgeState {
    pub fn new() -> Self {
        Self { vessels: HashMap::with_capacity(16_384) }
    }

    pub fn check_draught(&mut self, mmsi: u32, draught: f32) -> Option<(f32, Option<f32>)> {
        let key   = (draught * 10.0).round() as u32;
        let entry = self.vessels.entry(mmsi).or_default();
        match entry.draught_x10 {
            Some(prev) if prev == key => None,
            prev => {
                entry.draught_x10 = Some(key);
                Some((draught, prev.map(|p| p as f32 / 10.0)))
            }
        }
    }

    pub fn check_nav_status(&mut self, mmsi: u32, status: u8) -> Option<(u8, Option<u8>)> {
        let entry = self.vessels.entry(mmsi).or_default();
        match entry.nav_status {
            Some(prev) if prev == status => None,
            prev => { entry.nav_status = Some(status); Some((status, prev)) }
        }
    }

    pub fn check_destination(&mut self, mmsi: u32, dest: &str) -> Option<(String, Option<String>)> {
        let trimmed = dest.trim().to_uppercase();
        if trimmed.is_empty() { return None; }
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

    pub fn check_ship_name(&mut self, mmsi: u32, name: &str) -> Option<String> {
        let trimmed = name.trim().to_uppercase();
        if trimmed.is_empty() { return None; }
        let entry = self.vessels.entry(mmsi).or_default();
        match &entry.ship_name {
            Some(prev) if *prev == trimmed => None,
            _ => { entry.ship_name = Some(trimmed.clone()); Some(trimmed) }
        }
    }

    pub fn check_vessel_static(&mut self, mmsi: u32, key: VesselStaticKey) -> bool {
        let entry = self.vessels.entry(mmsi).or_default();
        match &entry.vessel_static {
            Some(prev) if *prev == key => false,
            _ => { entry.vessel_static = Some(key); true }
        }
    }

    pub fn vessel_count(&self) -> usize { self.vessels.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_draught_edge() {
        let mut s = EdgeState::new();
        assert!(s.check_draught(1, 5.0).is_some());
        assert!(s.check_draught(1, 5.0).is_none());
        let ev = s.check_draught(1, 8.0).unwrap();
        assert_eq!(ev.0, 8.0);
        assert_eq!(ev.1, Some(5.0));
    }

    #[test]
    fn test_nav_edge() {
        let mut s = EdgeState::new();
        assert_eq!(s.check_nav_status(1, 0).unwrap(), (0, None));
        assert!(s.check_nav_status(1, 0).is_none());
        assert_eq!(s.check_nav_status(1, 5).unwrap(), (5, Some(0)));
    }

    #[test]
    fn test_dest_edge() {
        let mut s = EdgeState::new();
        assert!(s.check_destination(1, "ROTTERDAM").is_some());
        assert!(s.check_destination(1, "rotterdam").is_none());
        assert!(s.check_destination(1, "SINGAPORE").is_some());
    }

    #[test]
    fn test_name_edge() {
        let mut s = EdgeState::new();
        assert_eq!(s.check_ship_name(1, "EVER GIVEN").unwrap(), "EVER GIVEN");
        assert!(s.check_ship_name(1, "ever given").is_none());
        assert!(s.check_ship_name(1, "").is_none());
    }
}
