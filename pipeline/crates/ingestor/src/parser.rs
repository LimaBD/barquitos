// ─── AIS JSON Parser ─────────────────────────────────────────────────────────
//
// Parses the raw JSONL messages produced by the AIS collector.
// Migrated unchanged from ais_ingestor_rs — all logic is identical.
// ─────────────────────────────────────────────────────────────────────────────

use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;

pub type TsSeconds = i64;

pub fn parse_time_utc(s: &str) -> Option<TsSeconds> {
    let s = s.trim();
    let date_time_part = s.split(" +").next()?;
    let ndt = NaiveDateTime::parse_from_str(date_time_part, "%Y-%m-%d %H:%M:%S%.f")
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(date_time_part, "%Y-%m-%d %H:%M:%S").ok())?;
    Some(Utc.from_utc_datetime(&ndt).timestamp())
}

#[inline]
pub fn round_coord_5dp(v: f64) -> f64 {
    (v * 100_000.0).round() / 100_000.0
}

pub fn eta_to_epoch(month: u8, day: u8, hour: u8, minute: u8) -> Option<i64> {
    use chrono::Datelike;
    let now  = Utc::now();
    let year = now.year();
    let dt   = NaiveDate::from_ymd_opt(year, month as u32, day as u32)?
        .and_hms_opt(hour as u32, minute as u32, 0)?;
    let utc_dt = Utc.from_utc_datetime(&dt);
    if utc_dt < now - chrono::Duration::days(180) {
        let dt2 = NaiveDate::from_ymd_opt(year + 1, month as u32, day as u32)?
            .and_hms_opt(hour as u32, minute as u32, 0)?;
        Some(Utc.from_utc_datetime(&dt2).timestamp())
    } else {
        Some(utc_dt.timestamp())
    }
}

#[derive(Debug)]
pub struct PositionRecord {
    pub mmsi:        u32,
    pub timestamp_s: TsSeconds,
    pub latitude:    f64,
    pub longitude:   f64,
    pub sog:         Option<f32>,
    pub cog:         Option<f32>,
    pub heading:     Option<u16>,
    pub nav_status:  Option<u8>,
    pub rot:         Option<i16>,
    pub ship_name:   Option<String>,
}

#[derive(Debug)]
pub struct StaticRecord {
    pub mmsi:        u32,
    pub timestamp_s: TsSeconds,
    pub ship_name:   Option<String>,
    pub call_sign:   Option<String>,
    pub imo_number:  Option<u32>,
    pub ship_type:   Option<u8>,
    pub dim_a:       Option<u16>,
    pub dim_b:       Option<u16>,
    pub dim_c:       Option<u16>,
    pub dim_d:       Option<u16>,
    pub draught:     Option<f32>,
    pub destination: Option<String>,
    pub eta_utc:     Option<i64>,
    #[allow(dead_code)]
    pub nav_status:  Option<u8>,
}

pub enum ParsedMessage {
    Position(PositionRecord),
    Static(StaticRecord),
    Ignored,
    Error(String),
}

pub fn parse_line(line: &str) -> ParsedMessage {
    let v: Value = match serde_json::from_str(line) {
        Ok(v)  => v,
        Err(e) => return ParsedMessage::Error(format!("JSON parse: {e}")),
    };

    let msg_type = match v["MessageType"].as_str() {
        Some(s) => s,
        None    => return ParsedMessage::Error("Missing MessageType".into()),
    };

    let meta     = &v["MetaData"];
    let ts_str   = match meta["time_utc"].as_str() {
        Some(s) => s,
        None    => return ParsedMessage::Error("Missing MetaData.time_utc".into()),
    };
    let timestamp_s = match parse_time_utc(ts_str) {
        Some(t) => t,
        None    => return ParsedMessage::Error(format!("Bad timestamp: {ts_str}")),
    };
    let mmsi = meta["MMSI"].as_u64().unwrap_or(0) as u32;
    if mmsi == 0 {
        return ParsedMessage::Error("MMSI is 0".into());
    }
    let ship_name_meta = meta["ShipName"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    match msg_type {
        "PositionReport" | "StandardClassBPositionReport" | "ExtendedClassBPositionReport" => {
            let msg = &v["Message"][msg_type];
            let lat = msg["Latitude"].as_f64().unwrap_or(0.0);
            let lon = msg["Longitude"].as_f64().unwrap_or(0.0);
            if lat.abs() > 90.0 || lon.abs() > 180.0 || (lat == 0.0 && lon == 0.0) {
                return ParsedMessage::Ignored;
            }
            ParsedMessage::Position(PositionRecord {
                mmsi,
                timestamp_s,
                latitude:   round_coord_5dp(lat),
                longitude:  round_coord_5dp(lon),
                sog:        msg["Sog"].as_f64().map(|v| v as f32),
                cog:        msg["Cog"].as_f64().map(|v| v as f32),
                heading:    msg["TrueHeading"].as_u64().map(|v| v as u16).filter(|&h| h < 360),
                nav_status: msg["NavigationalStatus"].as_u64().map(|v| v as u8),
                rot:        msg["RateOfTurn"].as_i64().map(|v| v as i16),
                ship_name:  ship_name_meta,
            })
        }
        "ShipStaticData" => {
            let msg     = &v["Message"]["ShipStaticData"];
            let name    = msg["Name"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let eta     = &msg["Eta"];
            let eta_utc = match (
                eta["Month"].as_u64().map(|v| v as u8).filter(|&m| (1..=12).contains(&m)),
                eta["Day"].as_u64().map(|v| v as u8).filter(|&d| (1..=31).contains(&d)),
                eta["Hour"].as_u64().map(|v| v as u8).filter(|&h| h < 24),
                eta["Minute"].as_u64().map(|v| v as u8).filter(|&m| m < 60),
            ) {
                (Some(m), Some(d), Some(h), Some(min)) => eta_to_epoch(m, d, h, min),
                _ => None,
            };
            ParsedMessage::Static(StaticRecord {
                mmsi,
                timestamp_s,
                ship_name:   name.or(ship_name_meta),
                call_sign:   msg["CallSign"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                imo_number:  msg["ImoNumber"].as_u64().map(|v| v as u32).filter(|&v| v > 0),
                ship_type:   msg["Type"].as_u64().map(|v| v as u8),
                dim_a:       msg["Dimension"]["A"].as_u64().map(|v| v as u16),
                dim_b:       msg["Dimension"]["B"].as_u64().map(|v| v as u16),
                dim_c:       msg["Dimension"]["C"].as_u64().map(|v| v as u16),
                dim_d:       msg["Dimension"]["D"].as_u64().map(|v| v as u16),
                draught:     msg["MaximumStaticDraught"].as_f64().map(|v| v as f32).filter(|&d| d > 0.0),
                destination: msg["Destination"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                eta_utc,
                nav_status:  msg["NavigationalStatus"].as_u64().map(|v| v as u8),
            })
        }
        _ => ParsedMessage::Ignored,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time_utc() {
        let ts = parse_time_utc("2026-03-04 04:34:51.629155984 +0000 UTC").unwrap();
        assert_eq!(ts, 1_772_598_891);
    }

    #[test]
    fn test_parse_position() {
        let line = r#"{"Message":{"PositionReport":{"Cog":192.6,"Latitude":64.82,"Longitude":11.61,"NavigationalStatus":5,"RateOfTurn":0,"Sog":0,"TrueHeading":275,"UserID":257266500,"Valid":true}},"MessageType":"PositionReport","MetaData":{"MMSI":257266500,"ShipName":"LYSINGEN","time_utc":"2026-03-04 04:34:51.629155984 +0000 UTC"}}"#;
        match parse_line(line) {
            ParsedMessage::Position(p) => {
                assert_eq!(p.mmsi, 257266500);
                assert!((p.latitude - 64.82).abs() < 0.00001);
                assert_eq!(p.nav_status, Some(5));
            }
            _ => panic!("Expected Position"),
        }
    }

    #[test]
    fn test_round_coord() {
        assert_eq!(round_coord_5dp(64.82345678), 64.82346);
    }
}
