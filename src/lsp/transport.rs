//! The standard input and output of the language server.
//!
//! `tower-lsp` ends its read loop at the first message it cannot decode (the
//! process would then exit as if the client had closed the connection), and
//! after an `exit` notification it keeps reading until the stream ends. Both
//! are wrong for a language server, so the server does not hand it the raw
//! streams:
//!
//! - [`SanitizedInput`] cuts the input into `Content-Length` frames itself and
//!   passes on only the frames `tower-lsp` will accept. For any other frame it
//!   queues the JSON-RPC error response the protocol prescribes (`-32700` for
//!   text that is not JSON or a broken header, `-32600` for JSON that is not a
//!   request) and reads on. It notes a `shutdown` request and the `exit`
//!   notification, and ends the stream after `exit`.
//! - [`pump_frames`] is the only writer of standard output. It writes the
//!   frames `tower-lsp` produces and the queued error responses one whole
//!   frame at a time, so they never interleave.

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::mpsc::{Receiver, Sender};
use tower_lsp::jsonrpc::{Error, Id, Request, Response};

/// What the input stream went through.
#[derive(Default)]
pub(super) struct InputWatch {
    /// A `shutdown` request went by. It counts when it is read, not when it
    /// is answered: a client may send `exit` right behind it, and the
    /// service drops the request still in flight.
    pub(super) shutdown_requested: AtomicBool,
    /// The `exit` notification went by; the stream ends after it.
    pub(super) exit_requested: AtomicBool,
    /// The underlying stream reached its end.
    pub(super) reached_eof: AtomicBool,
}

/// Header bytes kept before a header block is given up as garbage.
const MAX_HEADER: usize = 8192;
/// The longest body accepted; a larger `Content-Length` is a broken header.
const MAX_BODY: usize = 256 * 1024 * 1024;
/// The largest buffer kept once it has been emptied.
const KEPT_CAPACITY: usize = 1024 * 1024;

/// One thing found in the input.
#[derive(Debug, PartialEq, Eq)]
enum Event {
    /// A complete frame: its body.
    Frame(Vec<u8>),
    /// A header block that names no usable `Content-Length`.
    BadHeader,
}

/// The words that open the header of a frame.
const MARKER: &[u8] = b"content-length:";

/// Cuts a byte stream into `Content-Length` frames.
#[derive(Default)]
struct FrameReader {
    buf: Vec<u8>,
    /// The body length of the frame whose header has been read.
    body_len: Option<usize>,
    /// A header could not be read: what follows is the body of that frame or
    /// noise, and is dropped up to the next thing that looks like a header.
    skipping: bool,
    /// A broken header has been reported and no header has been read since:
    /// the noise that follows it is not reported again.
    reported: bool,
}

impl FrameReader {
    /// Feed the bytes just read; returns the events they complete.
    fn feed(&mut self, data: &[u8]) -> Vec<Event> {
        self.buf.extend_from_slice(data);
        let mut events = Vec::new();
        loop {
            if self.skipping {
                if !self.skip_to_header() {
                    break;
                }
                self.skipping = false;
            }
            match self.body_len {
                None => match header_end(&self.buf) {
                    Some(end) => {
                        let header: Vec<u8> = self.buf.drain(..end).collect();
                        match content_length(&header) {
                            Some(length) if length <= MAX_BODY => {
                                self.body_len = Some(length);
                                self.reported = false;
                            }
                            _ => {
                                self.report_bad_header(&mut events);
                                self.skipping = true;
                            }
                        }
                    }
                    None if self.buf.len() > MAX_HEADER => {
                        self.report_bad_header(&mut events);
                        // The buffer begins with the header that got too
                        // long; move past it before looking for the next.
                        self.buf.drain(..1);
                        self.skipping = true;
                    }
                    None => break,
                },
                Some(length) => {
                    if self.buf.len() < length {
                        break;
                    }
                    let body: Vec<u8> = self.buf.drain(..length).collect();
                    self.body_len = None;
                    events.push(Event::Frame(body));
                }
            }
        }
        // A large message leaves a large buffer behind; do not keep it.
        if self.buf.is_empty() && self.buf.capacity() > KEPT_CAPACITY {
            self.buf = Vec::new();
        }
        events
    }

    /// One report per run of noise, however much of it there is.
    fn report_bad_header(&mut self, events: &mut Vec<Event>) {
        if !self.reported {
            events.push(Event::BadHeader);
            self.reported = true;
        }
    }

    /// Drop bytes up to the next `Content-Length:`. Returns whether it was
    /// found; if not, only a tail that could be the start of it (cut by the
    /// end of this read) is kept.
    fn skip_to_header(&mut self) -> bool {
        self.body_len = None;
        if let Some(at) = find_ignore_ascii_case(&self.buf, MARKER) {
            self.buf.drain(..at);
            return true;
        }
        let keep = (1..MARKER.len())
            .rev()
            .find(|&k| {
                k <= self.buf.len()
                    && self.buf[self.buf.len() - k..].eq_ignore_ascii_case(&MARKER[..k])
            })
            .unwrap_or(0);
        let cut = self.buf.len() - keep;
        self.buf.drain(..cut);
        false
    }
}

/// Where a header block ends: the number of bytes up to and including the
/// blank line that closes it. Header lines end with `\r\n`, or with a bare
/// `\n` as `tower-lsp` alone accepted; a blank line before the first header
/// line is skipped.
fn header_end(buf: &[u8]) -> Option<usize> {
    let mut start = 0;
    let mut seen_header_line = false;
    while let Some(newline) = buf[start..].iter().position(|&b| b == b'\n') {
        let line = &buf[start..start + newline];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        start += newline + 1;
        if !line.is_empty() {
            seen_header_line = true;
        } else if seen_header_line {
            return Some(start);
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn find_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

/// The value of the `Content-Length` header in a header block.
///
/// When no header line is that header, the last place the words
/// `Content-Length:` appear is read instead: bytes left over from a frame
/// that could not be cut may sit in front of the header of the next frame,
/// on the same line.
fn content_length(header: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(header);
    let strict = text.split('\n').find_map(|line| {
        let (name, value) = line.trim_end_matches('\r').split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse().ok())
            .flatten()
    });
    if strict.is_some() {
        return strict;
    }
    let at = header
        .windows(MARKER.len())
        .rposition(|w| w.eq_ignore_ascii_case(MARKER))?;
    let rest = String::from_utf8_lossy(&header[at + MARKER.len()..]);
    rest.lines().next()?.trim().parse().ok()
}

/// What `tower-lsp` decodes a frame body as: the same two shapes it accepts.
#[derive(Deserialize)]
#[serde(untagged)]
enum Message {
    #[allow(dead_code)]
    Response(Response),
    Request(Request),
}

/// A frame ready to be written: its header and body.
fn frame_bytes(body: &[u8]) -> Vec<u8> {
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body);
    out
}

/// The error response for a frame that is not a message `tower-lsp` takes.
///
/// A body that is not JSON is a parse error with no id. A JSON value that is
/// not a request or response is an invalid request, answered under its `id`
/// when it has a usable one.
fn error_frame_for(body: &[u8], error: &serde_json::Error) -> Vec<u8> {
    let (id, rpc_error) = if error.is_data() {
        let id = serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| match value.get("id") {
                Some(serde_json::Value::Number(n)) => n.as_i64().map(Id::Number),
                Some(serde_json::Value::String(s)) => Some(Id::String(s.clone())),
                _ => None,
            })
            .unwrap_or(Id::Null);
        (id, Error::invalid_request())
    } else {
        (Id::Null, Error::parse_error())
    };
    response_frame(&Response::from_error(id, rpc_error))
}

fn response_frame(response: &Response) -> Vec<u8> {
    frame_bytes(&serde_json::to_vec(response).unwrap_or_else(|_| b"{}".to_vec()))
}

/// The input stream of the server, with the frames it cannot decode taken
/// out. See the module documentation.
pub(super) struct SanitizedInput<R> {
    inner: R,
    watch: Arc<InputWatch>,
    frames: FrameReader,
    /// Error responses for the client.
    errors: Sender<Vec<u8>>,
    /// Bytes for `tower-lsp`, not yet handed over.
    ready: VecDeque<u8>,
}

impl<R> SanitizedInput<R> {
    pub(super) fn new(inner: R, watch: Arc<InputWatch>, errors: Sender<Vec<u8>>) -> Self {
        Self {
            inner,
            watch,
            frames: FrameReader::default(),
            errors,
            ready: VecDeque::new(),
        }
    }

    /// Queue an error response for the client. A client that sends more bad
    /// frames than it reads answers for is not owed all of them: what does
    /// not fit in the queue is dropped.
    fn queue_error(&self, frame: Vec<u8>) {
        let _ = self.errors.try_send(frame);
    }

    fn accept(&mut self, events: Vec<Event>) {
        for event in events {
            // Nothing that comes after `exit` is passed on: `tower-lsp` ends
            // its work at `exit` only if no message follows it.
            if self.watch.exit_requested.load(Ordering::SeqCst) {
                break;
            }
            match event {
                Event::Frame(body) => match serde_json::from_slice::<Message>(&body) {
                    Ok(message) => {
                        if let Message::Request(request) = &message {
                            match request.method() {
                                "shutdown" if request.id().is_some() => {
                                    self.watch.shutdown_requested.store(true, Ordering::SeqCst)
                                }
                                // `tower-lsp` takes `exit` as the end whether
                                // or not the client gave it an id.
                                "exit" => self.watch.exit_requested.store(true, Ordering::SeqCst),
                                _ => {}
                            }
                        }
                        self.ready.extend(frame_bytes(&body));
                    }
                    Err(error) => {
                        self.queue_error(error_frame_for(&body, &error));
                    }
                },
                Event::BadHeader => {
                    self.queue_error(response_frame(&Response::from_error(
                        Id::Null,
                        Error::parse_error(),
                    )));
                }
            }
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for SanitizedInput<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        loop {
            if !this.ready.is_empty() {
                let take = this.ready.len().min(buf.remaining());
                let bytes: Vec<u8> = this.ready.drain(..take).collect();
                buf.put_slice(&bytes);
                if this.ready.is_empty() && this.ready.capacity() > KEPT_CAPACITY {
                    this.ready = VecDeque::new();
                }
                return Poll::Ready(Ok(()));
            }
            // Nothing the client sends after `exit` is read.
            if this.watch.exit_requested.load(Ordering::SeqCst) {
                return Poll::Ready(Ok(()));
            }
            let mut chunk = [0u8; 16 * 1024];
            let mut chunk_buf = ReadBuf::new(&mut chunk);
            match Pin::new(&mut this.inner).poll_read(cx, &mut chunk_buf) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {
                    let read = chunk_buf.filled();
                    if read.is_empty() {
                        this.watch.reached_eof.store(true, Ordering::SeqCst);
                        return Poll::Ready(Ok(()));
                    }
                    let events = this.frames.feed(read);
                    this.accept(events);
                }
            }
        }
    }
}

/// Splits the frames a writer produces from the bytes it writes.
#[derive(Default)]
struct OutputFrames {
    buf: Vec<u8>,
}

impl OutputFrames {
    /// Feed bytes; returns every complete frame (header included) in them.
    /// Bytes that do not look like a frame are returned as they are, so
    /// nothing is held back for good.
    fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        loop {
            let Some(end) = find(&self.buf, b"\r\n\r\n") else {
                break;
            };
            let Some(length) = content_length(&self.buf[..end + 4]) else {
                out.push(std::mem::take(&mut self.buf));
                break;
            };
            if self.buf.len() < end + 4 + length {
                break;
            }
            out.push(self.buf.drain(..end + 4 + length).collect());
        }
        out
    }

    fn rest(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }
}

/// Write everything `from_server` produces, and every queued error response,
/// to `out`, one whole frame at a time. Returns when both sources have ended.
pub(super) async fn pump_frames<S, W>(mut from_server: S, mut errors: Receiver<Vec<u8>>, mut out: W)
where
    S: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut frames = OutputFrames::default();
    let mut chunk = vec![0u8; 16 * 1024];
    let mut server_open = true;
    let mut errors_open = true;
    while server_open || errors_open {
        tokio::select! {
            read = from_server.read(&mut chunk), if server_open => match read {
                Ok(0) | Err(_) => server_open = false,
                Ok(n) => {
                    for frame in frames.feed(&chunk[..n]) {
                        if write_frame(&mut out, &frame).await.is_err() {
                            return;
                        }
                    }
                }
            },
            frame = errors.recv(), if errors_open => match frame {
                Some(frame) => {
                    if write_frame(&mut out, &frame).await.is_err() {
                        return;
                    }
                }
                None => errors_open = false,
            },
        }
    }
    let rest = frames.rest();
    if !rest.is_empty() {
        let _ = write_frame(&mut out, &rest).await;
    }
}

async fn write_frame<W: AsyncWrite + Unpin>(out: &mut W, frame: &[u8]) -> io::Result<()> {
    out.write_all(frame).await?;
    out.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &str) -> Vec<u8> {
        frame_bytes(body.as_bytes())
    }

    fn bodies(events: Vec<Event>) -> Vec<String> {
        events
            .into_iter()
            .map(|event| match event {
                Event::Frame(body) => String::from_utf8(body).unwrap(),
                Event::BadHeader => "<bad header>".to_string(),
            })
            .collect()
    }

    #[test]
    fn frames_are_cut_from_a_stream_however_it_is_split() {
        let mut bytes = frame(r#"{"a":1}"#);
        bytes.extend(frame(r#"{"b":2}"#));
        for split in 0..bytes.len() {
            let mut reader = FrameReader::default();
            let mut got = bodies(reader.feed(&bytes[..split]));
            got.extend(bodies(reader.feed(&bytes[split..])));
            assert_eq!(got, vec![r#"{"a":1}"#, r#"{"b":2}"#], "split at {split}");
        }
    }

    #[test]
    fn a_broken_header_is_reported_and_the_next_frame_still_read() {
        let mut bytes = b"Content-Length: nope\r\n\r\ngarbage".to_vec();
        bytes.extend(frame(r#"{"ok":true}"#));
        let mut reader = FrameReader::default();
        assert_eq!(
            bodies(reader.feed(&bytes)),
            vec!["<bad header>", r#"{"ok":true}"#]
        );
    }

    #[test]
    fn a_missing_length_is_reported_and_the_next_frame_still_read() {
        let mut bytes = b"Content-Type: x\r\n\r\n".to_vec();
        bytes.extend(frame(r#"{"ok":true}"#));
        let mut reader = FrameReader::default();
        assert_eq!(
            bodies(reader.feed(&bytes)),
            vec!["<bad header>", r#"{"ok":true}"#]
        );
    }

    #[test]
    fn header_names_are_case_insensitive_and_extra_headers_are_fine() {
        let body = r#"{"ok":true}"#;
        let bytes = format!(
            "content-length: {}\r\nContent-Type: application/vscode-jsonrpc; charset=utf-8\r\nX-Other: 1\r\n\r\n{}",
            body.len(),
            body
        );
        let mut reader = FrameReader::default();
        assert_eq!(bodies(reader.feed(bytes.as_bytes())), vec![body]);
    }

    #[test]
    fn a_header_ended_by_bare_line_feeds_is_accepted() {
        let body = r#"{"ok":true}"#;
        let bytes = format!("Content-Length: {}\n\n{}", body.len(), body);
        let mut reader = FrameReader::default();
        assert_eq!(bodies(reader.feed(bytes.as_bytes())), vec![body]);
    }

    #[test]
    fn a_frame_cut_wrongly_does_not_spoil_the_next_one_read_later() {
        // The length is one byte short, so a byte of the body is left over
        // and sits in front of the next header.
        let body = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
        let mut reader = FrameReader::default();
        let first = format!("Content-Length: {}\r\n\r\n{}", body.len() - 1, body);
        let mut got = bodies(reader.feed(first.as_bytes()));
        got.extend(bodies(reader.feed(&frame(r#"{"ok":true}"#))));
        assert_eq!(got.last().map(String::as_str), Some(r#"{"ok":true}"#));
        // A broken header whose body arrives in a read of its own.
        let mut reader = FrameReader::default();
        let mut got = bodies(reader.feed(b"Content-Length: nope\r\n\r\n"));
        got.extend(bodies(reader.feed(br#"{"junk":true}"#)));
        got.extend(bodies(reader.feed(&frame(r#"{"ok":true}"#))));
        assert_eq!(got.last().map(String::as_str), Some(r#"{"ok":true}"#));
    }

    #[test]
    fn a_header_that_never_ends_is_given_up_and_reading_goes_on() {
        let mut reader = FrameReader::default();
        let mut noisy = b"Content-Length: 2\r\nX-Pad: ".to_vec();
        noisy.extend(vec![b'a'; MAX_HEADER * 2]);
        let events = reader.feed(&noisy);
        assert!(events.contains(&Event::BadHeader));
        assert!(reader.buf.len() < MAX_HEADER, "{}", reader.buf.len());
        let got = bodies(reader.feed(&frame(r#"{"ok":true}"#)));
        assert_eq!(got.last().map(String::as_str), Some(r#"{"ok":true}"#));
    }

    #[test]
    fn noise_that_keeps_naming_the_header_is_reported_once() {
        let mut reader = FrameReader::default();
        let mut events = reader.feed(b"Content-Length: nope\r\n\r\n");
        // Junk that mentions the header again and again, with no blank line
        // after it for thousands of bytes.
        for _ in 0..5 {
            let mut junk = b"\"Content-Length: 5".to_vec();
            junk.extend(vec![b'y'; 9000]);
            events.extend(reader.feed(&junk));
        }
        assert_eq!(
            events
                .iter()
                .filter(|event| **event == Event::BadHeader)
                .count(),
            1
        );
        // A good frame is read, and a later bad header is reported again.
        let events = reader.feed(&frame(r#"{"ok":true}"#));
        assert_eq!(bodies(events), vec![r#"{"ok":true}"#]);
        assert_eq!(
            reader.feed(b"Content-Length: nope\r\n\r\n"),
            vec![Event::BadHeader]
        );
    }

    #[test]
    fn noise_without_a_header_marker_does_not_grow_without_bound() {
        let mut reader = FrameReader::default();
        let events = reader.feed(&vec![b'x'; MAX_HEADER * 3]);
        assert!(events.contains(&Event::BadHeader));
        assert!(reader.buf.len() < 64, "{}", reader.buf.len());
    }

    fn error_body(body: &str) -> serde_json::Value {
        let error = serde_json::from_slice::<Message>(body.as_bytes())
            .err()
            .expect("not a message");
        let frame = error_frame_for(body.as_bytes(), &error);
        let text = String::from_utf8(frame).unwrap();
        let (_, json) = text.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn text_that_is_not_json_is_a_parse_error_without_an_id() {
        let response = error_body("{not json");
        assert_eq!(response["error"]["code"], -32700);
        assert_eq!(response["id"], serde_json::Value::Null);
    }

    #[test]
    fn json_that_is_not_a_request_is_an_invalid_request_answered_under_its_id() {
        let response = error_body(r#"{"id": 5, "method": "textDocument/hover"}"#);
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["id"], 5);
        let response = error_body(r#"{"id": 1.5, "jsonrpc": "2.0", "method": "x"}"#);
        assert_eq!(response["error"]["code"], -32600);
        assert_eq!(response["id"], serde_json::Value::Null);
        let response = error_body(r#"[{"jsonrpc": "2.0", "method": "x"}]"#);
        assert_eq!(response["error"]["code"], -32600);
    }

    #[test]
    fn requests_notifications_and_responses_are_messages() {
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":7,"result":null}"#,
            r#"{"jsonrpc":"2.0","id":null,"method":"x"}"#,
        ] {
            assert!(
                serde_json::from_slice::<Message>(body.as_bytes()).is_ok(),
                "{body}"
            );
        }
    }

    #[tokio::test]
    async fn sanitized_input_passes_valid_frames_and_answers_the_rest() {
        let mut bytes = frame(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        bytes.extend(frame("{not json"));
        bytes.extend(frame(r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#));
        bytes.extend(frame(r#"{"jsonrpc":"2.0","method":"exit"}"#));
        bytes.extend(frame(r#"{"jsonrpc":"2.0","id":3,"method":"never read"}"#));
        let watch = Arc::new(InputWatch::default());
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let mut input = SanitizedInput::new(&bytes[..], watch.clone(), tx);
        let mut passed = Vec::new();
        input.read_to_end(&mut passed).await.unwrap();
        let passed = String::from_utf8(passed).unwrap();
        assert!(passed.contains("initialize") && passed.contains("shutdown"));
        assert!(passed.contains(r#""method":"exit""#));
        assert!(!passed.contains("not json"));
        assert!(watch.shutdown_requested.load(Ordering::SeqCst));
        assert!(watch.exit_requested.load(Ordering::SeqCst));
        let error = rx.recv().await.expect("an error response was queued");
        assert!(String::from_utf8(error).unwrap().contains("-32700"));
    }

    #[tokio::test]
    async fn nothing_after_exit_is_passed_on_whether_or_not_exit_has_an_id() {
        for exit in [
            r#"{"jsonrpc":"2.0","method":"exit"}"#,
            r#"{"jsonrpc":"2.0","id":9,"method":"exit"}"#,
        ] {
            let mut bytes = frame(exit);
            bytes.extend(frame(
                r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
            ));
            let watch = Arc::new(InputWatch::default());
            let (tx, _rx) = tokio::sync::mpsc::channel(16);
            let mut input = SanitizedInput::new(&bytes[..], watch.clone(), tx);
            let mut passed = Vec::new();
            input.read_to_end(&mut passed).await.unwrap();
            let passed = String::from_utf8(passed).unwrap();
            assert!(passed.contains(r#""method":"exit""#), "{passed}");
            assert!(!passed.contains("initialized"), "{passed}");
            assert!(watch.exit_requested.load(Ordering::SeqCst));
        }
    }

    #[test]
    fn output_frames_are_written_whole() {
        let mut out = OutputFrames::default();
        let mut bytes = frame(r#"{"a":1}"#);
        bytes.extend(frame(r#"{"b":2}"#));
        let (first, second) = bytes.split_at(20);
        assert!(out.feed(first).is_empty());
        let frames = out.feed(second);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], frame(r#"{"a":1}"#));
    }

    #[tokio::test]
    async fn the_pump_keeps_server_frames_and_error_frames_apart() {
        let (mut server_side, pump_side) = tokio::io::duplex(1024);
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let (out_writer, mut out_reader) = tokio::io::duplex(1 << 20);
        let pump = tokio::spawn(pump_frames(pump_side, rx, out_writer));

        // A frame of the server arrives in two parts, with an error frame
        // queued between them.
        let first = frame(r#"{"jsonrpc":"2.0","id":1,"result":null}"#);
        let (a, b) = first.split_at(10);
        server_side.write_all(a).await.unwrap();
        tx.send(frame(r#"{"error":true}"#)).await.unwrap();
        tokio::task::yield_now().await;
        server_side.write_all(b).await.unwrap();
        drop(server_side);
        drop(tx);
        pump.await.unwrap();

        let mut written = String::new();
        out_reader.read_to_string(&mut written).await.unwrap();
        let frames: Vec<&str> = written.split("Content-Length: ").skip(1).collect();
        assert_eq!(frames.len(), 2, "{written}");
        for piece in frames {
            let (header, body) = piece.split_once("\r\n\r\n").unwrap();
            assert_eq!(
                header.trim().parse::<usize>().unwrap(),
                body.len(),
                "{piece}"
            );
        }
    }
}
