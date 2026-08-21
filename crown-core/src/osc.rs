//! Raw EEG over OSC, the transport that replaced Bluetooth for this stream.
//!
//! Measured against hardware: ~250 samples/s at ~2% packet loss with stable
//! latency, against Bluetooth's 24% of realtime and unbounded backlog.
//!
//! The device broadcasts to the subnet, so anything on the LAN can receive
//! it — including packets from someone else's headset. Every message is
//! matched against our own device id before it is trusted.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rosc::{OscPacket, OscType};

use crate::filter::{ChannelFilter, FilterConfig};
use crate::raw::RawSample;
use crate::record::Recorder;
use crate::state::Live;

/// The device broadcasts here and offers no way to configure a destination.
pub const OSC_PORT: u16 = 9000;

/// The counter carried by each message wraps at 32 — established on
/// hardware. Reading it as mod 256 makes every wrap look like a 224-packet
/// loss, which is exactly the mistake that first reported 87% loss on a link
/// actually dropping 2%.
pub const COUNTER_MODULUS: i32 = 32;

#[derive(Debug, Clone, PartialEq)]
pub struct OscSample {
    pub sample: RawSample,
    /// Sequence counter mod `COUNTER_MODULUS`. UDP drops rather than queues,
    /// so a gap here is the only evidence a sample went missing.
    pub counter: i32,
}

/// Decodes one datagram, or `None` if it is not a raw sample from
/// `device_id`. Silence is the right response to a foreign or malformed
/// packet on a broadcast port: it is not an error condition, it is traffic.
pub fn decode_raw(packet_bytes: &[u8], device_id: &str) -> Option<OscSample> {
    let (_, packet) = rosc::decoder::decode_udp(packet_bytes).ok()?;
    let OscPacket::Message(message) = packet else {
        return None;
    };
    if message.addr != format!("/neurosity/notion/{device_id}/raw") {
        return None;
    }

    // Eight channels, then the timestamp string, counter, and marker.
    let (values, tail) = message.args.split_at_checked(8)?;
    let data: Vec<f64> = values
        .iter()
        .map(|a| match a {
            OscType::Float(f) => Some(*f as f64),
            _ => None,
        })
        .collect::<Option<_>>()?;

    // Milliseconds, as a decimal string: "1787270434786.832". Parsing the
    // integer part as u64 directly is lossless, and it rejects NaN,
    // infinity, negatives, and scientific notation by construction --
    // none of those parse as u64, so there is nothing left to guard.
    let [OscType::String(ts), OscType::Int(counter), ..] = tail else {
        return None;
    };
    let integer_part = match ts.split_once('.') {
        Some((int, frac)) if frac.bytes().all(|b| b.is_ascii_digit()) => int,
        Some(_) => return None,
        None => ts.as_str(),
    };
    let timestamp = integer_part.parse::<u64>().ok()?;

    Some(OscSample {
        sample: RawSample { timestamp, marker: 0, data },
        counter: *counter,
    })
}

/// Turns the wrapping message counter into a count of missing samples.
#[derive(Debug, Default)]
pub struct LossTracker {
    previous: Option<i32>,
}

impl LossTracker {
    /// Returns how many samples went missing between the previous message
    /// and this one.
    pub fn observe(&mut self, counter: i32) -> u64 {
        let Some(previous) = self.previous.replace(counter) else {
            return 0;
        };
        // wrapping_sub: the counter comes off unauthenticated broadcast
        // traffic, so it can be any i32. 32 divides 2^32, so wrapping the
        // subtraction leaves the mod-32 residue exact.
        let step = counter.wrapping_sub(previous).rem_euclid(COUNTER_MODULUS);
        // A step of 0 is a duplicate, not 32 consecutive drops: losing
        // exactly one full cycle is far less likely than the device or the
        // network repeating one.
        if step <= 1 {
            0
        } else {
            (step - 1) as u64
        }
    }
}

/// Per-session state for the listener: one filter per channel, plus the
/// loss tracker.
struct ListenerState {
    device_id: String,
    config: FilterConfig,
    filters: Vec<ChannelFilter>,
    loss: LossTracker,
}

impl ListenerState {
    fn new(device_id: String, config: FilterConfig) -> Self {
        Self { device_id, config, filters: Vec::new(), loss: LossTracker::default() }
    }

    /// Decodes one datagram and applies it. Anything unrecognised is
    /// ignored in silence — a broadcast port carries other people's traffic,
    /// and treating that as an error would make `dropped_frames` meaningless.
    fn handle(&mut self, datagram: &[u8], live: &Mutex<Live>, recorder: &Mutex<Option<Recorder>>) {
        let Some(decoded) = decode_raw(datagram, &self.device_id) else {
            return;
        };

        // The recorder takes the signal verbatim, before filtering, so
        // `raw.csv` stays the ground truth `meta.json` advertises it to be.
        // Filtering is a display concern and a display concern only -- and
        // for the same reason, samples reconstructed by `fill_gap` below
        // must never reach it: a capture claiming to be verbatim cannot
        // contain values the device never sent.
        //
        // `write_raw` can fail on routine device behaviour, not just disk
        // trouble: a mid-session `Live::configure` channel-count change
        // leaves it rejecting every sample against the old header. Left
        // unhandled that failure is silent forever -- `write_raw` returns
        // before touching the writer, so `Drop`'s flush reports nothing --
        // while `Live::recording` keeps claiming a live path. Latch the
        // recorder off on the first failure, and only take `live`'s lock
        // once the recorder's own lock has been released -- never hold both
        // at once.
        let recorder_failed = {
            let mut guard = crate::sync::lock(recorder);
            match guard.as_mut() {
                Some(r) => match r.write_raw(&decoded.sample) {
                    Ok(()) => false,
                    Err(e) => {
                        eprintln!("warning: recorder stopped, failed to write raw sample: {e}");
                        *guard = None;
                        true
                    }
                },
                None => false,
            }
        }; // recorder's lock is dropped here, before `live` is ever touched.
        if recorder_failed {
            let mut l = crate::sync::lock(live);
            l.recording = None;
            l.touch();
        }

        if self.filters.len() != decoded.sample.data.len() {
            self.filters =
                (0..decoded.sample.data.len()).map(|_| ChannelFilter::new(&self.config)).collect();
            // A channel-count change is a session discontinuity -- most
            // plausibly a device restart -- and the wrapping counter starts
            // over too. Without this reset, the first sample after one
            // reads as up to `COUNTER_MODULUS - 2` samples lost against the
            // previous session's counter: a spurious `dropped_frames` bump,
            // plus that many bogus `fill_gap()` calls into these brand-new,
            // zero-state filters.
            self.loss = LossTracker::default();
        }

        let missing = self.loss.observe(decoded.counter);

        // Every sample UDP dropped must still be fed to the filters, or the
        // gap becomes a phase discontinuity that rings the notch's poles --
        // measured, a stream with gaps simply closed up leaves *more* mains
        // hum than applying no filter at all, because at one drop per ~50
        // samples the ringing never decays between drops. `fill_gap`
        // reconstructs the missing sample from the two before it, which is
        // near-exact here precisely because what is missing is hum.
        //
        // Bounded by construction: `LossTracker` reports at most
        // `COUNTER_MODULUS - 2` missing samples, so this cannot spin.
        for _ in 0..missing {
            for filter in self.filters.iter_mut() {
                filter.fill_gap();
            }
        }

        let filtered: Vec<f64> = decoded
            .sample
            .data
            .iter()
            .zip(self.filters.iter_mut())
            .map(|(v, f)| f.apply(*v))
            .collect();

        let mut l = crate::sync::lock(live);
        l.dropped_frames += missing;
        l.push_raw(&RawSample { data: filtered, ..decoded.sample });
    }
}

/// Receives raw EEG until the task is cancelled.
///
/// Binds the broadcast port the device sends to. Returns an error only if
/// the socket cannot be bound — a receive error is logged and the loop
/// continues, since one bad datagram must not end a session.
pub async fn listen(
    live: Arc<Mutex<Live>>,
    device_id: String,
    config: FilterConfig,
    recorder: Arc<Mutex<Option<Recorder>>>,
) -> anyhow::Result<()> {
    let socket = tokio::net::UdpSocket::bind(("0.0.0.0", OSC_PORT)).await?;
    let mut state = ListenerState::new(device_id, config);
    // rosc decodes OSC bundles recursively with no depth bound, so a large
    // nested-bundle datagram recurses in proportion to its size. The real
    // `/raw` message is 132 bytes; 2048 leaves wide margin while keeping
    // that recursion bounded. Anything bigger just fails to decode, which is
    // fine since only `/raw` is ever accepted.
    let mut buffer = vec![0u8; 2048];
    loop {
        match socket.recv_from(&mut buffer).await {
            Ok((len, _)) => state.handle(&buffer[..len], &live, &recorder),
            Err(e) => {
                eprintln!("warning: OSC receive failed: {e}");
                // Tokio only clears readiness on `WouldBlock`; a persistent
                // hard error (e.g. ENETDOWN) leaves the socket looking
                // permanently ready, so `recv_from` would otherwise return
                // this same error immediately forever -- pegging a worker
                // thread and flooding stderr. A short sleep is enough to
                // break that, and only runs on the error path.
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{encoder, OscMessage, OscPacket, OscType};

    const DEVICE: &str = "83ce6707229234b38aa695de3bf6d70e";

    fn raw_args(values: [f32; 8], timestamp: &str, counter: i32) -> Vec<OscType> {
        let mut args: Vec<OscType> = values.iter().map(|v| OscType::Float(*v)).collect();
        args.push(OscType::String(timestamp.into()));
        args.push(OscType::Int(counter));
        args.push(OscType::String(String::new()));
        args
    }

    /// Builds a packet in the exact shape observed on the wire: eight
    /// float32 channel values, a millisecond timestamp as a *string*, an
    /// int32 counter, and a marker string.
    fn raw_packet(device: &str, values: [f32; 8], timestamp: &str, counter: i32) -> Vec<u8> {
        encoder::encode(&OscPacket::Message(OscMessage {
            addr: format!("/neurosity/notion/{device}/raw"),
            args: raw_args(values, timestamp, counter),
        }))
        .unwrap()
    }

    #[test]
    fn decodes_a_sample_from_our_device() {
        let bytes = raw_packet(DEVICE, [1.0, -2.0, 3.0, -4.0, 5.0, -6.0, 7.0, -8.0],
                               "1787270434786.832", 7);
        let decoded = decode_raw(&bytes, DEVICE).expect("should decode");
        assert_eq!(decoded.counter, 7);
        assert_eq!(decoded.sample.timestamp, 1_787_270_434_786);
        assert_eq!(decoded.sample.data.len(), 8);
        assert_eq!(decoded.sample.data[0], 1.0);
        assert_eq!(decoded.sample.data[7], -8.0);
    }

    #[test]
    fn ignores_another_crown_broadcasting_on_the_same_network() {
        // OSC is unauthenticated broadcast. A second headset on the LAN
        // must not feed our display.
        let bytes = raw_packet("some-other-device", [0.0; 8], "1787270434786.832", 1);
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn ignores_the_info_beacon_and_other_addresses() {
        let bytes = encoder::encode(&OscPacket::Message(OscMessage {
            addr: format!("/neurosity/notion/{DEVICE}/info"),
            args: vec![OscType::Int(8)],
        }))
        .unwrap();
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn rejects_a_message_with_the_wrong_argument_shape() {
        let bytes = encoder::encode(&OscPacket::Message(OscMessage {
            addr: format!("/neurosity/notion/{DEVICE}/raw"),
            args: vec![OscType::Float(1.0), OscType::Float(2.0)],
        }))
        .unwrap();
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn rejects_a_malformed_timestamp_rather_than_guessing() {
        let bytes = raw_packet(DEVICE, [0.0; 8], "not-a-number", 1);
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn rejects_bytes_that_are_not_osc_at_all() {
        assert!(decode_raw(b"random udp noise", DEVICE).is_none());
    }

    #[test]
    fn rejects_a_scientific_notation_timestamp_instead_of_saturating() {
        // A float path parses this to 1e30 and saturates the cast to
        // u64::MAX, which would poison any ordering built on the timestamp.
        let bytes = raw_packet(DEVICE, [0.0; 8], "1e30", 1);
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn rejects_a_negative_zero_timestamp() {
        // A float path reads this as 0.0, passing an `>= 0.0` guard the
        // integer parse never needs in the first place.
        let bytes = raw_packet(DEVICE, [0.0; 8], "-0.0", 1);
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn ignores_an_address_that_merely_embeds_our_device_id() {
        // A `contains(device_id)` check would accept this. Exact-path
        // matching is the only thing stopping an attacker-chosen prefix
        // that happens to carry our device id.
        let bytes = encoder::encode(&OscPacket::Message(OscMessage {
            addr: format!("/attacker/{DEVICE}/raw"),
            args: raw_args([0.0; 8], "1787270434786.832", 1),
        }))
        .unwrap();
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn rejects_a_timestamp_with_a_garbled_fractional_part() {
        // The fractional part is discarded, but garbage there is still a
        // sign the sender isn't speaking the observed wire format.
        let bytes = raw_packet(DEVICE, [0.0; 8], "1787270434786.not-a-number", 1);
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn rejects_channel_values_that_are_not_float32() {
        // Wire tags are always `,ffffffff...`; accepting Double widens the
        // shape beyond anything hardware ever sends.
        let mut args = vec![OscType::Double(1.0)];
        args.extend((1..8).map(|_| OscType::Float(0.0)));
        args.push(OscType::String("1787270434786.832".into()));
        args.push(OscType::Int(1));
        args.push(OscType::String(String::new()));
        let bytes = encoder::encode(&OscPacket::Message(OscMessage {
            addr: format!("/neurosity/notion/{DEVICE}/raw"),
            args,
        }))
        .unwrap();
        assert!(decode_raw(&bytes, DEVICE).is_none());
    }

    #[test]
    fn consecutive_counters_report_no_loss() {
        let mut tracker = LossTracker::default();
        assert_eq!(tracker.observe(0), 0);
        assert_eq!(tracker.observe(1), 0);
        assert_eq!(tracker.observe(2), 0);
    }

    #[test]
    fn the_counter_wrapping_at_thirty_two_is_not_a_loss() {
        let mut tracker = LossTracker::default();
        tracker.observe(30);
        assert_eq!(tracker.observe(31), 0);
        assert_eq!(tracker.observe(0), 0, "wrap must not read as 31 lost samples");
        assert_eq!(tracker.observe(1), 0);
    }

    #[test]
    fn a_gap_reports_the_number_of_missing_samples() {
        let mut tracker = LossTracker::default();
        tracker.observe(5);
        assert_eq!(tracker.observe(8), 2, "6 and 7 went missing");

        // Pins the boundary against a `step <= 2` bug that would silently
        // under-report every single-sample drop as zero.
        let mut single_drop = LossTracker::default();
        single_drop.observe(0);
        assert_eq!(single_drop.observe(2), 1);

        // Pins the modulus at 32: a smaller modulus would still pass the
        // wrap-boundary tests but misjudge a gap that runs right up to them.
        let mut near_wrap = LossTracker::default();
        near_wrap.observe(0);
        assert_eq!(near_wrap.observe(31), 30);
    }

    #[test]
    fn a_hostile_counter_does_not_panic() {
        // Unauthenticated broadcast traffic can carry any i32; the widest
        // possible step must not overflow the subtraction.
        let mut tracker = LossTracker::default();
        tracker.observe(i32::MAX);
        tracker.observe(i32::MIN);
    }

    #[test]
    fn a_gap_across_the_wrap_boundary_is_counted_correctly() {
        let mut tracker = LossTracker::default();
        tracker.observe(30);
        assert_eq!(tracker.observe(1), 2, "31 and 0 went missing");
    }

    #[test]
    fn the_first_sample_of_a_session_reports_no_loss() {
        assert_eq!(LossTracker::default().observe(17), 0);
    }

    #[test]
    fn a_repeated_counter_reports_no_loss_rather_than_a_full_wrap() {
        // A duplicate is far likelier than exactly 32 consecutive drops, and
        // reporting 31 losses for one would badly skew the figure.
        let mut tracker = LossTracker::default();
        tracker.observe(4);
        assert_eq!(tracker.observe(4), 0);
    }

    #[test]
    fn handling_a_datagram_filters_records_and_counts_loss() {
        use crate::streams::DeviceInfo;

        let live = Arc::new(Mutex::new(Live::new()));
        crate::sync::lock(&live).configure(DeviceInfo {
            device_id: DEVICE.into(),
            device_nickname: "Crown-83C".into(),
            channel_names: (0..8).map(|i| format!("ch{i}")).collect(),
            channels: 8,
            sampling_rate: 256.0,
        });

        let mut state = ListenerState::new(DEVICE.into(), FilterConfig::default());
        let recorder = Arc::new(Mutex::new(None));

        let first = raw_packet(DEVICE, [100.0; 8], "1787270434786.832", 0);
        state.handle(&first, &live, &recorder);
        assert_eq!(crate::sync::lock(&live).snapshot(10).raw_samples, 1);

        // Counter jumps 0 -> 3: samples 1 and 2 were dropped in flight.
        let second = raw_packet(DEVICE, [100.0; 8], "1787270434790.569", 3);
        state.handle(&second, &live, &recorder);
        let snap = crate::sync::lock(&live).snapshot(10);
        assert_eq!(snap.raw_samples, 2);
        assert_eq!(snap.dropped_frames, 2, "the two missing samples must be counted");
    }

    #[test]
    fn a_foreign_datagram_changes_nothing() {
        let live = Arc::new(Mutex::new(Live::new()));
        let mut state = ListenerState::new(DEVICE.into(), FilterConfig::default());
        let recorder = Arc::new(Mutex::new(None));

        state.handle(b"not osc at all", &live, &recorder);
        let snap = crate::sync::lock(&live).snapshot(10);
        assert_eq!(snap.raw_samples, 0);
        assert_eq!(snap.dropped_frames, 0, "noise on the port is not a dropped sample");
    }

    fn configured_live() -> Arc<Mutex<Live>> {
        use crate::streams::DeviceInfo;
        let live = Arc::new(Mutex::new(Live::new()));
        crate::sync::lock(&live).configure(DeviceInfo {
            device_id: DEVICE.into(),
            device_nickname: "Crown-83C".into(),
            channel_names: (0..8).map(|i| format!("ch{i}")).collect(),
            channels: 8,
            sampling_rate: FilterConfig::default().sample_rate_hz,
        });
        live
    }

    /// Feeds `n` samples of a `hz` sine through `handle`, dropping every
    /// 50th datagram in flight if `lossy` (the following datagram's counter
    /// jumps by 2, exactly like a real UDP drop), and returns the peak
    /// absolute filtered value pushed into `Live` over the final third —
    /// by which point the filter has settled.
    fn handled_peak(hz: f64, n: usize, lossy: bool) -> f32 {
        let config = FilterConfig::default();
        let rate = config.sample_rate_hz;
        let live = configured_live();
        let mut state = ListenerState::new(DEVICE.into(), config);
        let recorder = Arc::new(Mutex::new(None));

        for i in 0..n {
            let t = i as f64 / rate;
            let v = (2.0 * std::f64::consts::PI * hz * t).sin() as f32;
            let dropped = lossy && i > 0 && i % 50 == 0;
            if !dropped {
                // The counter increments every iteration regardless of
                // `dropped` -- it tracks the device's own per-sample clock,
                // which keeps ticking whether or not the datagram arrives.
                let bytes =
                    raw_packet(DEVICE, [v; 8], "1787270434786.832", i as i32 % COUNTER_MODULUS);
                state.handle(&bytes, &live, &recorder);
            }
        }

        let waveform = crate::sync::lock(&live).snapshot(n).waveform[0].clone();
        waveform[waveform.len() * 2 / 3..]
            .iter()
            .fold(0.0f32, |peak, &(lo, hi)| peak.max(lo.abs()).max(hi.abs()))
    }

    #[test]
    fn gap_fill_keeps_dropped_mains_samples_from_ringing_the_notch() {
        let config = FilterConfig::default();
        let n = (config.sample_rate_hz * 4.0) as usize;

        let clean_peak = handled_peak(config.mains_hz, n, false);
        assert!(clean_peak < 0.1, "undropped mains peak {clean_peak} should be attenuated");

        let lossy_peak = handled_peak(config.mains_hz, n, true);
        assert!(
            lossy_peak < clean_peak + 0.05,
            "gap-filled peak {lossy_peak} should stay close to the no-loss peak {clean_peak}, \
             not ring"
        );
    }

    #[test]
    fn handle_actually_filters_rather_than_passing_the_raw_value_through() {
        let live = configured_live();
        let mut state = ListenerState::new(DEVICE.into(), FilterConfig::default());
        let recorder = Arc::new(Mutex::new(None));

        for i in 0..2048 {
            let bytes = raw_packet(DEVICE, [1000.0; 8], "1787270434786.832", i % COUNTER_MODULUS);
            state.handle(&bytes, &live, &recorder);
        }

        let (_, last) = *crate::sync::lock(&live).snapshot(2048).waveform[0].last().unwrap();
        assert!(last.abs() < 1.0, "a settled DC input should filter near zero, got {last}");
    }

    #[test]
    fn the_recorder_receives_the_verbatim_signal_not_the_filtered_one() {
        use crate::streams::DeviceInfo;

        let root = std::env::temp_dir().join(format!(
            "crown-test-{}-osc_recorder_receives_verbatim_signal",
            std::process::id()
        ));
        let info = DeviceInfo {
            device_id: DEVICE.into(),
            device_nickname: "Crown-83C".into(),
            channel_names: (0..8).map(|i| format!("ch{i}")).collect(),
            channels: 8,
            sampling_rate: FilterConfig::default().sample_rate_hz,
        };
        let rec = Recorder::start(&root, &info, "session-osc-verbatim").unwrap();
        let recorder = Arc::new(Mutex::new(Some(rec)));

        let live = Arc::new(Mutex::new(Live::new()));
        crate::sync::lock(&live).configure(info);
        let mut state = ListenerState::new(DEVICE.into(), FilterConfig::default());

        // A large, obviously-unfiltered value: the notch would crush this to
        // near zero by the time it reaches `Live` (see the test above), so
        // finding it intact in raw.csv is what proves the recorder sees the
        // signal before filtering, not after.
        let first = raw_packet(DEVICE, [1000.0; 8], "1787270434786.832", 0);
        state.handle(&first, &live, &recorder);
        let second = raw_packet(DEVICE, [1000.0; 8], "1787270434790.832", 1);
        state.handle(&second, &live, &recorder);

        drop(crate::sync::lock(&recorder).take()); // flush by dropping the Recorder

        let raw =
            std::fs::read_to_string(root.join("session-osc-verbatim").join("raw.csv")).unwrap();
        let rows: Vec<&str> = raw.lines().skip(1).collect();
        assert_eq!(rows.len(), 2, "both samples should have been recorded");
        let timestamps: Vec<&str> = rows.iter().map(|r| r.split(',').next().unwrap()).collect();
        assert_eq!(
            timestamps,
            ["1787270434786", "1787270434790"],
            "rows must be recorded in the order the datagrams arrived"
        );
        for row in rows {
            for v in row.split(',').skip(1) {
                assert_eq!(
                    v.parse::<f64>().unwrap(),
                    1000.0,
                    "raw.csv must hold the verbatim device value, not the filtered one"
                );
            }
        }

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_recorder_write_failure_latches_recording_off() {
        use crate::streams::DeviceInfo;

        let root = std::env::temp_dir().join(format!(
            "crown-test-{}-osc_recorder_write_failure_latches_off",
            std::process::id()
        ));
        // Started with 4 channels; every real /raw datagram carries 8, so
        // every write_raw call here is a guaranteed width mismatch --
        // standing in for the mid-session Live::configure channel-count
        // change this latch-off guards against.
        let recorder_info = DeviceInfo {
            device_id: DEVICE.into(),
            device_nickname: "Crown-83C".into(),
            channel_names: (0..4).map(|i| format!("ch{i}")).collect(),
            channels: 4,
            sampling_rate: FilterConfig::default().sample_rate_hz,
        };
        let rec = Recorder::start(&root, &recorder_info, "session-osc-write-fail").unwrap();
        let recorder = Arc::new(Mutex::new(Some(rec)));

        let live = configured_live();
        crate::sync::lock(&live).recording = Some(root.join("session-osc-write-fail"));

        let mut state = ListenerState::new(DEVICE.into(), FilterConfig::default());
        let bytes = raw_packet(DEVICE, [1.0; 8], "1787270434786.832", 0);
        state.handle(&bytes, &live, &recorder);

        assert!(
            crate::sync::lock(&recorder).is_none(),
            "the recorder must latch off after a write failure"
        );
        assert!(
            crate::sync::lock(&live).snapshot(10).recording.is_none(),
            "the recording indicator must clear to match"
        );

        std::fs::remove_dir_all(&root).unwrap();
    }
}
