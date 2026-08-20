use serde::Deserialize;
use std::collections::BTreeMap;

/// Authority for channel count, channel names, and sampling rate.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_nickname: String,
    pub channel_names: Vec<String>,
    pub channels: usize,
    pub sampling_rate: f64,
}

/// The payload shape shared by the `calm` and `focus` characteristics.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Awareness {
    pub probability: f64,
    pub timestamp: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QualityStatus {
    Great,
    Good,
    Bad,
    NoContact,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelQuality {
    pub standard_deviation: f64,
    pub status: QualityStatus,
}

/// Keyed by channel name, not indexed by position.
pub type SignalQuality = BTreeMap<String, ChannelQuality>;

#[derive(Debug, Clone, Deserialize)]
pub struct PowerByBand {
    pub delta: Vec<f64>,
    pub theta: Vec<f64>,
    pub alpha: Vec<f64>,
    pub beta: Vec<f64>,
    pub gamma: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_info() {
        let json = r#"{"deviceId":"abc","deviceNickname":"Crown-1234",
            "channelNames":["CP3","C3","F5","PO3","PO4","F6","C4","CP4"],
            "channels":8,"samplingRate":256,"manufacturer":"Neurosity"}"#;
        let d: DeviceInfo = serde_json::from_str(json).unwrap();
        assert_eq!(d.channels, 8);
        assert_eq!(d.channel_names[0], "CP3");
        assert_eq!(d.sampling_rate, 256.0);
    }

    #[test]
    fn parses_awareness_metric() {
        let json = r#"{"metric":"awareness","label":"calm","probability":0.42,"timestamp":1700000000000}"#;
        let a: Awareness = serde_json::from_str(json).unwrap();
        assert_eq!(a.probability, 0.42);
    }

    #[test]
    fn parses_signal_quality_keyed_by_channel_name() {
        let json = r#"{"CP3":{"standardDeviation":8.1,"status":"great"},
                       "C3":{"standardDeviation":90.0,"status":"noContact"}}"#;
        let q: SignalQuality = serde_json::from_str(json).unwrap();
        assert_eq!(q["CP3"].status, QualityStatus::Great);
        assert_eq!(q["C3"].status, QualityStatus::NoContact);
    }

    #[test]
    fn parses_power_by_band() {
        let json = r#"{"delta":[1.0,2.0],"theta":[3.0,4.0],"alpha":[5.0,6.0],
                       "beta":[7.0,8.0],"gamma":[9.0,10.0]}"#;
        let p: PowerByBand = serde_json::from_str(json).unwrap();
        assert_eq!(p.alpha, vec![5.0, 6.0]);
    }
}
