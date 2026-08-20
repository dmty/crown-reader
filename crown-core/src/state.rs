use std::path::PathBuf;

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
const MAX_CHANNELS: usize = 64;

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
    rings: Vec<ChannelRing>,
    rev: u64,
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
        let channels = info.channels.min(info.channel_names.len()).min(MAX_CHANNELS);
        if channels == 0 {
            self.dropped_frames += 1;
            self.touch();
            return;
        }

        let sampling_rate = if info.sampling_rate.is_finite() {
            info.sampling_rate.clamp(MIN_SAMPLING_RATE_HZ, MAX_SAMPLING_RATE_HZ)
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
        if s.data.len() != self.rings.len() || self.rings.is_empty() || s.data.iter().any(|v| !(*v as f32).is_finite()) {
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

    /// Call after any mutation so pollers can skip unchanged state.
    pub fn touch(&mut self) {
        self.rev += 1;
    }

    pub fn snapshot(&self, width_px: usize) -> Snapshot {
        Snapshot {
            connection: self.connection,
            device_name: self.device.as_ref().map(|d| d.device_nickname.clone()),
            channel_names: self
                .device
                .as_ref()
                .map(|d| d.channel_names.clone())
                .unwrap_or_default(),
            quality: self.quality.clone(),
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
    use crate::streams::DeviceInfo;

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
            live.push_raw(&RawSample { timestamp: i, marker: 0, data: vec![1.0, -1.0] });
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
        live.push_raw(&RawSample { timestamp: 1, marker: 0, data: vec![1.0, 2.0, 3.0] });
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
        assert!(snap.rev > before_rev, "the drop must still be observable via rev");
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
        live.push_raw(&RawSample { timestamp: 1, marker: 0, data: vec![1.0, 2.0] });
        assert!(live.snapshot(10).rev > before);
    }

    #[test]
    fn raw_samples_counts_accepted_but_not_rejected_frames() {
        let mut live = Live::new();
        live.configure(info(2));
        assert_eq!(live.snapshot(10).raw_samples, 0);

        live.push_raw(&RawSample { timestamp: 1, marker: 0, data: vec![1.0, 2.0] });
        assert_eq!(live.snapshot(10).raw_samples, 1);

        // Wrong channel count: rejected, must not advance the counter.
        live.push_raw(&RawSample { timestamp: 2, marker: 0, data: vec![1.0, 2.0, 3.0] });
        assert_eq!(live.snapshot(10).raw_samples, 1);
    }

    #[test]
    fn push_raw_drops_a_sample_containing_non_finite_values() {
        let mut live = Live::new();
        live.configure(info(2));
        live.push_raw(&RawSample { timestamp: 1, marker: 0, data: vec![1.0, f64::NAN] });
        live.push_raw(&RawSample { timestamp: 2, marker: 0, data: vec![f64::INFINITY, 1.0] });
        live.push_raw(&RawSample { timestamp: 3, marker: 0, data: vec![f64::NEG_INFINITY, 1.0] });
        let snap = live.snapshot(10);
        assert_eq!(snap.dropped_frames, 3);
        assert!(snap.waveform[0].is_empty());
        assert!(snap.waveform[1].is_empty());

        // 1e300 is itself a finite f64 — `is_finite()` on the raw sample would
        // let it through — but `as f32` saturates it to +inf, so the guard
        // must check finiteness on the cast value that actually reaches the
        // ring, not on the wire value.
        live.push_raw(&RawSample { timestamp: 4, marker: 0, data: vec![1e300, 1.0] });
        assert_eq!(live.snapshot(10).dropped_frames, 4);
        assert!(live.snapshot(10).waveform[0].is_empty());

        live.push_raw(&RawSample { timestamp: 5, marker: 0, data: vec![1.0, 2.0] });
        assert!(!live.snapshot(10).waveform[0].is_empty());
    }
}
