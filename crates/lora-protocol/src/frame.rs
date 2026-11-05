//! Length-prefixed v1 wire framing for inherited anonymous pipes.
//!
//! Frame layout (`docs/INFERENCE_ENGINE.md`):
//! - 4-byte unsigned big-endian byte length (`u32`)
//! - `length` bytes of valid UTF-8 JSON
//! - Reject length `0` or `> MAX_FRAME_BYTES` (1 MiB) before allocating payload memory.
//! - EOF before the first byte of the prefix returns `Ok(None)` so callers can distinguish
//!   clean stream closure from a mid-frame truncation (`FrameError::TruncatedFrame`).

use std::io::{self, Read, Write};
use thiserror::Error;

/// Maximum permitted frame payload length (1 MiB).
pub const MAX_FRAME_BYTES: u32 = 1_048_576;

/// Framing errors independent of higher-level JSON schema validation.
#[derive(Debug, Error)]
pub enum FrameError {
    #[error("invalid frame length {0} bytes (must be 1..={MAX_FRAME_BYTES})")]
    InvalidLength(u32),

    #[error("truncated frame on wire (unexpected EOF inside length prefix or payload)")]
    TruncatedFrame,

    #[error("frame payload is not valid UTF-8: {0}")]
    InvalidUtf8(#[from] std::string::FromUtf8Error),

    #[error("I/O error on framed stream: {0}")]
    Io(#[from] io::Error),
}

impl PartialEq for FrameError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::InvalidLength(a), Self::InvalidLength(b)) => a == b,
            (Self::TruncatedFrame, Self::TruncatedFrame) => true,
            (Self::InvalidUtf8(_), Self::InvalidUtf8(_)) => true,
            (Self::Io(a), Self::Io(b)) => a.kind() == b.kind(),
            _ => false,
        }
    }
}

impl Eq for FrameError {}

/// Encode a UTF-8 JSON string into a length-prefixed v1 frame buffer.
pub fn encode_frame_str(json: &str) -> Result<Vec<u8>, FrameError> {
    let bytes = json.as_bytes();
    let len = u32::try_from(bytes.len()).map_err(|_| FrameError::InvalidLength(u32::MAX))?;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(FrameError::InvalidLength(len));
    }
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(out)
}

/// Write a single UTF-8 JSON frame to `writer` and flush.
pub fn write_frame_str<W: Write>(writer: &mut W, json: &str) -> Result<(), FrameError> {
    let encoded = encode_frame_str(json)?;
    writer.write_all(&encoded)?;
    writer.flush()?;
    Ok(())
}

/// Read a single UTF-8 JSON frame from `reader`.
///
/// Returns `Ok(None)` if EOF occurs cleanly before any byte of the 4-byte length prefix is read.
/// Returns `Err(FrameError::TruncatedFrame)` if EOF occurs mid-prefix or mid-payload.
/// Rejects zero or oversized frames (`> 1 MiB`) before allocating the payload buffer.
pub fn read_frame_str<R: Read>(reader: &mut R) -> Result<Option<String>, FrameError> {
    let mut prefix = [0u8; 4];
    let mut read_prefix = 0usize;

    while read_prefix < 4 {
        match reader.read(&mut prefix[read_prefix..]) {
            Ok(0) => {
                if read_prefix == 0 {
                    return Ok(None);
                }
                return Err(FrameError::TruncatedFrame);
            }
            Ok(n) => read_prefix += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FrameError::Io(e)),
        }
    }

    let len = u32::from_be_bytes(prefix);
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(FrameError::InvalidLength(len));
    }

    let mut payload = vec![0u8; len as usize];
    let mut read_payload = 0usize;
    while read_payload < payload.len() {
        match reader.read(&mut payload[read_payload..]) {
            Ok(0) => return Err(FrameError::TruncatedFrame),
            Ok(n) => read_payload += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FrameError::Io(e)),
        }
    }

    let text = String::from_utf8(payload)?;
    Ok(Some(text))
}
