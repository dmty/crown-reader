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
    /// Any status label the device sends that isn't one of the four above.
    /// Without this catch-all, one unrecognised label fails `SignalQuality`
    /// (a `BTreeMap`)'s *entire* parse — every electrode tile goes grey, not
    /// just the one with the odd label — since a single bad map entry fails
    /// the whole map in serde's default `Deserialize` for `BTreeMap`. QML
    /// already renders any status it doesn't recognise (this included) as a
    /// grey tile, so degrading here is a strict improvement: seven honest
    /// readings and one grey tile beats eight grey tiles.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelQuality {
    pub standard_deviation: f64,
    pub status: QualityStatus,
}

/// Keyed by whatever string the device uses per channel. Real firmware sends
/// positions ("0".."7"); `Live::snapshot` relabels those onto
/// `DeviceInfo::channel_names`, which is the only place both are in hand.
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
    fn parses_signal_quality_entries() {
        let json = r#"{"CP3":{"standardDeviation":8.1,"status":"great"},
                       "C3":{"standardDeviation":90.0,"status":"noContact"}}"#;
        let q: SignalQuality = serde_json::from_str(json).unwrap();
        assert_eq!(q["CP3"].status, QualityStatus::Great);
        assert_eq!(q["C3"].status, QualityStatus::NoContact);
    }

    #[test]
    fn an_unrecognised_quality_label_degrades_to_unknown_instead_of_failing_the_whole_map() {
        // Without `#[serde(other)]` on `QualityStatus::Unknown`, the
        // unrecognised "somethingNew" label below fails `SignalQuality`'s
        // entire parse -- not just the C3 entry -- which is exactly the bug
        // this variant exists to prevent.
        let json = r#"{"CP3":{"standardDeviation":8.1,"status":"great"},
                       "C3":{"standardDeviation":50.0,"status":"somethingNew"}}"#;
        let q: SignalQuality = serde_json::from_str(json).unwrap();
        assert_eq!(q["CP3"].status, QualityStatus::Great);
        assert_eq!(q["C3"].status, QualityStatus::Unknown);
    }

    #[test]
    fn quality_status_debug_output_matches_what_the_qt_bridge_and_qml_depend_on() {
        // crown-qt's bridge publishes `format!("{:?}", status)` and
        // Metrics.qml string-compares the result against these exact
        // literals ("Great"/"Good"/"Bad"/"NoContact", falling through to a
        // grey tile for anything else, `Unknown` included). Pinning `Debug`
        // here means a variant rename breaks this test instead of silently
        // turning every tile grey with no compile error.
        assert_eq!(format!("{:?}", QualityStatus::Great), "Great");
        assert_eq!(format!("{:?}", QualityStatus::Good), "Good");
        assert_eq!(format!("{:?}", QualityStatus::Bad), "Bad");
        assert_eq!(format!("{:?}", QualityStatus::NoContact), "NoContact");
        assert_eq!(format!("{:?}", QualityStatus::Unknown), "Unknown");
    }

    #[test]
    fn parses_power_by_band() {
        let json = r#"{"delta":[1.0,2.0],"theta":[3.0,4.0],"alpha":[5.0,6.0],
                       "beta":[7.0,8.0],"gamma":[9.0,10.0]}"#;
        let p: PowerByBand = serde_json::from_str(json).unwrap();
        assert_eq!(p.alpha, vec![5.0, 6.0]);
    }
}
