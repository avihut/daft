//! Length-prefixed framing for the coordinator IPC socket.
//!
//! Each frame is a 4-byte big-endian `u32` length header followed by exactly
//! that many bytes of payload. Connections carry one logical request per
//! frame. Streaming responses are encoded as a sequence of frames terminated
//! by a `StreamEnd` envelope at the JSON layer (see [`super::envelope`]).
//!
//! The previous protocol was newline-delimited JSON; switching to
//! length-prefixed lets payloads contain embedded newlines (live-streamed log
//! lines, error messages with stack traces) without escaping.

use anyhow::{Result, anyhow, bail};
use std::io::{Read, Write};

/// Refuse frames larger than this. Local IPC carries small control messages
/// or batched log records; 8 MiB is generous and bounded.
pub const MAX_FRAME_BYTES: u32 = 8 * 1024 * 1024;

pub fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> Result<()> {
    let len = u32::try_from(payload.len())
        .map_err(|_| anyhow!("payload too large: {} bytes", payload.len()))?;
    if len > MAX_FRAME_BYTES {
        bail!(
            "frame size {} exceeds MAX_FRAME_BYTES ({})",
            len,
            MAX_FRAME_BYTES
        );
    }
    w.write_all(&len.to_be_bytes())?;
    w.write_all(payload)?;
    Ok(())
}

pub fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    r.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header);
    if len > MAX_FRAME_BYTES {
        bail!(
            "incoming frame size {} exceeds MAX_FRAME_BYTES ({})",
            len,
            MAX_FRAME_BYTES
        );
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn empty_frame_roundtrips() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"").unwrap();
        assert_eq!(buf, vec![0, 0, 0, 0]);
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap(), b"");
    }

    #[test]
    fn small_frame_roundtrips_with_be_length_prefix() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"hello").unwrap();
        // u32 big-endian length 5, then "hello".
        assert_eq!(&buf[..4], &[0, 0, 0, 5]);
        assert_eq!(&buf[4..], b"hello");
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap(), b"hello");
    }

    #[test]
    fn multiple_frames_in_sequence_decode_independently() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"first").unwrap();
        write_frame(&mut buf, b"second-longer").unwrap();
        write_frame(&mut buf, &[0u8, 1, 2, 3, 0xff]).unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap(), b"first");
        assert_eq!(read_frame(&mut cur).unwrap(), b"second-longer");
        assert_eq!(read_frame(&mut cur).unwrap(), &[0u8, 1, 2, 3, 0xff]);
    }

    #[test]
    fn frame_with_embedded_newlines_roundtrips() {
        let payload = b"line1\nline2\r\nline3";
        let mut buf = Vec::new();
        write_frame(&mut buf, payload).unwrap();
        let mut cur = Cursor::new(buf);
        assert_eq!(read_frame(&mut cur).unwrap(), payload);
    }

    #[test]
    fn read_rejects_oversize_header() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_FRAME_BYTES + 1).to_be_bytes());
        // No body needed — the size check fires before any body read.
        let mut cur = Cursor::new(buf);
        let err = read_frame(&mut cur).unwrap_err();
        assert!(err.to_string().contains("exceeds MAX_FRAME_BYTES"));
    }

    #[test]
    fn read_truncated_body_returns_unexpected_eof() {
        // Header says 10 bytes; we only provide 4.
        let mut buf = vec![0u8, 0, 0, 10];
        buf.extend_from_slice(b"abcd");
        let mut cur = Cursor::new(buf);
        let err = read_frame(&mut cur).unwrap_err();
        let downcast = err.downcast_ref::<std::io::Error>();
        assert!(
            matches!(downcast, Some(e) if e.kind() == std::io::ErrorKind::UnexpectedEof),
            "expected UnexpectedEof, got {err:?}"
        );
    }

    #[test]
    fn write_rejects_oversize_payload() {
        // Build a fake payload pretending to be huge via a 0-len slice
        // pointer cast — we just call write_frame with a near-cap size to
        // trigger the gate without allocating 8 MiB.
        let payload = vec![0u8; (MAX_FRAME_BYTES + 1) as usize];
        let mut buf = Vec::new();
        let err = write_frame(&mut buf, &payload).unwrap_err();
        assert!(err.to_string().contains("exceeds MAX_FRAME_BYTES"));
    }
}
