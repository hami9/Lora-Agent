//! Incremental UTF-8 decoding for token pieces.
//!
//! llama.cpp hands back raw byte pieces; a single multi-byte character may be split across
//! several tokens. The accumulator only releases complete text and carries an incomplete tail
//! forward. Definitively invalid bytes become U+FFFD.

#[derive(Debug, Default)]
pub struct Utf8Accumulator {
    carry: Vec<u8>,
}

impl Utf8Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `bytes` and return all text that is now complete.
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.carry.extend_from_slice(bytes);
        let mut out = String::new();
        let mut start = 0usize;
        loop {
            match std::str::from_utf8(&self.carry[start..]) {
                Ok(s) => {
                    out.push_str(s);
                    start = self.carry.len();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(
                        std::str::from_utf8(&self.carry[start..start + valid])
                            .expect("validated prefix"),
                    );
                    start += valid;
                    match e.error_len() {
                        Some(n) => {
                            out.push('\u{FFFD}');
                            start += n;
                        }
                        // Incomplete sequence at the end: keep it for the next piece.
                        None => break,
                    }
                }
            }
        }
        self.carry.drain(..start);
        out
    }

    /// Flush at end of stream: an unfinished trailing sequence becomes U+FFFD.
    pub fn finish(&mut self) -> String {
        if self.carry.is_empty() {
            return String::new();
        }
        let s = String::from_utf8_lossy(&self.carry).into_owned();
        self.carry.clear();
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through() {
        let mut a = Utf8Accumulator::new();
        assert_eq!(a.push(b"hello"), "hello");
        assert_eq!(a.finish(), "");
    }

    #[test]
    fn multibyte_split_across_pieces() {
        let euro = "€".as_bytes(); // E2 82 AC
        let mut a = Utf8Accumulator::new();
        assert_eq!(a.push(&euro[..1]), "");
        assert_eq!(a.push(&euro[1..2]), "");
        assert_eq!(a.push(&euro[2..]), "€");
        let emoji = "a😀b".as_bytes();
        assert_eq!(a.push(&emoji[..3]), "a");
        assert_eq!(a.push(&emoji[3..]), "😀b");
        assert_eq!(a.finish(), "");
    }

    #[test]
    fn invalid_bytes_become_replacement() {
        let mut a = Utf8Accumulator::new();
        assert_eq!(a.push(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
        // A lead byte followed by a non-continuation is invalid, not carried.
        assert_eq!(a.push(&[0xE2, b'x']), "\u{FFFD}x");
    }

    #[test]
    fn truncated_tail_is_flushed_lossy() {
        let mut a = Utf8Accumulator::new();
        assert_eq!(a.push(&"€".as_bytes()[..2]), "");
        assert_eq!(a.finish(), "\u{FFFD}");
        assert_eq!(a.finish(), "");
    }
}
