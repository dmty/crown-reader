const TIMESTAMP_SIZE: usize = 8;
const MARKER_SIZE: usize = 2;
const CHANNEL_SIZE: usize = 8;

/// Wire size of one sample. The stream carries no delimiter and no checksum,
/// so alignment depends entirely on this being right.
pub fn encoded_sample_size(channels: usize) -> usize {
    TIMESTAMP_SIZE + MARKER_SIZE + channels * CHANNEL_SIZE
}

#[derive(Debug, Clone, PartialEq)]
pub struct RawSample {
    pub timestamp: u64,
    pub marker: u16,
    pub data: Vec<f64>,
}

#[derive(Default)]
pub struct RawDecoder {
    buf: Vec<u8>,
    channels: Option<usize>,
}

impl RawDecoder {
    /// Decodes big-endian samples from the byte stream.
    ///
    /// `channels` must not change for a decoder's lifetime. If the channel count changes,
    /// clear the decoder with a fresh `RawDecoder::default()` to reconnect.
    pub fn push(&mut self, bytes: &[u8], channels: usize) -> Vec<RawSample> {
        if let Some(prev_channels) = self.channels {
            if prev_channels != channels {
                self.buf.clear();
            }
        }
        self.channels = Some(channels);
        let size = encoded_sample_size(channels);
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while self.buf.len() >= size {
            let timestamp = u64::from_be_bytes(self.buf[..TIMESTAMP_SIZE].try_into().unwrap());
            let marker = u16::from_be_bytes(
                self.buf[TIMESTAMP_SIZE..TIMESTAMP_SIZE + MARKER_SIZE].try_into().unwrap(),
            );
            let base = TIMESTAMP_SIZE + MARKER_SIZE;
            let data = (0..channels)
                .map(|i| {
                    let o = base + i * CHANNEL_SIZE;
                    f64::from_be_bytes(self.buf[o..o + CHANNEL_SIZE].try_into().unwrap())
                })
                .collect();
            out.push(RawSample { timestamp, marker, data });
            self.buf.drain(..size);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(timestamp: u64, marker: u16, data: &[f64]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&timestamp.to_be_bytes());
        v.extend_from_slice(&marker.to_be_bytes());
        for d in data {
            v.extend_from_slice(&d.to_be_bytes());
        }
        v
    }

    #[test]
    fn sample_size_is_ten_plus_eight_per_channel() {
        assert_eq!(encoded_sample_size(8), 74);
        assert_eq!(encoded_sample_size(2), 26);
    }

    #[test]
    fn decodes_one_big_endian_sample() {
        let mut d = RawDecoder::default();
        let bytes = encode(1_700_000_000_000, 0, &[1.5, -2.5]);
        let out = d.push(&bytes, 2);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].timestamp, 1_700_000_000_000);
        assert_eq!(out[0].data, vec![1.5, -2.5]);
    }

    #[test]
    fn holds_a_partial_sample_until_the_rest_arrives() {
        let mut d = RawDecoder::default();
        let bytes = encode(42, 7, &[3.0, 4.0]);
        assert!(d.push(&bytes[..10], 2).is_empty());
        let out = d.push(&bytes[10..], 2);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].marker, 7);
    }

    #[test]
    fn decodes_several_samples_from_one_notification() {
        let mut d = RawDecoder::default();
        let mut bytes = encode(1, 0, &[1.0]);
        bytes.extend(encode(2, 0, &[2.0]));
        bytes.extend(encode(3, 0, &[3.0]));
        let out = d.push(&bytes, 1);
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].timestamp, 3);
    }

    #[test]
    fn changing_channel_count_discards_stranded_bytes() {
        let mut d = RawDecoder::default();
        let bytes_2ch = encode(42, 0, &[1.0, 2.0]);
        // Push only the first 20 bytes of a 2-channel sample (26 bytes total)
        assert!(d.push(&bytes_2ch[..20], 2).is_empty());
        // Now change to 1-channel and push a complete 1-channel sample
        let bytes_1ch = encode(99, 0, &[3.0]);
        let out = d.push(&bytes_1ch, 1);
        // Should get exactly one sample with the 1-channel data, proving stranded bytes were discarded
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].timestamp, 99);
        assert_eq!(out[0].data, vec![3.0]);
    }
}
