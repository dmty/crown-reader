use std::path::PathBuf;
use std::time::Instant;

use crate::decimate::decimate;
use crate::raw::RawSample;
use crate::ring::ChannelRing;
use crate::streams::{DeviceInfo, PowerByBand, SignalQuality};

/// Seconds of raw signal held in memory for the waveform.
const RING_SECONDS: f64 = 10.0;

/// Sane bounds for a device's self-reported sampling rate. The report
/// arrives as untrusted BLE JSON; without a ceiling, a corrupt or hostile
/// value can make the capacity computation below request an unreasonable
/// (or overflowing) allocation. The Crown reports 256.
const MIN_SAMPLING_RATE_HZ: f64 = 1.0;
const MAX_SAMPLING_RATE_HZ: f64 = 100_000.0;

/// Upper bound on channel count. The Crown has 8; this is headroom against
/// a corrupt report, not a real limit.
pub(crate) const MAX_CHANNELS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Scanning,
    Connecting,
    Authenticating,
    Streaming,
    Reconnecting,
    Failed,
}

impl ConnectionState {
    /// Whether a BLE session (the task `supervise` spawns) is actually
    /// running. `Disconnected` and `Failed` are the only idle states — the
    /// initial, never-connected state, and where a terminal error (or a
    /// fresh `Live`) leaves things once the task has ended — so this is
    /// their negation rather than a whitelist of the running states: a
    /// value this doesn't recognize should read as active, not idle.
    pub fn is_active(self) -> bool {
        !matches!(
            self,
            ConnectionState::Disconnected | ConnectionState::Failed
        )
    }
}

/// The complete core-to-UI contract. Contains no UI types by design.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub connection: ConnectionState,
    pub device_name: Option<String>,
    pub channel_names: Vec<String>,
    pub quality: SignalQuality,
    pub bands: Option<PowerByBand>,
    pub calm: f32,
    pub focus: f32,
    pub waveform: Vec<Vec<(f32, f32)>>,
    pub raw_enabled: bool,
    pub dropped_frames: u64,
    pub raw_samples: u64,
    /// How far behind the metric stream has fallen, in milliseconds, or
    /// `None` before the first metric arrives.
    ///
    /// Measured against the smallest device-to-host offset seen this
    /// session rather than against the host clock directly, so a constant
    /// difference between the two clocks reads as 0 rather than as
    /// permanent staleness. A healthy stream sits near zero; a stream the
    /// link can no longer carry climbs without bound.
    pub metric_staleness_ms: Option<i64>,
    pub recording: Option<PathBuf>,
    pub rev: u64,
}

pub struct Live {
    pub connection: ConnectionState,
    pub device: Option<DeviceInfo>,
    pub quality: SignalQuality,
    pub bands: Option<PowerByBand>,
    pub calm: f32,
    pub focus: f32,
    pub raw_enabled: bool,
    pub dropped_frames: u64,
    pub raw_samples: u64,
    pub recording: Option<PathBuf>,
    /// Smallest `host - device` offset seen this session, and the current
    /// staleness measured from it. Kept separate from the clock itself:
    /// callers pass both timestamps in, so this stays deterministic and
    /// testable rather than reading the wall clock here.
    metric_offset_floor: Option<i64>,
    metric_staleness_ms: Option<i64>,
    /// When the current run last transitioned into `Streaming`, if it has.
    /// Set by `ble::run` on that transition and cleared back to `None` when
    /// a fresh run starts scanning, so a caller measuring how long a session
    /// actually streamed (as opposed to how long the whole attempt,
    /// including scanning and auth, took) has a reliable signal to read
    /// after the run ends.
    pub streaming_since: Option<Instant>,
    rings: Vec<ChannelRing>,
    rev: u64,
}

/// Relabels a positional signal-quality map onto the device's channel names.
///
/// The firmware keys `signalQuality` by position ("0".."7"), not by the names
/// `deviceInfo` carries, so every name lookup misses and the whole electrode
/// display goes grey. Relabelling here, rather than at ingest, is what makes
/// it safe: the two characteristics arrive independently and can race, and
/// only a snapshot has both in hand at once.
///
/// A key that is not an index, or points past the channel list, passes
/// through untouched — firmware that starts sending real names keeps working.
fn label_quality(quality: &SignalQuality, names: &[String]) -> SignalQuality {
    quality
        .iter()
        .map(|(key, q)| {
            let name = key
                .parse::<usize>()
                .ok()
                .and_then(|i| names.get(i))
                .unwrap_or(key);
            (name.clone(), *q)
        })
        .collect()
}

impl Live {
    pub fn new() -> Self {
        Self {
            connection: ConnectionState::Disconnected,
            device: None,
            quality: SignalQuality::new(),
            bands: None,
            calm: 0.0,
            focus: 0.0,
            raw_enabled: false,
            dropped_frames: 0,
            raw_samples: 0,
            recording: None,
            metric_offset_floor: None,
            metric_staleness_ms: None,
            streaming_since: None,
            rings: Vec::new(),
            rev: 0,
        }
    }

    /// Sizes every per-channel structure from the device's own report. The
    /// report is untrusted, so this is also where it gets sanitized:
    /// `channels` is reconciled against `channel_names.len()` (the smaller
    /// wins) and capped at `MAX_CHANNELS`, so `Snapshot.waveform.len() ==
    /// Snapshot.channel_names.len()` always holds downstream. A report with
    /// no usable channels after reconciliation is dropped rather than
    /// applied: the previous configuration (or the unconfigured state) is
    /// kept, and the drop is counted via `dropped_frames` so it isn't silent.
    pub fn configure(&mut self, mut info: DeviceInfo) {
        let channels = info
            .channels
            .min(info.channel_names.len())
            .min(MAX_CHANNELS);
        if channels == 0 {
            self.dropped_frames += 1;
            self.touch();
            return;
        }

        let sampling_rate = if info.sampling_rate.is_finite() {
            info.sampling_rate
                .clamp(MIN_SAMPLING_RATE_HZ, MAX_SAMPLING_RATE_HZ)
        } else {
            MIN_SAMPLING_RATE_HZ
        };

        info.channels = channels;
        info.channel_names.truncate(channels);
        info.sampling_rate = sampling_rate;

        let cap = (sampling_rate * RING_SECONDS) as usize;
        self.rings = (0..channels).map(|_| ChannelRing::new(cap)).collect();
        self.device = Some(info);
        self.touch();
    }

    pub fn push_raw(&mut self, s: &RawSample) {
        if self.rings.is_empty() {
            // Samples can arrive before configure() has sized the rings (the
            // OSC listener starts before the device's channel-count report
            // does). That's the normal startup window, not a dropped frame.
            return;
        }
        if s.data.len() != self.rings.len() || s.data.iter().any(|v| !(*v as f32).is_finite()) {
            self.dropped_frames += 1;
            self.touch();
            return;
        }
        for (ring, v) in self.rings.iter_mut().zip(&s.data) {
            ring.push(*v as f32);
        }
        self.raw_samples += 1;
        self.touch();
    }

    /// Records that a metric carrying `device_ms` arrived when the host
    /// clock read `host_ms`.
    ///
    /// Does not `touch()`: every caller already does so for the value the
    /// metric carried, and a second bump would only cost pollers a redundant
    /// snapshot.
    pub fn note_metric_time(&mut self, device_ms: f64, host_ms: i64) {
        if !device_ms.is_finite() {
            return;
        }
        let offset = host_ms - device_ms as i64;
        let floor = self.metric_offset_floor.get_or_insert(offset);
        *floor = (*floor).min(offset);
        self.metric_staleness_ms = Some(offset - *floor);
    }

    /// Forgets the device-to-host clock relationship learned this session.
    ///
    /// `Live` outlives a single connection — the reconnect supervisor reuses
    /// it — so without this the floor stays at the previous session's
    /// minimum. A device whose clock resynced across the reconnect would
    /// then read as permanently stale, or silently re-floor and hide real
    /// staleness, depending on which way it moved.
    pub fn forget_metric_clock(&mut self) {
        self.metric_offset_floor = None;
        self.metric_staleness_ms = None;
    }

    /// Call after any mutation so pollers can skip unchanged state.
    pub fn touch(&mut self) {
        self.rev += 1;
    }

    /// Current revision, without building a `Snapshot`. Lets a poller skip
    /// the snapshot itself (waveform decimation included) when nothing has
    /// changed, rather than building one only to discard it.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// Runs with the caller holding the `Live` mutex — the same one the BLE
    /// metric loop and the OSC listener take on every notification, so a
    /// slow render here is itself a contributor to notification lag.
    pub fn snapshot(&self, width_px: usize) -> Snapshot {
        let channel_names = self
            .device
            .as_ref()
            .map(|d| d.channel_names.clone())
            .unwrap_or_default();
        Snapshot {
            connection: self.connection,
            device_name: self.device.as_ref().map(|d| d.device_nickname.clone()),
            quality: label_quality(&self.quality, &channel_names),
            channel_names,
            bands: self.bands.clone(),
            calm: self.calm,
            focus: self.focus,
            waveform: self
                .rings
                .iter()
                .map(|r| decimate(&r.to_vec(), width_px))
                .collect(),
            raw_enabled: self.raw_enabled,
            dropped_frames: self.dropped_frames,
            raw_samples: self.raw_samples,
            metric_staleness_ms: self.metric_staleness_ms,
            recording: self.recording.clone(),
            rev: self.rev,
        }
    }
}

impl Default for Live {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::RawSample;
    use crate::streams::{ChannelQuality, DeviceInfo, QualityStatus};

    #[test]
    fn positional_quality_keys_are_relabelled_onto_the_channel_names() {
        // Real firmware keys this map "0".."7"; a name lookup against it
        // misses every entry and the whole electrode display goes grey.
        let mut live = Live::new();
        live.configure(info(2));
        live.quality = [
            ("0".to_string(), quality(QualityStatus::Great)),
            ("1".to_string(), quality(QualityStatus::NoContact)),
        ]
        .into_iter()
        .collect();

        let snap = live.snapshot(10);
        assert_eq!(snap.quality["CH0"].status, QualityStatus::Great);
        assert_eq!(snap.quality["CH1"].status, QualityStatus::NoContact);
    }

    #[test]
    fn quality_keys_that_are_not_positions_pass_through_untouched() {
        // Firmware that starts sending real names, and an index past the end
        // of the channel list, both have to survive rather than vanish.
        let mut live = Live::new();
        live.configure(info(2));
        live.quality = [
            ("CP3".to_string(), quality(QualityStatus::Good)),
            ("7".to_string(), quality(QualityStatus::Bad)),
        ]
        .into_iter()
        .collect();

        let snap = live.snapshot(10);
        assert_eq!(snap.quality["CP3"].status, QualityStatus::Good);
        assert_eq!(snap.quality["7"].status, QualityStatus::Bad);
    }

    #[test]
    fn quality_relabelling_is_a_no_op_before_device_info_arrives() {
        let mut live = Live::new();
        live.quality = [("0".to_string(), quality(QualityStatus::Great))]
            .into_iter()
            .collect();
        assert_eq!(live.snapshot(10).quality["0"].status, QualityStatus::Great);
    }

    fn quality(status: QualityStatus) -> ChannelQuality {
        ChannelQuality {
            standard_deviation: 1.0,
            status,
        }
    }

    fn info(channels: usize) -> DeviceInfo {
        DeviceInfo {
            device_id: "abc".into(),
            device_nickname: "Crown-1234".into(),
            channel_names: (0..channels).map(|i| format!("CH{i}")).collect(),
            channels,
            sampling_rate: 256.0,
        }
    }

    #[test]
    fn is_active_is_false_only_for_the_two_idle_states() {
        use ConnectionState::*;
        for s in [
            Scanning,
            Connecting,
            Authenticating,
            Streaming,
            Reconnecting,
        ] {
            assert!(s.is_active(), "{s:?} should be active");
        }
        for s in [Disconnected, Failed] {
            assert!(!s.is_active(), "{s:?} should be idle");
        }
    }

    #[test]
    fn starts_disconnected_and_empty() {
        let live = Live::new();
        let snap = live.snapshot(100);
        assert_eq!(snap.connection, ConnectionState::Disconnected);
        assert!(snap.waveform.is_empty());
        assert!(snap.channel_names.is_empty());
    }

    #[test]
    fn sizes_itself_from_device_info() {
        let mut live = Live::new();
        live.configure(info(4));
        let snap = live.snapshot(50);
        assert_eq!(snap.waveform.len(), 4);
        assert_eq!(snap.channel_names.len(), 4);
    }

    #[test]
    fn raw_samples_reach_the_waveform() {
        let mut live = Live::new();
        live.configure(info(2));
        for i in 0..500 {
            live.push_raw(&RawSample {
                timestamp: i,
                marker: 0,
                data: vec![1.0, -1.0],
            });
        }
        let snap = live.snapshot(10);
        assert_eq!(snap.waveform.len(), 2);
        assert_eq!(snap.waveform[0].len(), 10);
        assert_eq!(snap.waveform[0][0], (1.0, 1.0));
        assert_eq!(snap.waveform[1][0], (-1.0, -1.0));
    }

    #[test]
    fn a_sample_with_the_wrong_channel_count_is_dropped_not_panicked_on() {
        let mut live = Live::new();
        live.configure(info(2));
        live.push_raw(&RawSample {
            timestamp: 1,
            marker: 0,
            data: vec![1.0, 2.0, 3.0],
        });
        assert_eq!(live.snapshot(10).dropped_frames, 1);
    }

    #[test]
    fn rev_increments_when_state_changes() {
        let mut live = Live::new();
        let before = live.snapshot(10).rev;
        live.configure(info(2));
        assert!(live.snapshot(10).rev > before);
    }

    #[test]
    fn configure_survives_a_nonsense_sampling_rate() {
        let mut live = Live::new();
        // 0, negative, and NaN are rejected outright; 1e300 is finite but
        // far outside any sane range and must be clamped instead of
        // producing a multi-terabyte (or overflowing) ring capacity.
        for rate in [0.0, -1.0, f64::NAN, 1e300] {
            let mut d = info(2);
            d.sampling_rate = rate;
            live.configure(d);
            assert_eq!(live.snapshot(10).waveform.len(), 2);
            assert_eq!(live.snapshot(10).channel_names.len(), 2);
        }
    }

    #[test]
    fn configure_with_zero_usable_channels_is_dropped_not_applied() {
        let mut live = Live::new();
        let before_rev = live.snapshot(10).rev;
        live.configure(info(0));
        let snap = live.snapshot(10);
        assert_eq!(snap.dropped_frames, 1);
        assert!(snap.device_name.is_none());
        assert!(snap.waveform.is_empty());
        assert!(snap.channel_names.is_empty());
        assert!(
            snap.rev > before_rev,
            "the drop must still be observable via rev"
        );
    }

    #[test]
    fn configure_reconciles_a_channels_and_channel_names_mismatch() {
        let mut live = Live::new();
        let mut d = info(4);
        d.channels = 6; // claims 6 channels but only reports 4 names
        live.configure(d);
        let snap = live.snapshot(10);
        assert_eq!(snap.waveform.len(), 4);
        assert_eq!(snap.channel_names.len(), 4);
    }

    #[test]
    fn configure_bounds_an_excessive_channel_count() {
        let mut live = Live::new();
        live.configure(info(1000));
        let snap = live.snapshot(10);
        assert_eq!(snap.waveform.len(), MAX_CHANNELS);
        assert_eq!(snap.channel_names.len(), MAX_CHANNELS);
    }

    #[test]
    fn rev_increments_on_push_raw() {
        let mut live = Live::new();
        live.configure(info(2));
        let before = live.snapshot(10).rev;
        live.push_raw(&RawSample {
            timestamp: 1,
            marker: 0,
            data: vec![1.0, 2.0],
        });
        assert!(live.snapshot(10).rev > before);
    }

    #[test]
    fn metric_staleness_is_zero_when_the_stream_keeps_up_despite_clock_skew() {
        let mut live = Live::new();
        // Device clock runs 5s ahead of the host. That is skew, not lag, and
        // must not be reported as staleness.
        live.note_metric_time(105_000.0, 100_000);
        live.note_metric_time(106_000.0, 101_000);
        live.note_metric_time(107_000.0, 102_000);
        assert_eq!(live.snapshot(10).metric_staleness_ms, Some(0));
    }

    #[test]
    fn metric_staleness_grows_when_the_stream_falls_behind() {
        let mut live = Live::new();
        live.note_metric_time(100_000.0, 100_000);
        assert_eq!(live.snapshot(10).metric_staleness_ms, Some(0));

        // Host advances 10s; the newest metric is only 2s newer, so 8s of
        // backlog has accumulated.
        live.note_metric_time(102_000.0, 110_000);
        assert_eq!(live.snapshot(10).metric_staleness_ms, Some(8_000));
    }

    #[test]
    fn a_reconnect_forgets_the_previous_session_clock_offset() {
        let mut live = Live::new();
        live.note_metric_time(100_000.0, 100_000);
        live.note_metric_time(102_000.0, 110_000);
        assert_eq!(live.snapshot(10).metric_staleness_ms, Some(8_000));

        // A fresh session: the device clock may have resynced, so the old
        // floor is meaningless and must not carry over.
        live.forget_metric_clock();
        assert_eq!(live.snapshot(10).metric_staleness_ms, None);

        // A device now running 30s behind the host is a new baseline, not
        // 30s of staleness inherited from the last session.
        live.note_metric_time(70_000.0, 100_000);
        assert_eq!(live.snapshot(10).metric_staleness_ms, Some(0));
    }

    #[test]
    fn metric_staleness_is_none_until_a_metric_arrives() {
        assert_eq!(Live::new().snapshot(10).metric_staleness_ms, None);
    }

    #[test]
    fn a_non_finite_metric_timestamp_is_ignored_rather_than_poisoning_the_floor() {
        let mut live = Live::new();
        live.note_metric_time(100_000.0, 100_000);
        live.note_metric_time(f64::NAN, 101_000);
        live.note_metric_time(f64::INFINITY, 102_000);
        // Still measuring from the one good offset, not from a garbage floor.
        live.note_metric_time(101_000.0, 101_000);
        assert_eq!(live.snapshot(10).metric_staleness_ms, Some(0));
    }

    #[test]
    fn push_raw_before_configure_is_not_a_dropped_frame() {
        let mut live = Live::new();
        // No configure() call: rings are unsized, mirroring the window
        // between the OSC listener starting and the device-info reply.
        live.push_raw(&RawSample {
            timestamp: 1,
            marker: 0,
            data: vec![1.0, 2.0],
        });
        assert_eq!(live.snapshot(10).dropped_frames, 0);

        // A genuine width mismatch after configuration still counts.
        live.configure(info(2));
        live.push_raw(&RawSample {
            timestamp: 2,
            marker: 0,
            data: vec![1.0, 2.0, 3.0],
        });
        assert_eq!(live.snapshot(10).dropped_frames, 1);
    }

    #[test]
    fn raw_samples_counts_accepted_but_not_rejected_frames() {
        let mut live = Live::new();
        live.configure(info(2));
        assert_eq!(live.snapshot(10).raw_samples, 0);

        live.push_raw(&RawSample {
            timestamp: 1,
            marker: 0,
            data: vec![1.0, 2.0],
        });
        assert_eq!(live.snapshot(10).raw_samples, 1);

        // Wrong channel count: rejected, must not advance the counter.
        live.push_raw(&RawSample {
            timestamp: 2,
            marker: 0,
            data: vec![1.0, 2.0, 3.0],
        });
        assert_eq!(live.snapshot(10).raw_samples, 1);
    }

    #[test]
    fn push_raw_drops_a_sample_containing_non_finite_values() {
        let mut live = Live::new();
        live.configure(info(2));
        live.push_raw(&RawSample {
            timestamp: 1,
            marker: 0,
            data: vec![1.0, f64::NAN],
        });
        live.push_raw(&RawSample {
            timestamp: 2,
            marker: 0,
            data: vec![f64::INFINITY, 1.0],
        });
        live.push_raw(&RawSample {
            timestamp: 3,
            marker: 0,
            data: vec![f64::NEG_INFINITY, 1.0],
        });
        let snap = live.snapshot(10);
        assert_eq!(snap.dropped_frames, 3);
        assert!(snap.waveform[0].is_empty());
        assert!(snap.waveform[1].is_empty());

        // 1e300 is itself a finite f64 — `is_finite()` on the raw sample would
        // let it through — but `as f32` saturates it to +inf, so the guard
        // must check finiteness on the cast value that actually reaches the
        // ring, not on the wire value.
        live.push_raw(&RawSample {
            timestamp: 4,
            marker: 0,
            data: vec![1e300, 1.0],
        });
        assert_eq!(live.snapshot(10).dropped_frames, 4);
        assert!(live.snapshot(10).waveform[0].is_empty());

        live.push_raw(&RawSample {
            timestamp: 5,
            marker: 0,
            data: vec![1.0, 2.0],
        });
        assert!(!live.snapshot(10).waveform[0].is_empty());
    }
}
