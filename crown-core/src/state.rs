use std::path::PathBuf;

use crate::decimate::decimate;
use crate::raw::RawSample;
use crate::ring::ChannelRing;
use crate::streams::{DeviceInfo, PowerByBand, SignalQuality};

/// Seconds of raw signal held in memory for the waveform.
const RING_SECONDS: f64 = 10.0;

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
            recording: None,
            rings: Vec::new(),
            rev: 0,
        }
    }

    /// Sizes every per-channel structure from the device's own report.
    pub fn configure(&mut self, info: DeviceInfo) {
        let cap = ((info.sampling_rate * RING_SECONDS) as usize).max(1);
        self.rings = (0..info.channels).map(|_| ChannelRing::new(cap)).collect();
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
        for rate in [0.0, -1.0, f64::NAN] {
            let mut d = info(2);
            d.sampling_rate = rate;
            live.configure(d);
            assert_eq!(live.snapshot(10).waveform.len(), 2);
        }
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

        live.push_raw(&RawSample { timestamp: 4, marker: 0, data: vec![1.0, 2.0] });
        assert!(!live.snapshot(10).waveform[0].is_empty());
    }
}
