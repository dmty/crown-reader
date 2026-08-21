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
            OscType::Double(d) => Some(*d),
            _ => None,
        })
        .collect::<Option<_>>()?;

    let timestamp = match tail.first()? {
        // Milliseconds, as a decimal string: "1787270434786.832".
        OscType::String(s) => s.parse::<f64>().ok()?,
        _ => return None,
    };
    if !timestamp.is_finite() || timestamp < 0.0 {
        return None;
    }
    let counter = match tail.get(1)? {
        OscType::Int(i) => *i,
        _ => return None,
    };

    Some(OscSample {
        sample: RawSample { timestamp: timestamp as u64, marker: 0, data },
        counter,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rosc::{encoder, OscMessage, OscPacket, OscType};

    const DEVICE: &str = "83ce6707229234b38aa695de3bf6d70e";

    /// Builds a packet in the exact shape observed on the wire: eight
    /// float32 channel values, a millisecond timestamp as a *string*, an
    /// int32 counter, and a marker string.
    fn raw_packet(device: &str, values: [f32; 8], timestamp: &str, counter: i32) -> Vec<u8> {
        let mut args: Vec<OscType> = values.iter().map(|v| OscType::Float(*v)).collect();
        args.push(OscType::String(timestamp.into()));
        args.push(OscType::Int(counter));
        args.push(OscType::String(String::new()));
        encoder::encode(&OscPacket::Message(OscMessage {
            addr: format!("/neurosity/notion/{device}/raw"),
            args,
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
}
