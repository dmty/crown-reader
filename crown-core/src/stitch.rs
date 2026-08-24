/// Reassembles newline-delimited text from BLE notification fragments.
///
/// Buffers bytes rather than strings: a notification boundary can land in the
/// middle of a multi-byte UTF-8 character, and decoding early would corrupt it.
#[derive(Default)]
pub struct Stitcher {
    buf: Vec<u8>,
}

impl Stitcher {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            let text = std::str::from_utf8(&self.buf[..i]).ok().map(str::to_string);
            self.buf.drain(..=i);
            if let Some(text) = text {
                if !text.is_empty() {
                    out.push(text);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_nothing_until_a_newline_arrives() {
        let mut s = Stitcher::default();
        assert!(s.push(b"{\"a\":1}").is_empty());
        assert_eq!(s.push(b"\n"), vec!["{\"a\":1}".to_string()]);
    }

    #[test]
    fn splits_multiple_lines_in_one_push() {
        let mut s = Stitcher::default();
        assert_eq!(
            s.push(b"one\ntwo\n"),
            vec!["one".to_string(), "two".to_string()]
        );
    }

    #[test]
    fn carries_the_remainder_across_pushes() {
        let mut s = Stitcher::default();
        assert_eq!(s.push(b"first\nsec"), vec!["first".to_string()]);
        assert_eq!(s.push(b"ond\n"), vec!["second".to_string()]);
    }

    #[test]
    fn skips_empty_lines() {
        let mut s = Stitcher::default();
        assert_eq!(s.push(b"\n\nx\n"), vec!["x".to_string()]);
    }

    #[test]
    fn survives_a_multibyte_char_split_across_pushes() {
        let mut s = Stitcher::default();
        let bytes = "héllo\n".as_bytes();
        assert!(s.push(&bytes[..2]).is_empty());
        assert_eq!(s.push(&bytes[2..]), vec!["héllo".to_string()]);
    }

    #[test]
    fn invalid_utf8_line_is_skipped_without_stalling() {
        let mut s = Stitcher::default();
        assert_eq!(s.push(b"\xff\xfe\ngood\n"), vec!["good".to_string()]);
    }
}
