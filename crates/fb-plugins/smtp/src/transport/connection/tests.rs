use super::*;
use futures_util::io::{AsyncRead, Cursor};
use std::{
    io,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
}

fn line(length: usize, continuation: bool) -> Vec<u8> {
    let mut bytes = vec![b'x'; length];
    bytes[..4].copy_from_slice(if continuation { b"250-" } else { b"250 " });
    bytes[length - 2..].copy_from_slice(b"\r\n");
    bytes
}

#[test]
fn exact_line_and_response_ceilings_accept_and_one_extra_byte_rejects() {
    runtime().block_on(async {
        for capacity in [1, 7, READ_BUFFER_BYTES] {
            let exact = line(MAX_LINE_BYTES, false);
            let mut reader = BufReader::with_capacity(capacity, Cursor::new(exact));
            assert!(read_response(&mut reader).await.unwrap().has_code(250));
            let too_long = line(MAX_LINE_BYTES + 1, false);
            let mut reader = BufReader::with_capacity(capacity, Cursor::new(too_long));
            assert!(matches!(
                read_response(&mut reader).await,
                Err(PluginError::InvalidResponse)
            ));

            let mut exact =
                line(MAX_LINE_BYTES, true).repeat(MAX_RESPONSE_BYTES / MAX_LINE_BYTES - 1);
            exact.extend(line(MAX_LINE_BYTES, false));
            assert_eq!(exact.len(), MAX_RESPONSE_BYTES);
            let mut reader = BufReader::with_capacity(capacity, Cursor::new(exact));
            assert!(read_response(&mut reader).await.unwrap().has_code(250));

            // Every line is legal; only the total response is over the ceiling.
            let mut too_long =
                line(MAX_LINE_BYTES, true).repeat(MAX_RESPONSE_BYTES / MAX_LINE_BYTES - 1);
            too_long.extend(line(500, true));
            too_long.extend(line(501, false));
            let mut reader = BufReader::with_capacity(capacity, Cursor::new(too_long));
            assert!(matches!(
                read_response(&mut reader).await,
                Err(PluginError::InvalidResponse)
            ));
        }
    });
}

/// Produces unending attacker input without allocating an attacker-sized fixture.
/// The counter proves rejection after a fixed amount of upstream reading, not EOF
/// or the total timeout. In production this reader is the bounded TLS/TCP stream.
struct Endless {
    pattern: Vec<u8>,
    read: Arc<AtomicUsize>,
}
impl AsyncRead for Endless {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let offset = self.read.load(Ordering::Relaxed);
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = self.pattern[(offset + index) % self.pattern.len()];
        }
        self.read.fetch_add(out.len(), Ordering::Relaxed);
        Poll::Ready(Ok(out.len()))
    }
}

#[test]
fn infinite_unterminated_line_and_multiline_response_have_bounded_reads() {
    runtime().block_on(async {
        for capacity in [1, 7, READ_BUFFER_BYTES] {
            for (pattern, ceiling) in [
                (vec![b'x'], MAX_LINE_BYTES),
                (line(MAX_LINE_BYTES, true), MAX_RESPONSE_BYTES),
            ] {
                let count = Arc::new(AtomicUsize::new(0));
                let input = Endless {
                    pattern,
                    read: count.clone(),
                };
                let mut reader = BufReader::with_capacity(capacity, input);
                assert!(matches!(
                    read_response(&mut reader).await,
                    Err(PluginError::InvalidResponse)
                ));
                let read = count.load(Ordering::Relaxed);
                assert!(read > ceiling);
                assert!(
                    read <= ceiling + capacity,
                    "read {read} past ceiling {ceiling}"
                );
            }
        }
    });
}

#[test]
fn reply_framing_rejects_bad_crlf_mixed_codes_and_incomplete_responses() {
    runtime().block_on(async {
        for wire in [
            b"250 bad\n".as_slice(),
            b"250-first\r\n220 last\r\n",
            b"250",
            b"250-first\r\n",
            b"250?bad\r\n",
            b"xyz bad\r\n",
            b"250 bad\xff\r\n",
            b"999 bad\r\n",
        ] {
            let mut reader = BufReader::new(Cursor::new(wire));
            assert!(matches!(
                read_response(&mut reader).await,
                Err(PluginError::InvalidResponse)
            ));
        }
        for wire in [
            b"250 \r\n".as_slice(),
            b"250-first\r\n250 \r\n",
            b"250-first\r\n250 last\r\n",
        ] {
            let mut reader = BufReader::new(Cursor::new(wire));
            assert!(read_response(&mut reader).await.unwrap().has_code(250));
        }
    });
}

#[test]
fn response_budget_resets_between_replies_not_between_continuation_lines() {
    runtime().block_on(async {
        let mut wire = line(MAX_LINE_BYTES, false);
        wire.extend(line(MAX_LINE_BYTES, false));
        let mut reader = BufReader::new(Cursor::new(wire));
        assert!(read_response(&mut reader).await.unwrap().has_code(250));
        assert!(read_response(&mut reader).await.unwrap().has_code(250));
    });
}
