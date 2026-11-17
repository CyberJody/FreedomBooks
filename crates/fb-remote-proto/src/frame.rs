//! Length-prefixed JSON frames: a 4-byte big-endian length, then that many bytes of UTF-8 JSON.
//! Used on the TLS connection and on the stdio pipes between processes. A frame over the limit is
//! refused before it is read, so a peer cannot make the reader allocate more than the limit.
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

/// Largest request a connected computer may send (a ported form is ~40 KB; fb-app accepts 256 KiB).
pub const MAX_REQUEST: usize = 1 << 20;
/// Largest response (a long report page or CSV export).
pub const MAX_RESPONSE: usize = 8 << 20;
/// Pipe frames carry a request or a response plus a small envelope.
pub const MAX_PIPE: usize = MAX_RESPONSE + 64 * 1024;

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    TooLarge(usize),
    Json(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "connection: {e}"),
            FrameError::TooLarge(n) => write!(f, "frame of {n} bytes is over the limit"),
            FrameError::Json(e) => write!(f, "invalid frame: {e}"),
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

pub fn write_frame<W: Write>(w: &mut W, bytes: &[u8], max: usize) -> Result<(), FrameError> {
    if bytes.len() > max {
        return Err(FrameError::TooLarge(bytes.len()));
    }
    let len = u32::try_from(bytes.len()).map_err(|_| FrameError::TooLarge(bytes.len()))?;
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    w.write_all(&out)?;
    w.flush()?;
    Ok(())
}

/// `Ok(None)` on a clean end of stream before a frame starts.
pub fn read_frame<R: Read>(r: &mut R, max: usize) -> Result<Option<Vec<u8>>, FrameError> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < 4 {
        match r.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Io(io::ErrorKind::UnexpectedEof.into())),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(FrameError::Io(e)),
        }
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > max {
        return Err(FrameError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    Ok(Some(body))
}

pub fn send<W: Write, T: Serialize>(w: &mut W, value: &T, max: usize) -> Result<(), FrameError> {
    let bytes = serde_json::to_vec(value).map_err(|e| FrameError::Json(e.to_string()))?;
    write_frame(w, &bytes, max)
}

/// Read one frame and parse it strictly: unknown fields (per the type) and duplicate keys refused.
pub fn recv<R: Read, T: DeserializeOwned>(r: &mut R, max: usize) -> Result<Option<T>, FrameError> {
    let Some(bytes) = read_frame(r, max)? else { return Ok(None) };
    let text = std::str::from_utf8(&bytes).map_err(|_| FrameError::Json("not UTF-8".into()))?;
    crate::strict::parse(text).map(Some).map_err(FrameError::Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_enforce_limits() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"{\"a\":1}", 64).unwrap();
        let mut r = &buf[..];
        assert_eq!(read_frame(&mut r, 64).unwrap().unwrap(), b"{\"a\":1}");
        assert!(read_frame(&mut r, 64).unwrap().is_none(), "clean end of stream");
        assert!(matches!(write_frame(&mut Vec::new(), &[0; 65], 64), Err(FrameError::TooLarge(65))));
        // A declared length over the limit is refused without reading (or allocating) the body.
        let mut huge = &(u32::MAX).to_be_bytes()[..];
        assert!(matches!(read_frame(&mut huge, 64), Err(FrameError::TooLarge(_))));
        // A truncated header or body is an error, not a frame.
        assert!(read_frame(&mut &[0u8, 0][..], 64).is_err());
        assert!(read_frame(&mut &[0u8, 0, 0, 5, b'x'][..], 64).is_err());
    }
}
