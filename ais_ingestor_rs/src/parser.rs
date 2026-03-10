// ─── AIS JSON Parser ────────────────────────────────────────────────────────
//
// Parses the raw JSONL messages produced by the Python AIS collector.
// Extracts only the fields we need for each fact table, zero-copy where
// possible via serde_json::Value borrows.
// ────────────────────────────────────────────────────────────────────────────

use chrono::{NaiveDate, NaiveDateTime, Utc, TimeZone};
use serde_json::Value;

/// Parsed timestamp in seconds since Unix epoch (UTC).
/// Second-precision is sufficient for AIS data (saves ~50% vs microseconds).
pub type TsSeconds = i64;

/// Parse the AISstream `time_utc` string into seconds since epoch.
/// Format: "2026-03-04 04:34:51.629155984 +0000 UTC"
pub fn parse_time_utc(s: &str) -> Option<TsSeconds> {
    // Strip trailing " UTC" or " +0000 UTC"
    let s = s.trim();
    // Try to find the fractional-second part
    // Expected: "2026-03-04 04:34:51.629155984 +0000 UTC"
    let date_time_part = s.split(" +").next()?;
    // "2026-03-04 04:34:51.629155984"
    let ndt = NaiveDateTime::parse_from_str(date_time_part, "%Y-%m-%d %H:%M:%S%.f").ok()
        .or_else(|| NaiveDateTime::parse_from_str(date_time_part, "%Y-%m-%d %H:%M:%S").ok())?;
    let dt = Utc.from_utc_datetime(&ndt);
    Some(dt.timestamp())
}

/// Round a coordinate to 5 decimal places (~1.1 m precision at the equator).
#[inline]
pub fn round_coord_5dp(v: f64) -> f64 {
    (v * 100_000.0).round() / 100_000.0
}

/// Build a UTC epoch-second timestamp from AIS ETA fields (month, day, hour, minute).
/// Uses the current year; if the resulting date is >6 months in the past, assumes next year.
pub fn eta_to_epoch(month: u8, day: u8, hour: u8, minute: u8) -> Option<i64> {
    use chrono::Datelike;
    let now = Utc::now();
    let year = now.year();
    let dt = NaiveDate::from_ymd_opt(year, month as u32, day as u32)?
        .and_hms_opt(hour as u32, minute as u32, 0)?;
    let utc_dt = Utc.from_utc_datetime(&dt);
    // If more than 6 months in the past, assume next year
    if utc_dt < now - chrono::Duration::days(180) {
        let dt2 = NaiveDate::from_ymd_opt(year + 1, month as u32, day as u32)?
            .and_hms_opt(hour as u32, minute as u32, 0)?;
        Some(Utc.from_utc_datetime(&dt2).timestamp())
    } else {
        Some(utc_dt.timestamp())
    }
}

/// Extracted position data from PositionReport / StandardClassBPositionReport / ExtendedClassBPositionReport.
#[derive(Debug)]
pub struct PositionRecord {
    pub mmsi: u32,
    pub timestamp_s: TsSeconds,
    pub latitude: f64,
    pub longitude: f64,
    pub sog: Option<f32>,
    pub cog: Option<f32>,
    pub heading: Option<u16>,
    pub nav_status: Option<u8>,
    pub rot: Option<i16>,
    /// Ship name from MetaData — used only for name-history edge detection,
    /// not stored in the position table.
    pub ship_name: Option<String>,
}

/// Extracted static data from ShipStaticData (AIS message type 5).
#[derive(Debug)]
pub struct StaticRecord {
    pub mmsi: u32,
    pub timestamp_s: TsSeconds,
    pub ship_name: Option<String>,
    pub call_sign: Option<String>,
    pub imo_number: Option<u32>,
    pub ship_type: Option<u8>,
    pub dim_a: Option<u16>,
    pub dim_b: Option<u16>,
    pub dim_c: Option<u16>,
    pub dim_d: Option<u16>,
    pub draught: Option<f32>,
    pub destination: Option<String>,
    /// ETA as a single UTC epoch-second timestamp (merged from month/day/hour/minute).
    pub eta_utc: Option<i64>,
    /// Present in some JSON messages but not consumed by current pipeline.
    #[allow(dead_code)]
    pub nav_status: Option<u8>,
}

/// Result of parsing a single AIS JSON line.
pub enum ParsedMessage {
    Position(PositionRecord),
    Static(StaticRecord),
    Ignored,
    Error(String),
}

/// Parse a single JSON line from the collector's JSONL output.
pub fn parse_line(line: &str) -> ParsedMessage {
    let v: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return ParsedMessage::Error(format!("JSON parse error: {e}")),
    };

    let msg_type = match v["MessageType"].as_str() {
        Some(s) => s,
        None => return ParsedMessage::Error("Missing MessageType".into()),
    };

    let meta = &v["MetaData"];
    let ts_str = match meta["time_utc"].as_str() {
        Some(s) => s,
        None => return ParsedMessage::Error("Missing MetaData.time_utc".into()),
    };
    let timestamp_s = match parse_time_utc(ts_str) {
        Some(t) => t,
        None => return ParsedMessage::Error(format!("Bad timestamp: {ts_str}")),
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
            // Filter out invalid coordinates
            if lat.abs() > 90.0 || lon.abs() > 180.0 || (lat == 0.0 && lon == 0.0) {
                return ParsedMessage::Ignored;
            }
            let sog = msg["Sog"].as_f64().map(|v| v as f32);
            let cog = msg["Cog"].as_f64().map(|v| v as f32);
            let heading = msg["TrueHeading"].as_u64().map(|v| v as u16).filter(|&h| h < 360);
            let nav_status = msg["NavigationalStatus"].as_u64().map(|v| v as u8);
            let rot = msg["RateOfTurn"].as_i64().map(|v| v as i16);

            ParsedMessage::Position(PositionRecord {
                mmsi,
                timestamp_s,
                latitude: round_coord_5dp(lat),
                longitude: round_coord_5dp(lon),
                sog,
                cog,
                heading,
                nav_status,
                rot,
                ship_name: ship_name_meta,
            })
        }
        "ShipStaticData" => {
            let msg = &v["Message"]["ShipStaticData"];
            let name = msg["Name"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let call_sign = msg["CallSign"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let imo = msg["ImoNumber"].as_u64().map(|v| v as u32).filter(|&v| v > 0);
            let ship_type = msg["Type"].as_u64().map(|v| v as u8);
            let dim = &msg["Dimension"];
            let dim_a = dim["A"].as_u64().map(|v| v as u16);
            let dim_b = dim["B"].as_u64().map(|v| v as u16);
            let dim_c = dim["C"].as_u64().map(|v| v as u16);
            let dim_d = dim["D"].as_u64().map(|v| v as u16);
            let draught = msg["MaximumStaticDraught"].as_f64().map(|v| v as f32).filter(|&d| d > 0.0);
            let destination = msg["Destination"].as_str().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            let eta = &msg["Eta"];
            let eta_month = eta["Month"].as_u64().map(|v| v as u8).filter(|&m| m >= 1 && m <= 12);
            let eta_day = eta["Day"].as_u64().map(|v| v as u8).filter(|&d| d >= 1 && d <= 31);
            let eta_hour = eta["Hour"].as_u64().map(|v| v as u8).filter(|&h| h < 24);
            let eta_minute = eta["Minute"].as_u64().map(|v| v as u8).filter(|&m| m < 60);

            // Merge ETA fields into a single timestamp
            let eta_utc = match (eta_month, eta_day, eta_hour, eta_minute) {
                (Some(m), Some(d), Some(h), Some(min)) => eta_to_epoch(m, d, h, min),
                _ => None,
            };

            // Some position reports include nav_status in the metadata
            let nav_status = msg["NavigationalStatus"].as_u64().map(|v| v as u8);

            ParsedMessage::Static(StaticRecord {
                mmsi,
                timestamp_s,
                ship_name: name.or(ship_name_meta),
                call_sign,
                imo_number: imo,
                ship_type,
                dim_a,
                dim_b,
                dim_c,
                dim_d,
                draught,
                destination,
                eta_utc,
                nav_status,
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
        let ts = parse_time_utc("2026-03-04 04:34:51.629155984 +0000 UTC");
        assert!(ts.is_some());
        let s = ts.unwrap();
        // 2026-03-04 04:34:51 UTC = epoch 1772598891
        assert_eq!(s, 1_772_598_891);
    }

    #[test]
    fn test_parse_position_report() {
        let line = r#"{"Message":{"PositionReport":{"Cog":192.6,"CommunicationState":86250,"Latitude":64.82,"Longitude":11.61,"MessageID":3,"NavigationalStatus":5,"PositionAccuracy":false,"Raim":false,"RateOfTurn":0,"RepeatIndicator":0,"Sog":0,"Spare":0,"SpecialManoeuvreIndicator":0,"Timestamp":50,"TrueHeading":275,"UserID":257266500,"Valid":true}},"MessageType":"PositionReport","MetaData":{"MMSI":257266500,"MMSI_String":257266500,"ShipName":"LYSINGEN","latitude":64.82,"longitude":11.61,"time_utc":"2026-03-04 04:34:51.629155984 +0000 UTC"}}"#;
        match parse_line(line) {
            ParsedMessage::Position(p) => {
                assert_eq!(p.mmsi, 257266500);
                assert!((p.latitude - 64.82).abs() < 0.00001);
                assert!((p.longitude - 11.61).abs() < 0.00001);
                assert_eq!(p.nav_status, Some(5));
            }
            other => panic!("Expected Position, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn test_parse_static_data() {
        let line = r#"{"Message":{"ShipStaticData":{"AisVersion":2,"CallSign":"LFTQ   ","Destination":"FISHFARMS           ","Dimension":{"A":15,"B":55,"C":7,"D":8},"Dte":false,"Eta":{"Day":21,"Hour":12,"Minute":34,"Month":3},"FixType":0,"ImoNumber":9674036,"MaximumStaticDraught":6.4,"MessageID":5,"Name":"RUBIN               ","RepeatIndicator":0,"Spare":false,"Type":75,"UserID":257421000,"Valid":true}},"MessageType":"ShipStaticData","MetaData":{"MMSI":257421000,"MMSI_String":257421000,"ShipName":"RUBIN               ","latitude":63.81,"longitude":9.64,"time_utc":"2026-03-04 04:34:51.632199078 +0000 UTC"}}"#;
        match parse_line(line) {
            ParsedMessage::Static(s) => {
                assert_eq!(s.mmsi, 257421000);
                assert_eq!(s.draught, Some(6.4));
                assert_eq!(s.destination.as_deref(), Some("FISHFARMS"));
                assert_eq!(s.imo_number, Some(9674036));
                assert_eq!(s.ship_type, Some(75));
                // ETA should be merged into a single timestamp
                assert!(s.eta_utc.is_some());
            }
            other => panic!("Expected Static, got {:?}", std::mem::discriminant(&other)),
        }
    }

    #[test]
    fn test_round_coord_5dp() {
        assert_eq!(round_coord_5dp(64.82345678), 64.82346);
        assert_eq!(round_coord_5dp(-11.123456789), -11.12346);
        assert_eq!(round_coord_5dp(0.0), 0.0);
    }
}
