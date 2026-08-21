//! Raw EEG over OSC, the transport that replaced Bluetooth for this stream.
//!
//! Measured against hardware: ~250 samples/s at ~2% packet loss with stable
//! latency, against Bluetooth's 24% of realtime and unbounded backlog.
//!
//! The device broadcasts to the subnet, so anything on the LAN can receive
//! it — including packets from someone else's headset. Every message is
//! matched against our own device id before it is trusted.

use rosc::{OscPacket, OscType};

use crate::raw::RawSample;

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
        let step = (counter - previous).rem_euclid(COUNTER_MODULUS);
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
}
