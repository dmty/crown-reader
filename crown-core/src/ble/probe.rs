//! Transport diagnostics, all inert unless their environment gate is set.
//!
//! Separated from the session logic because they share nothing with it: the
//! call sites are one line each, the state is private, and deleting this
//! module would leave `super` intact.

use std::time::Instant;

use btleplug::api::Peripheral as _;
use btleplug::platform::Peripheral;
use uuid::Uuid;

use super::{
    CHAR_AUTH, CHAR_CALM, CHAR_DEVICE_INFO, CHAR_FOCUS, CHAR_POWER_BY_BAND, CHAR_RAW,
    CHAR_SIGNAL_QUALITY,
};
use crate::raw::RawSample;

/// Prints every service and characteristic the device exposes, with its
/// properties, when `CROWN_GATT_DUMP` is set. Marks the seven UUIDs this
/// crate knows so anything unrecognised stands out.
///
/// The seven were taken from a fragment of the vendor SDK; the device has
/// never been asked directly what else it offers. A lower-rate or
/// lower-channel-count variant of the raw stream would show up here, and
/// would matter a great deal — the full raw stream needs four times the
/// bandwidth this link delivers.
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

/// Every characteristic the device exposes, named. The seven this crate
/// consumes plus the ten it does not — the vendor SDK names all seventeen,
/// and `dump_gatt` prints them so an unrecognised UUID stands out as new
/// firmware rather than as an unknown.
const CHAR_NAMES: [(Uuid, &str); 17] = [
    (CHAR_AUTH, "auth"),
    (CHAR_DEVICE_INFO, "deviceInfo"),
    (CHAR_RAW, "raw"),
    (CHAR_POWER_BY_BAND, "powerByBand"),
    (CHAR_FOCUS, "focus"),
    (CHAR_CALM, "calm"),
    (CHAR_SIGNAL_QUALITY, "signalQuality"),
    (Uuid::from_u128(0xd7e84cb2_ff37_4afc_9ed8_5577aeb84542), "deviceId"),
    (Uuid::from_u128(0xd2e4b9e7_ab9d_4806_88a3_58584c1cf02b), "action"),
    (Uuid::from_u128(0x1defa07f_2d1c_4e55_b981_eedabba7ae2b), "status"),
    (Uuid::from_u128(0x014975ce_50df_4bfb_8ed4_a3437d619268), "settings"),
    (Uuid::from_u128(0x84501dee_8665_4073_b111_bdecd69fb489), "accelerometer"),
    (Uuid::from_u128(0x902ac5f3_ce59_4c11_94fa_437e89f90630), "signalQualityV2"),
    (Uuid::from_u128(0x5472432e_3313_4169_add8_6fcb29accb0e), "rawUnfiltered"),
    (Uuid::from_u128(0xd6684fb0_8518_40c0_8e88_4634e762435d), "psd"),
    (Uuid::from_u128(0xf1cd519b_07dc_4f33_a285_286db2393359), "wifiNearbyNetworks"),
    (Uuid::from_u128(0x37b2ce69_6fac_4547_91f3_8f1c527b875d), "wifiConnections"),
];

fn char_name(uuid: Uuid) -> &'static str {
    CHAR_NAMES.iter().find(|(u, _)| *u == uuid).map(|(_, n)| *n).unwrap_or("UNKNOWN")
}

/// Host clock in epoch milliseconds, matching how `record.rs` stamps its
/// clock anchor — the probes compare device timestamps against this.
fn host_epoch_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

/// Whether the transport probes should report.
fn raw_debug() -> bool {
    std::env::var_os("CROWN_RAW_DEBUG").is_some()
}

/// Lag of the JSON metric path, printed every 8th `calm` update when
/// `CROWN_RAW_DEBUG` is set.
///
/// Raw and the metric streams share one link, so this answers a question the
/// raw probe cannot: whether enabling raw also delays the numbers the reader
/// actually displays. Run once with `--raw` and once without to compare.
pub struct MetricProbe {
    enabled: bool,
    count: usize,
}

impl MetricProbe {
    pub fn new() -> Self {
        Self {
            enabled: raw_debug(),
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
        eprintln!("metric probe: calm lag={:.0}ms", host_epoch_ms() as f64 - timestamp_ms);
    }
}

/// Health of the raw stream, reported every `PROBE_WINDOW` samples when
/// `CROWN_RAW_DEBUG` is set.
///
/// Two numbers, both of which the per-second sample rate alone cannot give:
///
/// - `device/wall` — 1.0 means the stream keeps pace with real time. Below
///   that the device produces faster than the link delivers and a backlog is
///   building.
/// - `lag` — how far the newest sample sits behind the host clock. Lag that
///   grows is an unbounded backlog; lag that holds steady is a fixed clock
///   offset between device and host, which is harmless.
pub struct RawProbe {
    enabled: bool,
    prev: Option<u64>,
    count: usize,
    window_start: Instant,
    window_first_timestamp: Option<u64>,
}

/// Samples per report: roughly nine seconds at the rate the link delivers.
const PROBE_WINDOW: usize = 600;

impl RawProbe {
    pub fn new() -> Self {
        Self {
            enabled: raw_debug(),
            prev: None,
            count: 0,
            window_start: Instant::now(),
            window_first_timestamp: None,
        }
    }

    pub fn observe(&mut self, samples: &[RawSample]) {
        if !self.enabled {
            return;
        }
        for s in samples {
            if self.window_first_timestamp.is_none() {
                self.window_first_timestamp = Some(s.timestamp);
            }
            self.prev = Some(s.timestamp);
            self.count += 1;
        }
        if self.count >= PROBE_WINDOW {
            self.report();
            self.reset();
        }
    }

    fn report(&self) {
        let wall = self.window_start.elapsed().as_secs_f64().max(0.001);
        let device = match (self.window_first_timestamp, self.prev) {
            (Some(first), Some(last)) => last.saturating_sub(first) as f64 / 1000.0,
            _ => 0.0,
        };
        let lag = self.prev.map(|last| host_epoch_ms() - last as i64);
        eprintln!(
            "raw probe: {} samples, {device:.1}s device / {wall:.1}s wall = {:.2}x realtime, lag={}",
            self.count,
            device / wall,
            lag.map(|l| format!("{l}ms")).unwrap_or_else(|| "n/a".into()),
        );
    }

    fn reset(&mut self) {
        self.count = 0;
        self.window_start = Instant::now();
        // Keeps `prev`: the gap between windows is a real interval, and
        // dropping it would hide a stall at the seam.
        self.window_first_timestamp = self.prev;
    }
}

