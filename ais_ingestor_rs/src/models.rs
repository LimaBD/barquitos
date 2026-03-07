/// Serde models for deserializing AIS JSONL records from aisstream.io.

use serde::Deserialize;

/// Top-level record in each JSONL line.
#[derive(Debug, Deserialize)]
pub struct AisRecord {
    #[serde(rename = "Message", default)]
    pub message: serde_json::Value,

    #[serde(rename = "MessageType", default)]
    pub message_type: String,

    #[serde(rename = "MetaData", default)]
    pub meta_data: MetaData,
}

#[derive(Debug, Deserialize, Default)]
pub struct MetaData {
    #[serde(rename = "MMSI")]
    pub mmsi: Option<i64>,

    #[serde(rename = "ShipName", default)]
    pub ship_name: Option<String>,

    pub latitude: Option<f64>,
    pub longitude: Option<f64>,

    #[serde(rename = "time_utc", default)]
    pub time_utc: Option<String>,
}

// ---------------------------------------------------------------------------
// Sub-message structs — used for typed extraction
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct PositionReport {
    #[serde(rename = "Latitude")]
    pub latitude: Option<f64>,
    #[serde(rename = "Longitude")]
    pub longitude: Option<f64>,
    #[serde(rename = "Sog")]
    pub sog: Option<f64>,
    #[serde(rename = "Cog")]
    pub cog: Option<f64>,
    #[serde(rename = "TrueHeading")]
    pub true_heading: Option<i64>,
    #[serde(rename = "RateOfTurn")]
    pub rate_of_turn: Option<f64>,
    #[serde(rename = "NavigationalStatus")]
    pub navigational_status: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Dimension {
    #[serde(rename = "A")]
    pub a: Option<i64>,
    #[serde(rename = "B")]
    pub b: Option<i64>,
    #[serde(rename = "C")]
    pub c: Option<i64>,
    #[serde(rename = "D")]
    pub d: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
pub struct Eta {
    #[serde(rename = "Month")]
    pub month: Option<u32>,
    #[serde(rename = "Day")]
    pub day: Option<u32>,
    #[serde(rename = "Hour")]
    pub hour: Option<u32>,
    #[serde(rename = "Minute")]
    pub minute: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ReportA {
    #[serde(rename = "Name")]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ShipStaticData {
    #[serde(rename = "Name")]
    pub name: Option<String>,
    #[serde(rename = "CallSign")]
    pub call_sign: Option<String>,
    #[serde(rename = "ImoNumber")]
    pub imo_number: Option<i64>,
    #[serde(rename = "Type")]
    pub ship_type: Option<i64>,
    #[serde(rename = "ShipType")]
    pub ship_type_alt: Option<i64>,
    #[serde(rename = "Dimension")]
    pub dimension: Option<Dimension>,
    #[serde(rename = "MaximumStaticDraught")]
    pub max_draught: Option<f64>,
    #[serde(rename = "AisVersion")]
    pub ais_version: Option<i64>,
    #[serde(rename = "FixType")]
    pub fix_type: Option<i64>,
    #[serde(rename = "Destination")]
    pub destination: Option<String>,
    #[serde(rename = "Eta")]
    pub eta: Option<Eta>,
    #[serde(rename = "ReportA")]
    pub report_a: Option<ReportA>,
}
