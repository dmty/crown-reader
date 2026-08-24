//! Transport diagnostics, all inert unless their environment gate is set.
//!
//! Separated from the session logic because they share nothing with it: the
//! call sites are one line each, the state is private, and deleting this
//! module would leave `super` intact. Bluetooth delivered raw at 24% of
//! realtime with unbounded-growing latency, and while subscribed pushed the
//! metric streams from 1-233 ms of lag to 25-49 seconds.

use btleplug::api::Peripheral as _;
use btleplug::platform::Peripheral;
use uuid::Uuid;

use super::host_epoch_ms;
use super::{
    CHAR_AUTH, CHAR_CALM, CHAR_DEVICE_INFO, CHAR_FOCUS, CHAR_POWER_BY_BAND, CHAR_SIGNAL_QUALITY,
};

/// Prints every service and characteristic the device exposes, with its
/// properties, when `CROWN_GATT_DUMP` is set. Marks the six UUIDs this
/// crate knows so anything unrecognised stands out.
///
/// The six were taken from a fragment of the vendor SDK; the device has
/// never been asked directly what else it offers.
pub fn dump_gatt(p: &Peripheral) {
    if std::env::var_os("CROWN_GATT_DUMP").is_none() {
        return;
    }
    let mut characteristics: Vec<_> = p.characteristics().into_iter().collect();
    characteristics.sort_by_key(|c| (c.service_uuid, c.uuid));

    eprintln!("gatt dump: {} characteristics", characteristics.len());
    let mut service = None;
    for c in characteristics {
        if service != Some(c.service_uuid) {
            eprintln!("  service {}", c.service_uuid);
            service = Some(c.service_uuid);
        }
        let name = char_name(c.uuid);
        eprintln!("    {} [{:?}] {name}", c.uuid, c.properties);
    }
}

/// Every characteristic the device exposes, named. The six this crate
/// consumes plus the eleven it does not — the vendor SDK names all seventeen,
/// and `dump_gatt` prints them so an unrecognised UUID stands out as new
/// firmware rather than as an unknown.
const CHAR_NAMES: [(Uuid, &str); 17] = [
    (CHAR_AUTH, "auth"),
    (CHAR_DEVICE_INFO, "deviceInfo"),
    (CHAR_POWER_BY_BAND, "powerByBand"),
    (CHAR_FOCUS, "focus"),
    (CHAR_CALM, "calm"),
    (CHAR_SIGNAL_QUALITY, "signalQuality"),
    (
        Uuid::from_u128(0x009cf0bb_b68d_4af1_a0e5_625f2eb964a6),
        "raw",
    ),
    (
        Uuid::from_u128(0xd7e84cb2_ff37_4afc_9ed8_5577aeb84542),
        "deviceId",
    ),
    (
        Uuid::from_u128(0xd2e4b9e7_ab9d_4806_88a3_58584c1cf02b),
        "action",
    ),
    (
        Uuid::from_u128(0x1defa07f_2d1c_4e55_b981_eedabba7ae2b),
        "status",
    ),
    (
        Uuid::from_u128(0x014975ce_50df_4bfb_8ed4_a3437d619268),
        "settings",
    ),
    (
        Uuid::from_u128(0x84501dee_8665_4073_b111_bdecd69fb489),
        "accelerometer",
    ),
    (
        Uuid::from_u128(0x902ac5f3_ce59_4c11_94fa_437e89f90630),
        "signalQualityV2",
    ),
    (
        Uuid::from_u128(0x5472432e_3313_4169_add8_6fcb29accb0e),
        "rawUnfiltered",
    ),
    (
        Uuid::from_u128(0xd6684fb0_8518_40c0_8e88_4634e762435d),
        "psd",
    ),
    (
        Uuid::from_u128(0xf1cd519b_07dc_4f33_a285_286db2393359),
        "wifiNearbyNetworks",
    ),
    (
        Uuid::from_u128(0x37b2ce69_6fac_4547_91f3_8f1c527b875d),
        "wifiConnections",
    ),
];

fn char_name(uuid: Uuid) -> &'static str {
    CHAR_NAMES
        .iter()
        .find(|(u, _)| *u == uuid)
        .map(|(_, n)| *n)
        .unwrap_or("UNKNOWN")
}

/// Lag of the JSON metric path, printed every 8th `calm` update when
/// `CROWN_RAW_DEBUG` is set.
pub struct MetricProbe {
    enabled: bool,
    count: usize,
}

impl MetricProbe {
    pub fn new() -> Self {
        Self {
            enabled: std::env::var_os("CROWN_RAW_DEBUG").is_some(),
            count: 0,
        }
    }

    pub fn observe(&mut self, timestamp_ms: f64) {
        if !self.enabled {
            return;
        }
        self.count += 1;
        if !self.count.is_multiple_of(8) {
            return;
        }
        eprintln!(
            "metric probe: calm lag={:.0}ms",
            host_epoch_ms() as f64 - timestamp_ms
        );
    }
}
