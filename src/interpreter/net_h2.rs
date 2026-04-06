/// HTTP/2 reference implementation for `taida-lang/net` v6.
///
/// This module implements the HTTP/2 protocol as defined in RFC 9113 for the
/// Interpreter backend. It serves as the reference implementation that the
/// Native backend (Phase 3) will follow.
///
/// # Architecture
///
/// The h2 implementation is structured as follows:
///
/// 1. **Frame layer**: Parse and serialize HTTP/2 frames (9-byte header + payload)
/// 2. **HPACK**: Header compression/decompression (RFC 7541)
/// 3. **Stream state machine**: Per-stream lifecycle (idle -> open -> half-closed -> closed)
/// 4. **Connection state**: Connection-level flow control, settings, GOAWAY
/// 5. **Server loop**: Accept h2 connections, dispatch requests to the Taida handler
///
/// # Design Decisions
///
/// - Synchronous/blocking I/O (consistent with the Interpreter's single-threaded model)
/// - Connection preface validation per RFC 9113 Section 3.4
/// - ALPN "h2" negotiation via rustls (TLS-only, h2c is out of scope)
/// - Stream multiplexing within a single connection (serial handler dispatch)
/// - Connection-local buffer reuse (no aggregate buffers on the hot path)
/// - HPACK with static table + dynamic table (RFC 7541)
use std::collections::{HashMap, VecDeque};
use std::io::{self, Read, Write};

// ── HTTP/2 Constants ────────────────────────────────────────────────

/// HTTP/2 connection preface (RFC 9113 Section 3.4).
/// The client MUST send this as the first 24 bytes of the connection.
pub(crate) const CONNECTION_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Default initial window size for flow control (RFC 9113 Section 6.9.2).
pub(crate) const DEFAULT_INITIAL_WINDOW_SIZE: u32 = 65_535;

/// Default max frame size (RFC 9113 Section 6.5.2).
const DEFAULT_MAX_FRAME_SIZE: u32 = 16_384;

/// Maximum allowed max frame size (RFC 9113 Section 6.5.2).
const MAX_MAX_FRAME_SIZE: u32 = 16_777_215;

/// Default header table size for HPACK (RFC 7541 Section 4.2).
const DEFAULT_HEADER_TABLE_SIZE: u32 = 4_096;

/// Maximum concurrent streams default (server-controlled).
const DEFAULT_MAX_CONCURRENT_STREAMS: u32 = 128;

/// Maximum flow-control window size (RFC 9113 Section 6.9.1).
/// A sender MUST NOT allow a flow-control window to exceed 2^31 - 1 octets.
const MAX_FLOW_CONTROL_WINDOW: i64 = 0x7FFFFFFF;

/// Maximum CONTINUATION buffer size before we reject the header block.
/// This protects against memory exhaustion from HPACK bombs or malformed
/// CONTINUATION sequences. 128KB is generous for real-world header blocks.
const MAX_CONTINUATION_BUFFER_SIZE: usize = 128 * 1024;

/// Maximum total decoded header list size (name + value + 32 overhead per entry).
/// RFC 9113 Section 6.5.2: SETTINGS_MAX_HEADER_LIST_SIZE advisory limit.
/// We enforce 64KB as a hard safety limit to prevent HPACK bombs.
const MAX_DECODED_HEADER_LIST_SIZE: usize = 64 * 1024;

// ── Frame Types (RFC 9113 Section 6) ────────────────────────────────

pub(crate) const FRAME_DATA: u8 = 0x0;
pub(crate) const FRAME_HEADERS: u8 = 0x1;
pub(crate) const FRAME_PRIORITY: u8 = 0x2;
pub(crate) const FRAME_RST_STREAM: u8 = 0x3;
pub(crate) const FRAME_SETTINGS: u8 = 0x4;
pub(crate) const FRAME_PUSH_PROMISE: u8 = 0x5;
pub(crate) const FRAME_PING: u8 = 0x6;
pub(crate) const FRAME_GOAWAY: u8 = 0x7;
pub(crate) const FRAME_WINDOW_UPDATE: u8 = 0x8;
pub(crate) const FRAME_CONTINUATION: u8 = 0x9;

// ── Frame Flags ─────────────────────────────────────────────────────

pub(crate) const FLAG_END_STREAM: u8 = 0x1;
pub(crate) const FLAG_ACK: u8 = 0x1; // For SETTINGS and PING
pub(crate) const FLAG_END_HEADERS: u8 = 0x4;
pub(crate) const FLAG_PADDED: u8 = 0x8;
pub(crate) const FLAG_PRIORITY: u8 = 0x20;

// ── Error Codes (RFC 9113 Section 7) ────────────────────────────────

#[allow(dead_code)]
const ERROR_NO_ERROR: u32 = 0x0;
const ERROR_PROTOCOL_ERROR: u32 = 0x1;
const ERROR_INTERNAL_ERROR: u32 = 0x2;
pub(crate) const ERROR_FLOW_CONTROL_ERROR: u32 = 0x3;
#[allow(dead_code)]
const ERROR_SETTINGS_TIMEOUT: u32 = 0x4;
const ERROR_STREAM_CLOSED: u32 = 0x5;
const ERROR_FRAME_SIZE_ERROR: u32 = 0x6;
#[allow(dead_code)]
const ERROR_REFUSED_STREAM: u32 = 0x7;
#[allow(dead_code)]
const ERROR_CANCEL: u32 = 0x8;
const ERROR_COMPRESSION_ERROR: u32 = 0x9;
#[allow(dead_code)]
const ERROR_CONNECT_ERROR: u32 = 0xa;
#[allow(dead_code)]
const ERROR_ENHANCE_YOUR_CALM: u32 = 0xb;
#[allow(dead_code)]
const ERROR_INADEQUATE_SECURITY: u32 = 0xc;
#[allow(dead_code)]
const ERROR_HTTP_1_1_REQUIRED: u32 = 0xd;

// ── Settings Identifiers (RFC 9113 Section 6.5.1) ───────────────────

const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
const SETTINGS_ENABLE_PUSH: u16 = 0x2;
const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

// ── HPACK Static Table (RFC 7541 Appendix A) ────────────────────────

/// Static table entries. Index is 1-based (entry 0 is unused).
const HPACK_STATIC_TABLE: &[(&str, &str)] = &[
    ("", ""),                             // 0: unused
    (":authority", ""),                   // 1
    (":method", "GET"),                   // 2
    (":method", "POST"),                  // 3
    (":path", "/"),                       // 4
    (":path", "/index.html"),             // 5
    (":scheme", "http"),                  // 6
    (":scheme", "https"),                 // 7
    (":status", "200"),                   // 8
    (":status", "204"),                   // 9
    (":status", "206"),                   // 10
    (":status", "304"),                   // 11
    (":status", "400"),                   // 12
    (":status", "404"),                   // 13
    (":status", "500"),                   // 14
    ("accept-charset", ""),               // 15
    ("accept-encoding", "gzip, deflate"), // 16
    ("accept-language", ""),              // 17
    ("accept-ranges", ""),                // 18
    ("accept", ""),                       // 19
    ("access-control-allow-origin", ""),  // 20
    ("age", ""),                          // 21
    ("allow", ""),                        // 22
    ("authorization", ""),                // 23
    ("cache-control", ""),                // 24
    ("content-disposition", ""),          // 25
    ("content-encoding", ""),             // 26
    ("content-language", ""),             // 27
    ("content-length", ""),               // 28
    ("content-location", ""),             // 29
    ("content-range", ""),                // 30
    ("content-type", ""),                 // 31
    ("cookie", ""),                       // 32
    ("date", ""),                         // 33
    ("etag", ""),                         // 34
    ("expect", ""),                       // 35
    ("expires", ""),                      // 36
    ("from", ""),                         // 37
    ("host", ""),                         // 38
    ("if-match", ""),                     // 39
    ("if-modified-since", ""),            // 40
    ("if-none-match", ""),                // 41
    ("if-range", ""),                     // 42
    ("if-unmodified-since", ""),          // 43
    ("last-modified", ""),                // 44
    ("link", ""),                         // 45
    ("location", ""),                     // 46
    ("max-forwards", ""),                 // 47
    ("proxy-authenticate", ""),           // 48
    ("proxy-authorization", ""),          // 49
    ("range", ""),                        // 50
    ("referer", ""),                      // 51
    ("refresh", ""),                      // 52
    ("retry-after", ""),                  // 53
    ("server", ""),                       // 54
    ("set-cookie", ""),                   // 55
    ("strict-transport-security", ""),    // 56
    ("transfer-encoding", ""),            // 57
    ("user-agent", ""),                   // 58
    ("vary", ""),                         // 59
    ("via", ""),                          // 60
    ("www-authenticate", ""),             // 61
];

/// NB6-39: Pre-built HashMap for O(1) static table name lookup (encoder).
/// Maps header name -> first 1-based index in HPACK_STATIC_TABLE.
static STATIC_NAME_MAP: std::sync::LazyLock<HashMap<&'static str, usize>> =
    std::sync::LazyLock::new(|| {
        let mut m = HashMap::new();
        for (i, &(name, _)) in HPACK_STATIC_TABLE.iter().enumerate().skip(1) {
            m.entry(name).or_insert(i);
        }
        m
    });

/// NB6-39: Pre-built HashMap for O(1) static table exact (name+value) lookup.
/// Maps (name, value) -> 1-based index. Only includes entries with non-empty values.
static STATIC_EXACT_MAP: std::sync::LazyLock<HashMap<(&'static str, &'static str), usize>> =
    std::sync::LazyLock::new(|| {
        let mut m = HashMap::new();
        for (i, &(name, value)) in HPACK_STATIC_TABLE.iter().enumerate().skip(1) {
            if !value.is_empty() {
                m.insert((name, value), i);
            }
        }
        m
    });

// ── Frame Parsing / Serialization ───────────────────────────────────

/// A parsed HTTP/2 frame header (9 bytes).
#[derive(Debug, Clone)]
pub(crate) struct FrameHeader {
    pub length: u32, // 24-bit payload length
    pub frame_type: u8,
    pub flags: u8,
    pub stream_id: u32, // 31-bit stream identifier (R bit masked)
}

impl FrameHeader {
    /// Parse a 9-byte frame header.
    pub fn parse(buf: &[u8; 9]) -> Self {
        let length = ((buf[0] as u32) << 16) | ((buf[1] as u32) << 8) | (buf[2] as u32);
        let frame_type = buf[3];
        let flags = buf[4];
        let stream_id = u32::from_be_bytes([buf[5] & 0x7F, buf[6], buf[7], buf[8]]);
        FrameHeader {
            length,
            frame_type,
            flags,
            stream_id,
        }
    }

    /// Serialize to a 9-byte buffer.
    pub fn serialize(&self) -> [u8; 9] {
        let mut buf = [0u8; 9];
        buf[0] = ((self.length >> 16) & 0xFF) as u8;
        buf[1] = ((self.length >> 8) & 0xFF) as u8;
        buf[2] = (self.length & 0xFF) as u8;
        buf[3] = self.frame_type;
        buf[4] = self.flags;
        let id_bytes = self.stream_id.to_be_bytes();
        buf[5] = id_bytes[0] & 0x7F; // R bit always 0
        buf[6] = id_bytes[1];
        buf[7] = id_bytes[2];
        buf[8] = id_bytes[3];
        buf
    }
}

// ── Stream State Machine (RFC 9113 Section 5.1) ─────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamState {
    /// Stream has been created but no frames received/sent.
    Idle,
    /// HEADERS received with END_STREAM; request is complete.
    /// We can send response HEADERS + DATA.
    HalfClosedRemote,
    /// Both sides have sent END_STREAM; stream is done.
    Closed,
}

/// Per-stream state.
pub(crate) struct H2Stream {
    pub state: StreamState,
    /// Accumulated request headers (decoded from HPACK).
    pub request_headers: Vec<(String, String)>,
    /// Accumulated request body data.
    pub request_body: Vec<u8>,
    /// Stream-level send window (how much we can send).
    pub send_window: i64,
    /// Stream-level receive window (how much peer can send).
    pub recv_window: i64,
}

impl H2Stream {
    fn new(initial_window_size: u32) -> Self {
        H2Stream {
            state: StreamState::Idle,
            request_headers: Vec::new(),
            request_body: Vec::new(),
            send_window: initial_window_size as i64,
            recv_window: DEFAULT_INITIAL_WINDOW_SIZE as i64,
        }
    }
}

// ── HPACK Decoder ───────────────────────────────────────────────────

/// HPACK dynamic table entry.
#[derive(Debug, Clone)]
struct HpackEntry {
    name: String,
    value: String,
}

impl HpackEntry {
    fn size(&self) -> usize {
        // RFC 7541 Section 4.1: size = name octets + value octets + 32
        self.name.len() + self.value.len() + 32
    }
}

/// HPACK decoder state (per-connection).
pub(crate) struct HpackDecoder {
    // NB6-33: VecDeque for O(1) push_front / pop_back instead of O(n) Vec::insert(0, ..)
    dynamic_table: VecDeque<HpackEntry>,
    max_table_size: usize,
    current_table_size: usize,
}

impl HpackDecoder {
    pub fn new(max_size: u32) -> Self {
        HpackDecoder {
            dynamic_table: VecDeque::new(),
            max_table_size: max_size as usize,
            current_table_size: 0,
        }
    }

    /// Decode an HPACK header block into a list of (name, value) pairs.
    pub fn decode(&mut self, data: &[u8]) -> Result<Vec<(String, String)>, H2Error> {
        let mut headers = Vec::new();
        let mut pos = 0;

        while pos < data.len() {
            let byte = data[pos];

            if byte & 0x80 != 0 {
                // Indexed header field (Section 6.1)
                let (index, consumed) = decode_integer(&data[pos..], 7)?;
                pos += consumed;
                let (name, value) = self.get_indexed(index)?;
                headers.push((name, value));
            } else if byte & 0x40 != 0 {
                // Literal header field with incremental indexing (Section 6.2.1)
                let (name, value, consumed) = self.decode_literal(&data[pos..], 6, true)?;
                pos += consumed;
                headers.push((name, value));
            } else if byte & 0x20 != 0 {
                // Dynamic table size update (Section 6.3)
                let (new_size, consumed) = decode_integer(&data[pos..], 5)?;
                pos += consumed;
                self.update_max_size(new_size);
            } else if byte & 0x10 != 0 {
                // Literal header field never indexed (Section 6.2.3)
                let (name, value, consumed) = self.decode_literal(&data[pos..], 4, false)?;
                pos += consumed;
                headers.push((name, value));
            } else {
                // Literal header field without indexing (Section 6.2.2)
                let (name, value, consumed) = self.decode_literal(&data[pos..], 4, false)?;
                pos += consumed;
                headers.push((name, value));
            }
        }

        Ok(headers)
    }

    /// Look up an indexed header from static or dynamic table.
    fn get_indexed(&self, index: usize) -> Result<(String, String), H2Error> {
        if index == 0 {
            return Err(H2Error::Compression("HPACK: index 0 is invalid".into()));
        }
        if index < HPACK_STATIC_TABLE.len() {
            let (name, value) = HPACK_STATIC_TABLE[index];
            Ok((name.to_string(), value.to_string()))
        } else {
            let dyn_index = index - HPACK_STATIC_TABLE.len();
            if dyn_index < self.dynamic_table.len() {
                let entry = &self.dynamic_table[dyn_index];
                Ok((entry.name.clone(), entry.value.clone()))
            } else {
                Err(H2Error::Compression(format!(
                    "HPACK: index {} out of range (static={}, dynamic={})",
                    index,
                    HPACK_STATIC_TABLE.len() - 1,
                    self.dynamic_table.len()
                )))
            }
        }
    }

    /// Look up just the name from an index.
    fn get_indexed_name(&self, index: usize) -> Result<String, H2Error> {
        if index == 0 {
            return Err(H2Error::Compression(
                "HPACK: index 0 is invalid for name lookup".into(),
            ));
        }
        if index < HPACK_STATIC_TABLE.len() {
            Ok(HPACK_STATIC_TABLE[index].0.to_string())
        } else {
            let dyn_index = index - HPACK_STATIC_TABLE.len();
            if dyn_index < self.dynamic_table.len() {
                Ok(self.dynamic_table[dyn_index].name.clone())
            } else {
                Err(H2Error::Compression(format!(
                    "HPACK: name index {} out of range",
                    index
                )))
            }
        }
    }

    /// Decode a literal header field.
    /// Returns (name, value, bytes_consumed).
    fn decode_literal(
        &mut self,
        data: &[u8],
        prefix_bits: u8,
        add_to_table: bool,
    ) -> Result<(String, String, usize), H2Error> {
        let mut pos = 0;

        // Decode the index (0 = new name, >0 = indexed name)
        let (index, consumed) = decode_integer(&data[pos..], prefix_bits)?;
        pos += consumed;

        let name = if index == 0 {
            // New name: decode string literal
            let (name_str, consumed) = decode_string(&data[pos..])?;
            pos += consumed;
            name_str
        } else {
            self.get_indexed_name(index)?
        };

        // Decode value string
        let (value, consumed) = decode_string(&data[pos..])?;
        pos += consumed;

        if add_to_table {
            self.add_entry(name.clone(), value.clone());
        }

        Ok((name, value, pos))
    }

    /// Add an entry to the dynamic table (FIFO, newest at index 0).
    fn add_entry(&mut self, name: String, value: String) {
        let entry = HpackEntry { name, value };
        let entry_size = entry.size();

        // Evict entries until we have room
        while self.current_table_size + entry_size > self.max_table_size {
            // NB6-33: pop_back() on VecDeque is O(1)
            if let Some(evicted) = self.dynamic_table.pop_back() {
                self.current_table_size -= evicted.size();
            } else {
                break;
            }
        }

        // Only add if the entry fits
        if entry_size <= self.max_table_size {
            self.current_table_size += entry_size;
            // NB6-33: push_front() on VecDeque is O(1) instead of Vec::insert(0, ..) O(n)
            self.dynamic_table.push_front(entry);
        }
    }

    /// Update the maximum dynamic table size.
    fn update_max_size(&mut self, new_size: usize) {
        self.max_table_size = new_size;
        // Evict entries that no longer fit
        while self.current_table_size > self.max_table_size {
            if let Some(evicted) = self.dynamic_table.pop_back() {
                self.current_table_size -= evicted.size();
            } else {
                break;
            }
        }
    }
}

// ── HPACK Encoder ───────────────────────────────────────────────────

/// HPACK encoder state (per-connection).
pub(crate) struct HpackEncoder {
    // NB6-33: VecDeque for O(1) push_front / pop_back instead of O(n) Vec::insert(0, ..)
    dynamic_table: VecDeque<HpackEntry>,
    max_table_size: usize,
    current_table_size: usize,
}

impl HpackEncoder {
    pub fn new(max_size: u32) -> Self {
        HpackEncoder {
            dynamic_table: VecDeque::new(),
            max_table_size: max_size as usize,
            current_table_size: 0,
        }
    }

    /// Encode a list of headers into an HPACK header block.
    pub fn encode(&mut self, headers: &[(String, String)]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(128);

        for (name, value) in headers {
            // Try static table exact match first
            if let Some(idx) = self.find_static_exact(name, value) {
                // Indexed header field representation
                encode_integer(&mut buf, idx, 7, 0x80);
            } else if let Some(idx) = self.find_static_name(name) {
                // Literal with incremental indexing, indexed name
                encode_integer(&mut buf, idx, 6, 0x40);
                encode_string(&mut buf, value);
                self.add_entry(name.clone(), value.clone());
            } else {
                // Literal with incremental indexing, new name
                buf.push(0x40);
                encode_string(&mut buf, name);
                encode_string(&mut buf, value);
                self.add_entry(name.clone(), value.clone());
            }
        }

        buf
    }

    /// Find exact match in static table. Returns 1-based index.
    /// NB6-39: O(1) HashMap lookup instead of O(62) linear scan.
    fn find_static_exact(&self, name: &str, value: &str) -> Option<usize> {
        STATIC_EXACT_MAP.get(&(name, value)).copied()
    }

    /// Find name-only match in static table. Returns 1-based index.
    /// NB6-39: O(1) HashMap lookup instead of O(62) linear scan.
    fn find_static_name(&self, name: &str) -> Option<usize> {
        STATIC_NAME_MAP.get(name).copied()
    }

    /// Update the maximum dynamic table size.
    pub fn update_max_size(&mut self, new_size: usize) {
        self.max_table_size = new_size;
        while self.current_table_size > self.max_table_size {
            if let Some(evicted) = self.dynamic_table.pop_back() {
                self.current_table_size -= evicted.size();
            } else {
                break;
            }
        }
    }

    /// Add an entry to the encoder's dynamic table.
    fn add_entry(&mut self, name: String, value: String) {
        let entry = HpackEntry { name, value };
        let entry_size = entry.size();

        while self.current_table_size + entry_size > self.max_table_size {
            if let Some(evicted) = self.dynamic_table.pop_back() {
                self.current_table_size -= evicted.size();
            } else {
                break;
            }
        }

        if entry_size <= self.max_table_size {
            self.current_table_size += entry_size;
            // NB6-33: push_front() on VecDeque is O(1) instead of Vec::insert(0, ..) O(n)
            self.dynamic_table.push_front(entry);
        }
    }
}

// ── HPACK Integer Coding (RFC 7541 Section 5.1) ─────────────────────

/// Decode an HPACK integer with the given prefix bit count.
/// Returns (value, bytes_consumed).
fn decode_integer(data: &[u8], prefix_bits: u8) -> Result<(usize, usize), H2Error> {
    if data.is_empty() {
        return Err(H2Error::Compression(
            "HPACK: unexpected end of input for integer".into(),
        ));
    }

    let mask = (1u8 << prefix_bits) - 1;
    let mut value = (data[0] & mask) as usize;
    let mut pos = 1;

    if value < mask as usize {
        return Ok((value, pos));
    }

    // Multi-byte integer
    let mut shift = 0u32;
    loop {
        if pos >= data.len() {
            return Err(H2Error::Compression("HPACK: truncated integer".into()));
        }
        let byte = data[pos];
        pos += 1;

        value += ((byte & 0x7F) as usize) << shift;
        shift += 7;

        if byte & 0x80 == 0 {
            break;
        }

        // Prevent overflow: limit to 4 continuation bytes (fits in usize)
        if shift > 28 {
            return Err(H2Error::Compression("HPACK: integer overflow".into()));
        }
    }

    Ok((value, pos))
}

/// Encode an HPACK integer with the given prefix bit count.
fn encode_integer(buf: &mut Vec<u8>, value: usize, prefix_bits: u8, prefix_pattern: u8) {
    let mask = (1u8 << prefix_bits) - 1;

    if value < mask as usize {
        buf.push(prefix_pattern | (value as u8));
    } else {
        buf.push(prefix_pattern | mask);
        let mut remaining = value - mask as usize;
        while remaining >= 128 {
            buf.push((remaining & 0x7F) as u8 | 0x80);
            remaining >>= 7;
        }
        buf.push(remaining as u8);
    }
}

// ── HPACK String Coding (RFC 7541 Section 5.2) ──────────────────────

/// Decode an HPACK string literal (no Huffman for simplicity).
/// Returns (string, bytes_consumed).
fn decode_string(data: &[u8]) -> Result<(String, usize), H2Error> {
    if data.is_empty() {
        return Err(H2Error::Compression(
            "HPACK: unexpected end of input for string".into(),
        ));
    }

    let huffman = data[0] & 0x80 != 0;
    let (length, consumed) = decode_integer(data, 7)?;
    let start = consumed;

    if start + length > data.len() {
        return Err(H2Error::Compression(format!(
            "HPACK: string length {} exceeds available data {}",
            length,
            data.len() - start
        )));
    }

    let raw = &data[start..start + length];

    let s = if huffman {
        // Decode Huffman-encoded string
        decode_huffman(raw)?
    } else {
        String::from_utf8(raw.to_vec())
            .map_err(|_| H2Error::Compression("HPACK: invalid UTF-8 in string literal".into()))?
    };

    Ok((s, start + length))
}

/// Encode an HPACK string literal (no Huffman encoding for simplicity).
fn encode_string(buf: &mut Vec<u8>, s: &str) {
    // Always use raw (non-Huffman) encoding for simplicity
    encode_integer(buf, s.len(), 7, 0x00);
    buf.extend_from_slice(s.as_bytes());
}

// ── Huffman Decoding (RFC 7541 Appendix B) ──────────────────────────

/// HPACK Huffman decode table.
/// Each entry: (symbol, bit_length)
/// Built from the Huffman code table in RFC 7541 Appendix B.
///
/// For decoding, we use a simple bit-by-bit approach. This is not the fastest
/// but is correct and sufficient for the Interpreter reference implementation.
fn decode_huffman(data: &[u8]) -> Result<String, H2Error> {
    let mut result = Vec::new();
    let mut bits: u64 = 0;
    let mut bits_left: u8 = 0;

    for &byte in data {
        bits = (bits << 8) | byte as u64;
        bits_left += 8;

        while bits_left >= 5 {
            // Try to match from longest to shortest code
            if let Some((sym, len)) = try_decode_huffman_symbol(bits, bits_left) {
                // NB7-75: RFC 7541 Section 5.2 — EOS symbol (256) MUST NOT be emitted
                // by an encoder. Treat it as a decoding error to prevent request smuggling.
                if sym == 256 {
                    return Err(H2Error::Compression(
                        "HPACK: Huffman EOS symbol forbidden in header compression".into(),
                    ));
                }
                result.push(sym as u8);
                bits_left -= len;
                // Mask off the consumed bits
                bits &= (1u64 << bits_left) - 1;
            } else if bits_left < 30 {
                // Not enough bits for any symbol yet
                break;
            } else {
                return Err(H2Error::Compression(
                    "HPACK: invalid Huffman encoding".into(),
                ));
            }
        }
    }

    // Check for valid padding (RFC 7541 Section 5.2):
    // remaining bits must be at most 7 and must be all 1s
    if bits_left > 7 {
        return Err(H2Error::Compression(
            "HPACK: Huffman padding exceeds 7 bits".into(),
        ));
    }
    if bits_left > 0 {
        let padding_mask = (1u64 << bits_left) - 1;
        if (bits & padding_mask) != padding_mask {
            return Err(H2Error::Compression(
                "HPACK: invalid Huffman padding (not all 1s)".into(),
            ));
        }
    }

    String::from_utf8(result)
        .map_err(|_| H2Error::Compression("HPACK: Huffman decoded to invalid UTF-8".into()))
}

/// Public Huffman decode for QPACK reuse (QPACK uses the same Huffman table as HPACK).
/// Returns `Some(decoded_string)` on success, `None` on error.
/// Used by `net_h3.rs` for QPACK string decoding (RFC 9204 Section 4.1.2).
pub(crate) fn huffman_decode(data: &[u8]) -> Option<String> {
    decode_huffman(data).ok()
}

/// Try to decode one Huffman symbol from the bit buffer.
/// Returns (symbol_byte, bits_consumed) if successful.
fn try_decode_huffman_symbol(bits: u64, bits_left: u8) -> Option<(u16, u8)> {
    // Check each symbol in the Huffman table (RFC 7541 Appendix B).
    // We align the top bits of our buffer and compare against each code.
    for &(sym, code, code_len) in HUFFMAN_TABLE.iter() {
        if bits_left >= code_len {
            let shift = bits_left - code_len;
            let candidate = (bits >> shift) as u32;
            if candidate == code && code_len > 0 {
                return Some((sym, code_len));
            }
        }
    }
    None
}

/// Huffman code table from RFC 7541 Appendix B.
/// Each entry: (symbol, code, code_length_in_bits)
/// Sorted by code length for decode efficiency.
#[rustfmt::skip]
const HUFFMAN_TABLE: &[(u16, u32, u8)] = &[
    // 5-bit codes
    (  48, 0x00,  5), // '0'
    (  49, 0x01,  5), // '1'
    (  50, 0x02,  5), // '2'
    (  97, 0x03,  5), // 'a'
    (  99, 0x04,  5), // 'c'
    ( 101, 0x05,  5), // 'e'
    ( 105, 0x06,  5), // 'i'
    ( 111, 0x07,  5), // 'o'
    ( 115, 0x08,  5), // 's'
    ( 116, 0x09,  5), // 't'
    // 6-bit codes
    (  32, 0x14,  6), // ' '
    (  37, 0x15,  6), // '%'
    (  45, 0x16,  6), // '-'
    (  46, 0x17,  6), // '.'
    (  47, 0x18,  6), // '/'
    (  51, 0x19,  6), // '3'
    (  52, 0x1a,  6), // '4'
    (  53, 0x1b,  6), // '5'
    (  54, 0x1c,  6), // '6'
    (  55, 0x1d,  6), // '7'
    (  56, 0x1e,  6), // '8'
    (  57, 0x1f,  6), // '9'
    (  61, 0x20,  6), // '='
    (  65, 0x21,  6), // 'A'
    (  95, 0x22,  6), // '_'
    (  98, 0x23,  6), // 'b'
    ( 100, 0x24,  6), // 'd'
    ( 102, 0x25,  6), // 'f'
    ( 103, 0x26,  6), // 'g'
    ( 104, 0x27,  6), // 'h'
    ( 108, 0x28,  6), // 'l'
    ( 109, 0x29,  6), // 'm'
    ( 110, 0x2a,  6), // 'n'
    ( 112, 0x2b,  6), // 'p'
    ( 114, 0x2c,  6), // 'r'
    ( 117, 0x2d,  6), // 'u'
    // 7-bit codes
    (  58, 0x5c,  7), // ':'
    (  66, 0x5d,  7), // 'B'
    (  67, 0x5e,  7), // 'C'
    (  68, 0x5f,  7), // 'D'
    (  69, 0x60,  7), // 'E'
    (  70, 0x61,  7), // 'F'
    (  71, 0x62,  7), // 'G'
    (  72, 0x63,  7), // 'H'
    (  73, 0x64,  7), // 'I'
    (  74, 0x65,  7), // 'J'
    (  75, 0x66,  7), // 'K'
    (  76, 0x67,  7), // 'L'
    (  77, 0x68,  7), // 'M'
    (  78, 0x69,  7), // 'N'
    (  79, 0x6a,  7), // 'O'
    (  80, 0x6b,  7), // 'P'
    (  81, 0x6c,  7), // 'Q'
    (  82, 0x6d,  7), // 'R'
    (  83, 0x6e,  7), // 'S'
    (  84, 0x6f,  7), // 'T'
    (  85, 0x70,  7), // 'U'
    (  86, 0x71,  7), // 'V'
    (  87, 0x72,  7), // 'W'
    (  89, 0x73,  7), // 'Y'
    ( 106, 0x74,  7), // 'j'
    ( 107, 0x75,  7), // 'k'
    ( 113, 0x76,  7), // 'q'
    ( 118, 0x77,  7), // 'v'
    ( 119, 0x78,  7), // 'w'
    ( 120, 0x79,  7), // 'x'
    ( 121, 0x7a,  7), // 'y'
    ( 122, 0x7b,  7), // 'z'
    // 8-bit codes
    (  38, 0xf8,  8), // '&'
    (  42, 0xf9,  8), // '*'
    (  44, 0xfa,  8), // ','
    (  59, 0xfb,  8), // ';'
    (  88, 0xfc,  8), // 'X'
    (  90, 0xfd,  8), // 'Z'
    // 10-bit codes
    (  33, 0x3f8, 10), // '!'
    (  34, 0x3f9, 10), // '"'
    (  40, 0x3fa, 10), // '('
    (  41, 0x3fb, 10), // ')'
    (  63, 0x3fc, 10), // '?'
    // 11-bit codes
    (  39, 0x7fa, 11), // '\''
    (  43, 0x7fb, 11), // '+'
    ( 124, 0x7fc, 11), // '|'
    // 12-bit codes
    (  35, 0xffa, 12), // '#'
    (  62, 0xffb, 12), // '>'
    // 13-bit codes
    (   0, 0x1ff8, 13),
    (  36, 0x1ff9, 13), // '$'
    (  64, 0x1ffa, 13), // '@'
    (  91, 0x1ffb, 13), // '['
    (  93, 0x1ffc, 13), // ']'
    ( 126, 0x1ffd, 13), // '~'
    // 14-bit codes
    (  94, 0x3ffc, 14), // '^'
    ( 125, 0x3ffd, 14), // '}'
    // 15-bit codes
    (  60, 0x7ffc, 15), // '<'
    (  96, 0x7ffd, 15), // '`'
    ( 123, 0x7ffe, 15), // '{'
    // 19-bit codes
    (  92, 0x7fff0, 19), // '\\'
    ( 195, 0x7fff1, 19),
    ( 208, 0x7fff2, 19),
    // 20-bit codes
    ( 128, 0xfffe6, 20),
    ( 130, 0xfffe7, 20),
    ( 131, 0xfffe8, 20),
    ( 162, 0xfffe9, 20),
    ( 184, 0xfffea, 20),
    ( 194, 0xfffeb, 20),
    ( 224, 0xfffec, 20),
    ( 226, 0xfffed, 20),
    // 21-bit codes
    ( 153, 0x1fffdc, 21),
    ( 161, 0x1fffdd, 21),
    ( 167, 0x1fffde, 21),
    ( 172, 0x1fffdf, 21),
    ( 176, 0x1fffe0, 21),
    ( 177, 0x1fffe1, 21),
    ( 179, 0x1fffe2, 21),
    ( 209, 0x1fffe3, 21),
    ( 216, 0x1fffe4, 21),
    ( 217, 0x1fffe5, 21),
    ( 227, 0x1fffe6, 21),
    ( 229, 0x1fffe7, 21),
    ( 230, 0x1fffe8, 21),
    // 22-bit codes
    ( 129, 0x3fffd2, 22),
    ( 132, 0x3fffd3, 22),
    ( 133, 0x3fffd4, 22),
    ( 134, 0x3fffd5, 22),
    ( 136, 0x3fffd6, 22),
    ( 146, 0x3fffd7, 22),
    ( 154, 0x3fffd8, 22),
    ( 156, 0x3fffd9, 22),
    ( 160, 0x3fffda, 22),
    ( 163, 0x3fffdb, 22),
    ( 164, 0x3fffdc, 22),
    ( 169, 0x3fffdd, 22),
    ( 170, 0x3fffde, 22),
    ( 173, 0x3fffdf, 22),
    ( 178, 0x3fffe0, 22),
    ( 181, 0x3fffe1, 22),
    ( 185, 0x3fffe2, 22),
    ( 186, 0x3fffe3, 22),
    ( 187, 0x3fffe4, 22),
    ( 189, 0x3fffe5, 22),
    ( 190, 0x3fffe6, 22),
    ( 196, 0x3fffe7, 22),
    ( 198, 0x3fffe8, 22),
    ( 228, 0x3fffe9, 22),
    ( 232, 0x3fffea, 22),
    ( 233, 0x3fffeb, 22),
    // 23-bit codes
    (   1, 0x7fffd8, 23),
    ( 135, 0x7fffd9, 23),
    ( 137, 0x7fffda, 23),
    ( 138, 0x7fffdb, 23),
    ( 139, 0x7fffdc, 23),
    ( 140, 0x7fffdd, 23),
    ( 141, 0x7fffde, 23),
    ( 143, 0x7fffdf, 23),
    ( 147, 0x7fffe0, 23),
    ( 149, 0x7fffe1, 23),
    ( 150, 0x7fffe2, 23),
    ( 151, 0x7fffe3, 23),
    ( 152, 0x7fffe4, 23),
    ( 155, 0x7fffe5, 23),
    ( 157, 0x7fffe6, 23),
    ( 158, 0x7fffe7, 23),
    ( 165, 0x7fffe8, 23),
    ( 166, 0x7fffe9, 23),
    ( 168, 0x7fffea, 23),
    ( 174, 0x7fffeb, 23),
    ( 175, 0x7fffec, 23),
    ( 180, 0x7fffed, 23),
    ( 182, 0x7fffee, 23),
    ( 183, 0x7fffef, 23),
    ( 188, 0x7ffff0, 23),
    ( 191, 0x7ffff1, 23),
    ( 197, 0x7ffff2, 23),
    ( 231, 0x7ffff3, 23),
    ( 239, 0x7ffff4, 23),
    // 24-bit codes
    (   9, 0xffffea, 24),
    ( 142, 0xffffeb, 24),
    ( 144, 0xffffec, 24),
    ( 145, 0xffffed, 24),
    ( 148, 0xffffee, 24),
    ( 159, 0xffffef, 24),
    ( 171, 0xfffff0, 24),
    ( 206, 0xfffff1, 24),
    ( 215, 0xfffff2, 24),
    ( 225, 0xfffff3, 24),
    ( 236, 0xfffff4, 24),
    ( 237, 0xfffff5, 24),
    // 25-bit codes
    ( 199, 0x1ffffec, 25),
    ( 207, 0x1ffffed, 25),
    ( 234, 0x1ffffee, 25),
    ( 235, 0x1ffffef, 25),
    // 26-bit codes
    ( 192, 0x3ffffdc, 26),
    ( 193, 0x3ffffdd, 26),
    ( 200, 0x3ffffde, 26),
    ( 201, 0x3ffffdf, 26),
    ( 202, 0x3ffffe0, 26),
    ( 205, 0x3ffffe1, 26),
    ( 210, 0x3ffffe2, 26),
    ( 213, 0x3ffffe3, 26),
    ( 218, 0x3ffffe4, 26),
    ( 219, 0x3ffffe5, 26),
    ( 238, 0x3ffffe6, 26),
    ( 240, 0x3ffffe7, 26),
    ( 242, 0x3ffffe8, 26),
    ( 243, 0x3ffffe9, 26),
    ( 255, 0x3ffffea, 26),
    // 27-bit codes
    ( 203, 0x7ffffd6, 27),
    ( 204, 0x7ffffd7, 27),
    ( 211, 0x7ffffd8, 27),
    ( 212, 0x7ffffd9, 27),
    ( 214, 0x7ffffda, 27),
    ( 221, 0x7ffffdb, 27),
    ( 222, 0x7ffffdc, 27),
    ( 223, 0x7ffffdd, 27),
    ( 241, 0x7ffffde, 27),
    ( 244, 0x7ffffdf, 27),
    ( 245, 0x7ffffe0, 27),
    ( 246, 0x7ffffe1, 27),
    ( 247, 0x7ffffe2, 27),
    ( 248, 0x7ffffe3, 27),
    ( 250, 0x7ffffe4, 27),
    ( 251, 0x7ffffe5, 27),
    ( 252, 0x7ffffe6, 27),
    ( 253, 0x7ffffe7, 27),
    ( 254, 0x7ffffe8, 27),
    // 28-bit codes
    (   2, 0xfffffe2, 28),
    (   3, 0xfffffe3, 28),
    (   4, 0xfffffe4, 28),
    (   5, 0xfffffe5, 28),
    (   6, 0xfffffe6, 28),
    (   7, 0xfffffe7, 28),
    (   8, 0xfffffe8, 28),
    (  11, 0xfffffe9, 28),
    (  12, 0xfffffea, 28),
    (  14, 0xfffffeb, 28),
    (  15, 0xfffffec, 28),
    (  16, 0xfffffed, 28),
    (  17, 0xfffffee, 28),
    (  18, 0xfffffef, 28),
    (  19, 0xffffff0, 28),
    (  20, 0xffffff1, 28),
    (  21, 0xffffff2, 28),
    (  23, 0xffffff3, 28),
    (  24, 0xffffff4, 28),
    (  25, 0xffffff5, 28),
    (  26, 0xffffff6, 28),
    (  27, 0xffffff7, 28),
    (  28, 0xffffff8, 28),
    (  29, 0xffffff9, 28),
    (  30, 0xffffffa, 28),
    (  31, 0xffffffb, 28),
    ( 127, 0xffffffc, 28),
    ( 220, 0xffffffd, 28),
    ( 249, 0xffffffe, 28),
    // 30-bit codes
    (  10, 0x3ffffffc, 30),
    (  13, 0x3ffffffd, 30),
    (  22, 0x3ffffffe, 30),
    // 30-bit EOS symbol (RFC 7541 Section 5.2). Must be rejected per spec:
    // an encoder MUST NOT emit this symbol; a decoder MUST treat it as an error.
    (256, 0x3fffffff, 30),
];

// ── H2 Connection State ─────────────────────────────────────────────

/// HTTP/2 connection-level state.
pub(crate) struct H2Connection {
    /// Active streams (stream_id -> stream state).
    pub streams: HashMap<u32, H2Stream>,
    /// HPACK decoder for incoming headers.
    pub decoder: HpackDecoder,
    /// HPACK encoder for outgoing headers.
    pub encoder: HpackEncoder,
    /// Connection-level send window (how much data we can send total).
    pub conn_send_window: i64,
    /// Connection-level receive window (how much data peer can send total).
    pub conn_recv_window: i64,
    /// Peer's settings.
    pub peer_settings: H2Settings,
    /// Our settings.
    pub local_settings: H2Settings,
    /// Last stream ID we've seen from the peer.
    pub last_peer_stream_id: u32,
    /// Whether GOAWAY has been sent.
    pub goaway_sent: bool,
    /// Total requests processed (for bounded shutdown).
    pub request_count: i64,
    /// NB6-38: Reusable payload buffer for frame reading (avoids per-frame heap alloc).
    #[allow(dead_code)]
    pub payload_buf: Vec<u8>,
    /// Buffered header block fragment during HEADERS/CONTINUATION sequence.
    /// Non-empty when we have received a HEADERS frame without END_HEADERS
    /// and are waiting for CONTINUATION frames.
    pub continuation_buf: Vec<u8>,
    /// Stream ID of the header block being assembled via CONTINUATION.
    /// 0 when no CONTINUATION sequence is in progress.
    pub continuation_stream_id: u32,
    /// Flags from the original HEADERS frame that started a CONTINUATION sequence.
    pub continuation_flags: u8,
}

/// HTTP/2 settings (RFC 9113 Section 6.5.1).
#[derive(Debug, Clone)]
pub(crate) struct H2Settings {
    pub header_table_size: u32,
    pub enable_push: bool,
    pub max_concurrent_streams: u32,
    pub initial_window_size: u32,
    pub max_frame_size: u32,
    pub max_header_list_size: u32,
}

impl Default for H2Settings {
    fn default() -> Self {
        H2Settings {
            header_table_size: DEFAULT_HEADER_TABLE_SIZE,
            enable_push: false, // Server never pushes in our implementation
            max_concurrent_streams: DEFAULT_MAX_CONCURRENT_STREAMS,
            initial_window_size: DEFAULT_INITIAL_WINDOW_SIZE,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            max_header_list_size: u32::MAX,
        }
    }
}

impl H2Connection {
    pub fn new() -> Self {
        H2Connection {
            streams: HashMap::new(),
            decoder: HpackDecoder::new(DEFAULT_HEADER_TABLE_SIZE),
            encoder: HpackEncoder::new(DEFAULT_HEADER_TABLE_SIZE),
            conn_send_window: DEFAULT_INITIAL_WINDOW_SIZE as i64,
            conn_recv_window: DEFAULT_INITIAL_WINDOW_SIZE as i64,
            peer_settings: H2Settings::default(),
            local_settings: H2Settings::default(),
            last_peer_stream_id: 0,
            goaway_sent: false,
            request_count: 0,
            // NB6-38: Reusable payload buffer, sized for max frame payload + header
            payload_buf: Vec::with_capacity(16_384 + 9),
            continuation_buf: Vec::new(),
            continuation_stream_id: 0,
            continuation_flags: 0,
        }
    }
}

// ── H2 Error Type ───────────────────────────────────────────────────

#[derive(Debug)]
pub(crate) enum H2Error {
    /// Connection-level protocol error. Send GOAWAY with the given error code.
    Connection(u32, String),
    /// Stream-level error. Send RST_STREAM with the given error code.
    Stream(u32, u32, String),
    /// HPACK compression error. Always a connection error.
    Compression(String),
    /// I/O error (connection lost).
    Io(io::Error),
}

impl From<io::Error> for H2Error {
    fn from(e: io::Error) -> Self {
        H2Error::Io(e)
    }
}

impl std::fmt::Display for H2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            H2Error::Connection(code, msg) => {
                write!(f, "h2 connection error (0x{:x}): {}", code, msg)
            }
            H2Error::Stream(stream_id, code, msg) => {
                write!(f, "h2 stream {} error (0x{:x}): {}", stream_id, code, msg)
            }
            H2Error::Compression(msg) => write!(f, "h2 compression error: {}", msg),
            H2Error::Io(e) => write!(f, "h2 I/O error: {}", e),
        }
    }
}

/// Validate that the decoded header list does not exceed the safety limit.
/// RFC 9113 Section 6.5.2: The value is based on the uncompressed size of
/// header fields, including the length of the name and value in octets
/// plus an overhead of 32 octets for each header field.
pub(crate) fn validate_header_list_size(headers: &[(String, String)]) -> Result<(), H2Error> {
    let mut total_size: usize = 0;
    for (name, value) in headers {
        total_size += name.len() + value.len() + 32;
        if total_size > MAX_DECODED_HEADER_LIST_SIZE {
            return Err(H2Error::Connection(
                ERROR_INTERNAL_ERROR,
                format!(
                    "decoded header list size {} exceeds safety limit {}",
                    total_size, MAX_DECODED_HEADER_LIST_SIZE
                ),
            ));
        }
    }
    Ok(())
}

// ── H2 Protocol Operations ──────────────────────────────────────────

/// Validate the HTTP/2 connection preface from the client.
/// Reads exactly 24 bytes and compares against the magic string.
pub(crate) fn validate_connection_preface<R: Read>(reader: &mut R) -> Result<(), H2Error> {
    let mut buf = [0u8; 24];
    reader.read_exact(&mut buf).map_err(|e| {
        H2Error::Connection(
            ERROR_PROTOCOL_ERROR,
            format!("failed to read connection preface: {}", e),
        )
    })?;
    if &buf != CONNECTION_PREFACE {
        return Err(H2Error::Connection(
            ERROR_PROTOCOL_ERROR,
            "invalid HTTP/2 connection preface".into(),
        ));
    }
    Ok(())
}

/// Read a single HTTP/2 frame (header + payload) from the stream.
/// Returns (FrameHeader, payload_bytes).
/// Note: Production hot path uses read_frame_reuse() instead (NB6-38).
/// This allocating variant is retained for unit tests.
#[allow(dead_code)]
pub(crate) fn read_frame<R: Read>(
    reader: &mut R,
    max_frame_size: u32,
) -> Result<(FrameHeader, Vec<u8>), H2Error> {
    let mut header_buf = [0u8; 9];
    reader.read_exact(&mut header_buf)?;
    let header = FrameHeader::parse(&header_buf);

    if header.length > max_frame_size {
        return Err(H2Error::Connection(
            ERROR_FRAME_SIZE_ERROR,
            format!(
                "frame length {} exceeds max_frame_size {}",
                header.length, max_frame_size
            ),
        ));
    }

    let mut payload = vec![0u8; header.length as usize];
    if header.length > 0 {
        reader.read_exact(&mut payload)?;
    }

    Ok((header, payload))
}

/// NB6-38: Read an HTTP/2 frame reusing the provided buffer to avoid per-frame heap alloc.
/// Used by h2_connection_loop() on the hot path instead of read_frame() which allocates per frame.
pub(crate) fn read_frame_reuse<'a, R: Read>(
    reader: &mut R,
    max_frame_size: u32,
    buf: &'a mut Vec<u8>,
) -> Result<(FrameHeader, &'a [u8]), H2Error> {
    let mut header_buf = [0u8; 9];
    reader.read_exact(&mut header_buf)?;
    let header = FrameHeader::parse(&header_buf);

    if header.length > max_frame_size {
        return Err(H2Error::Connection(
            ERROR_FRAME_SIZE_ERROR,
            format!(
                "frame length {} exceeds max_frame_size {}",
                header.length, max_frame_size
            ),
        ));
    }

    let len = header.length as usize;
    buf.resize(len, 0);
    if len > 0 {
        reader.read_exact(&mut buf[..len])?;
    }

    Ok((header, &buf[..len]))
}

/// Write an HTTP/2 frame to the stream (without flushing).
/// NB6-35: Removed per-frame flush(). Callers should use BufWriter and flush
/// once per logical response unit (e.g., after HEADERS+DATA sequence) to avoid
/// 5+ TLS records / syscalls per response.
pub(crate) fn write_frame<W: Write>(
    writer: &mut W,
    frame_type: u8,
    flags: u8,
    stream_id: u32,
    payload: &[u8],
) -> Result<(), H2Error> {
    let header = FrameHeader {
        length: payload.len() as u32,
        frame_type,
        flags,
        stream_id,
    };
    let header_bytes = header.serialize();
    writer.write_all(&header_bytes)?;
    if !payload.is_empty() {
        writer.write_all(payload)?;
    }
    Ok(())
}

/// Send a SETTINGS frame with our server settings.
pub(crate) fn send_settings<W: Write>(
    writer: &mut W,
    settings: &H2Settings,
) -> Result<(), H2Error> {
    let mut payload = Vec::with_capacity(36);

    // SETTINGS_MAX_CONCURRENT_STREAMS
    payload.extend_from_slice(&SETTINGS_MAX_CONCURRENT_STREAMS.to_be_bytes());
    payload.extend_from_slice(&settings.max_concurrent_streams.to_be_bytes());

    // SETTINGS_INITIAL_WINDOW_SIZE
    payload.extend_from_slice(&SETTINGS_INITIAL_WINDOW_SIZE.to_be_bytes());
    payload.extend_from_slice(&settings.initial_window_size.to_be_bytes());

    // SETTINGS_MAX_FRAME_SIZE
    payload.extend_from_slice(&SETTINGS_MAX_FRAME_SIZE.to_be_bytes());
    payload.extend_from_slice(&settings.max_frame_size.to_be_bytes());

    // SETTINGS_ENABLE_PUSH = 0 (server never pushes)
    payload.extend_from_slice(&SETTINGS_ENABLE_PUSH.to_be_bytes());
    payload.extend_from_slice(&0u32.to_be_bytes());

    write_frame(writer, FRAME_SETTINGS, 0, 0, &payload)
}

/// Send a SETTINGS ACK frame.
pub(crate) fn send_settings_ack<W: Write>(writer: &mut W) -> Result<(), H2Error> {
    write_frame(writer, FRAME_SETTINGS, FLAG_ACK, 0, &[])
}

/// Send a GOAWAY frame.
pub(crate) fn send_goaway<W: Write>(
    writer: &mut W,
    last_stream_id: u32,
    error_code: u32,
    debug_data: &[u8],
) -> Result<(), H2Error> {
    let mut payload = Vec::with_capacity(8 + debug_data.len());
    payload.extend_from_slice(&last_stream_id.to_be_bytes());
    payload.extend_from_slice(&error_code.to_be_bytes());
    payload.extend_from_slice(debug_data);
    write_frame(writer, FRAME_GOAWAY, 0, 0, &payload)
}

/// Send a RST_STREAM frame.
pub(crate) fn send_rst_stream<W: Write>(
    writer: &mut W,
    stream_id: u32,
    error_code: u32,
) -> Result<(), H2Error> {
    let payload = error_code.to_be_bytes();
    write_frame(writer, FRAME_RST_STREAM, 0, stream_id, &payload)
}

/// Send a WINDOW_UPDATE frame.
pub(crate) fn send_window_update<W: Write>(
    writer: &mut W,
    stream_id: u32,
    increment: u32,
) -> Result<(), H2Error> {
    if increment == 0 || increment > 0x7FFFFFFF {
        return Err(H2Error::Connection(
            ERROR_PROTOCOL_ERROR,
            format!("invalid window update increment: {}", increment),
        ));
    }
    let payload = increment.to_be_bytes();
    write_frame(writer, FRAME_WINDOW_UPDATE, 0, stream_id, &payload)
}

/// Send a PING response (ACK).
pub(crate) fn send_ping_ack<W: Write>(writer: &mut W, opaque_data: &[u8]) -> Result<(), H2Error> {
    write_frame(writer, FRAME_PING, FLAG_ACK, 0, opaque_data)
}

/// Send response HEADERS frame(s) with HPACK-encoded headers.
///
/// If the encoded header block exceeds `peer_max_frame_size`, the block is
/// split into an initial HEADERS frame (without END_HEADERS) followed by
/// one or more CONTINUATION frames, with END_HEADERS on the final frame.
/// This ensures we never send a frame larger than the peer's SETTINGS_MAX_FRAME_SIZE.
pub(crate) fn send_response_headers<W: Write>(
    writer: &mut W,
    encoder: &mut HpackEncoder,
    stream_id: u32,
    status: u16,
    headers: &[(String, String)],
    end_stream: bool,
    peer_max_frame_size: u32,
) -> Result<(), H2Error> {
    // Build pseudo-header + regular headers for HPACK encoding
    let mut all_headers = Vec::with_capacity(headers.len() + 1);
    all_headers.push((":status".to_string(), status.to_string()));
    for (name, value) in headers {
        all_headers.push((name.to_lowercase(), value.clone()));
    }

    let encoded = encoder.encode(&all_headers);
    let max_size = peer_max_frame_size as usize;

    if encoded.len() <= max_size {
        // Fits in a single HEADERS frame.
        let mut flags = FLAG_END_HEADERS;
        if end_stream {
            flags |= FLAG_END_STREAM;
        }
        write_frame(writer, FRAME_HEADERS, flags, stream_id, &encoded)
    } else {
        // Split into HEADERS + CONTINUATION* frames.
        // First frame: HEADERS with up to max_size bytes, no END_HEADERS.
        let mut flags = 0u8;
        if end_stream {
            flags |= FLAG_END_STREAM;
        }
        write_frame(
            writer,
            FRAME_HEADERS,
            flags,
            stream_id,
            &encoded[..max_size],
        )?;

        // Remaining fragments as CONTINUATION frames.
        let mut offset = max_size;
        while offset < encoded.len() {
            let chunk_end = (offset + max_size).min(encoded.len());
            let is_last = chunk_end == encoded.len();
            let cont_flags = if is_last { FLAG_END_HEADERS } else { 0 };
            write_frame(
                writer,
                FRAME_CONTINUATION,
                cont_flags,
                stream_id,
                &encoded[offset..chunk_end],
            )?;
            offset = chunk_end;
        }

        Ok(())
    }
}

/// Send response DATA frame(s), respecting max frame size **and** flow control windows.
///
/// `conn_send_window` and `stream_send_window` are mutable references so
/// we can debit them as frames are written. Each chunk size is
/// `min(max_frame_size, conn_send_window, stream_send_window)`.
///
/// Returns the total bytes sent.
pub(crate) fn send_response_data<W: Write>(
    writer: &mut W,
    stream_id: u32,
    data: &[u8],
    end_stream: bool,
    max_frame_size: u32,
    conn_send_window: &mut i64,
    stream_send_window: &mut i64,
) -> Result<usize, H2Error> {
    let total = data.len();

    if total == 0 {
        // Empty DATA frame with END_STREAM
        if end_stream {
            write_frame(writer, FRAME_DATA, FLAG_END_STREAM, stream_id, &[])?;
        }
        return Ok(0);
    }

    let mut sent = 0;
    while sent < total {
        // Determine the maximum chunk we can send, bounded by:
        // 1. peer's max frame size
        // 2. connection-level send window
        // 3. stream-level send window
        let remaining = total - sent;
        let frame_limit = max_frame_size as usize;
        let conn_limit = if *conn_send_window > 0 {
            *conn_send_window as usize
        } else {
            0
        };
        let stream_limit = if *stream_send_window > 0 {
            *stream_send_window as usize
        } else {
            0
        };

        let chunk_size = remaining.min(frame_limit).min(conn_limit).min(stream_limit);

        if chunk_size == 0 {
            // Flow control window exhausted — cannot send more data.
            // In a full async implementation we would wait for WINDOW_UPDATE.
            // In this blocking interpreter model, return what we've sent so far.
            // The caller can decide how to handle the partial send.
            return Err(H2Error::Connection(
                ERROR_FLOW_CONTROL_ERROR,
                format!(
                    "send window exhausted: conn={}, stream={}, remaining={}",
                    conn_send_window, stream_send_window, remaining
                ),
            ));
        }

        let chunk_end = sent + chunk_size;
        let is_last = chunk_end == total;
        let flags = if is_last && end_stream {
            FLAG_END_STREAM
        } else {
            0
        };
        write_frame(writer, FRAME_DATA, flags, stream_id, &data[sent..chunk_end])?;

        // Debit both windows
        let debit = chunk_size as i64;
        *conn_send_window -= debit;
        *stream_send_window -= debit;

        sent = chunk_end;
    }

    Ok(total)
}

/// Parse a SETTINGS frame payload into individual settings.
pub(crate) fn parse_settings(payload: &[u8]) -> Result<Vec<(u16, u32)>, H2Error> {
    #[allow(clippy::manual_is_multiple_of)]
    if payload.len() % 6 != 0 {
        return Err(H2Error::Connection(
            ERROR_FRAME_SIZE_ERROR,
            format!(
                "SETTINGS frame payload length {} is not a multiple of 6",
                payload.len()
            ),
        ));
    }

    let mut settings = Vec::new();
    let mut pos = 0;
    while pos + 6 <= payload.len() {
        let id = u16::from_be_bytes([payload[pos], payload[pos + 1]]);
        let value = u32::from_be_bytes([
            payload[pos + 2],
            payload[pos + 3],
            payload[pos + 4],
            payload[pos + 5],
        ]);
        settings.push((id, value));
        pos += 6;
    }

    Ok(settings)
}

/// Apply received settings to the connection state.
pub(crate) fn apply_peer_settings(
    conn: &mut H2Connection,
    settings: &[(u16, u32)],
) -> Result<(), H2Error> {
    for &(id, value) in settings {
        match id {
            SETTINGS_HEADER_TABLE_SIZE => {
                conn.peer_settings.header_table_size = value;
                // Update encoder's dynamic table size
                conn.encoder.update_max_size(value as usize);
            }
            SETTINGS_ENABLE_PUSH => {
                if value > 1 {
                    return Err(H2Error::Connection(
                        ERROR_PROTOCOL_ERROR,
                        format!("SETTINGS_ENABLE_PUSH must be 0 or 1, got {}", value),
                    ));
                }
                conn.peer_settings.enable_push = value == 1;
            }
            SETTINGS_MAX_CONCURRENT_STREAMS => {
                conn.peer_settings.max_concurrent_streams = value;
            }
            SETTINGS_INITIAL_WINDOW_SIZE => {
                if value > 0x7FFFFFFF {
                    return Err(H2Error::Connection(
                        ERROR_FLOW_CONTROL_ERROR,
                        format!(
                            "SETTINGS_INITIAL_WINDOW_SIZE {} exceeds maximum (2^31 - 1)",
                            value
                        ),
                    ));
                }
                let old = conn.peer_settings.initial_window_size;
                conn.peer_settings.initial_window_size = value;

                // Adjust all existing stream send windows
                let delta = value as i64 - old as i64;
                for stream in conn.streams.values_mut() {
                    stream.send_window += delta;
                }
            }
            SETTINGS_MAX_FRAME_SIZE => {
                if !(DEFAULT_MAX_FRAME_SIZE..=MAX_MAX_FRAME_SIZE).contains(&value) {
                    return Err(H2Error::Connection(
                        ERROR_PROTOCOL_ERROR,
                        format!(
                            "SETTINGS_MAX_FRAME_SIZE {} out of range [{}, {}]",
                            value, DEFAULT_MAX_FRAME_SIZE, MAX_MAX_FRAME_SIZE
                        ),
                    ));
                }
                conn.peer_settings.max_frame_size = value;
            }
            SETTINGS_MAX_HEADER_LIST_SIZE => {
                conn.peer_settings.max_header_list_size = value;
            }
            _ => {
                // Unknown settings MUST be ignored (RFC 9113 Section 6.5.2).
            }
        }
    }
    Ok(())
}

/// A completed HTTP/2 request: (stream_id, headers, body).
pub(crate) type CompletedRequest = (u32, Vec<(String, String)>, Vec<u8>);

/// Process one frame from the client and update connection/stream state.
/// Returns a completed request (stream_id, headers, body) if one is ready.
pub(crate) fn process_frame(
    conn: &mut H2Connection,
    header: &FrameHeader,
    payload: &[u8],
) -> Result<Option<CompletedRequest>, H2Error> {
    // RFC 9113 Section 6.2: While a CONTINUATION sequence is in progress,
    // the only frame type that may be received is CONTINUATION on the same
    // stream. Any other frame type is a connection error of type PROTOCOL_ERROR.
    if conn.continuation_stream_id != 0 && header.frame_type != FRAME_CONTINUATION {
        return Err(H2Error::Connection(
            ERROR_PROTOCOL_ERROR,
            format!(
                "expected CONTINUATION for stream {}, got frame type 0x{:02x}",
                conn.continuation_stream_id, header.frame_type
            ),
        ));
    }

    match header.frame_type {
        FRAME_SETTINGS => {
            if header.stream_id != 0 {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "SETTINGS frame on non-zero stream".into(),
                ));
            }
            if header.flags & FLAG_ACK != 0 {
                // Settings ACK — no payload expected
                if header.length != 0 {
                    return Err(H2Error::Connection(
                        ERROR_FRAME_SIZE_ERROR,
                        "SETTINGS ACK with non-zero length".into(),
                    ));
                }
                return Ok(None);
            }
            let settings = parse_settings(payload)?;
            apply_peer_settings(conn, &settings)?;
            // We need to send SETTINGS ACK, but the caller handles writing
            Ok(None)
        }

        FRAME_HEADERS => {
            // RFC 9113 Section 6.2: HEADERS frame.
            // If a CONTINUATION sequence is already in progress, receiving a
            // HEADERS frame is a protocol error.
            if conn.continuation_stream_id != 0 {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "received HEADERS while CONTINUATION sequence in progress".into(),
                ));
            }

            let stream_id = header.stream_id;
            if stream_id == 0 {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "HEADERS frame on stream 0".into(),
                ));
            }
            #[allow(clippy::manual_is_multiple_of)]
            if stream_id % 2 == 0 {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    format!("HEADERS on even stream id {}", stream_id),
                ));
            }

            // New stream — validate stream ID ordering
            if stream_id <= conn.last_peer_stream_id {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    format!(
                        "stream id {} <= last peer stream id {}",
                        stream_id, conn.last_peer_stream_id
                    ),
                ));
            }
            conn.last_peer_stream_id = stream_id;

            // Check max concurrent streams
            let active_count = conn
                .streams
                .values()
                .filter(|s| s.state != StreamState::Closed)
                .count() as u32;
            if active_count >= conn.local_settings.max_concurrent_streams {
                return Err(H2Error::Stream(
                    stream_id,
                    ERROR_PROTOCOL_ERROR,
                    "max concurrent streams exceeded".into(),
                ));
            }

            // Strip padding if PADDED flag is set
            let mut offset = 0;
            let mut pad_len = 0;
            if header.flags & FLAG_PADDED != 0 {
                if payload.is_empty() {
                    return Err(H2Error::Connection(
                        ERROR_PROTOCOL_ERROR,
                        "HEADERS PADDED flag set but no padding length byte".into(),
                    ));
                }
                pad_len = payload[0] as usize;
                offset = 1;
            }

            // Skip PRIORITY fields if present
            if header.flags & FLAG_PRIORITY != 0 {
                offset += 5; // 4 bytes stream dependency + 1 byte weight
            }

            if offset + pad_len > payload.len() {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "HEADERS padding length exceeds frame payload".into(),
                ));
            }

            let header_block = &payload[offset..payload.len() - pad_len];

            let end_headers = header.flags & FLAG_END_HEADERS != 0;

            if !end_headers {
                // CONTINUATION sequence: buffer the fragment and wait for
                // CONTINUATION frames until END_HEADERS is received.
                if header_block.len() > MAX_CONTINUATION_BUFFER_SIZE {
                    return Err(H2Error::Connection(
                        ERROR_INTERNAL_ERROR,
                        format!(
                            "header block fragment {} bytes exceeds safety limit {}",
                            header_block.len(),
                            MAX_CONTINUATION_BUFFER_SIZE
                        ),
                    ));
                }
                conn.continuation_buf = header_block.to_vec();
                conn.continuation_stream_id = stream_id;
                // Preserve END_STREAM from the original HEADERS (it applies
                // to the logical header block, not individual frames).
                conn.continuation_flags = header.flags;

                // Create the stream entry now so DATA-after-CONTINUATION works.
                let stream = H2Stream::new(conn.peer_settings.initial_window_size);
                conn.streams.insert(stream_id, stream);
                return Ok(None);
            }

            // END_HEADERS set — decode immediately.
            let headers = conn
                .decoder
                .decode(header_block)
                .map_err(|e| H2Error::Connection(ERROR_COMPRESSION_ERROR, format!("{}", e)))?;

            // Safety: enforce header list size limit (HPACK bomb protection).
            validate_header_list_size(&headers)?;

            let end_stream = header.flags & FLAG_END_STREAM != 0;

            let mut stream = H2Stream::new(conn.peer_settings.initial_window_size);
            stream.request_headers = headers;

            if end_stream {
                // Request is complete (no body)
                stream.state = StreamState::HalfClosedRemote;
                let headers = std::mem::take(&mut stream.request_headers);
                let body = std::mem::take(&mut stream.request_body);
                conn.streams.insert(stream_id, stream);
                return Ok(Some((stream_id, headers, body)));
            }

            // Expect DATA frames for body
            stream.state = StreamState::HalfClosedRemote; // We'll accumulate body in DATA
            conn.streams.insert(stream_id, stream);
            Ok(None)
        }

        FRAME_DATA => {
            let stream_id = header.stream_id;
            if stream_id == 0 {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "DATA frame on stream 0".into(),
                ));
            }

            let stream = conn.streams.get_mut(&stream_id).ok_or_else(|| {
                H2Error::Connection(
                    ERROR_STREAM_CLOSED,
                    format!("DATA on unknown stream {}", stream_id),
                )
            })?;

            // Strip padding
            let mut offset = 0;
            let mut pad_len = 0;
            if header.flags & FLAG_PADDED != 0 {
                if payload.is_empty() {
                    return Err(H2Error::Connection(
                        ERROR_PROTOCOL_ERROR,
                        "DATA PADDED flag set but no padding length byte".into(),
                    ));
                }
                pad_len = payload[0] as usize;
                offset = 1;
            }

            if offset + pad_len > payload.len() {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "DATA padding length exceeds frame payload".into(),
                ));
            }

            let data = &payload[offset..payload.len() - pad_len];

            // Flow control: enforce receive windows before debiting.
            // Per RFC 9113 Section 6.9.1, the entire frame payload (including
            // padding) counts against the flow-control window.
            let data_len = header.length as i64;

            // Connection-level check first (GOAWAY on violation)
            if data_len > conn.conn_recv_window {
                return Err(H2Error::Connection(
                    ERROR_FLOW_CONTROL_ERROR,
                    format!(
                        "DATA frame length {} exceeds connection recv window {}",
                        data_len, conn.conn_recv_window
                    ),
                ));
            }

            // Stream-level check (RST_STREAM on violation)
            if data_len > stream.recv_window {
                return Err(H2Error::Stream(
                    stream_id,
                    ERROR_FLOW_CONTROL_ERROR,
                    format!(
                        "DATA frame length {} exceeds stream {} recv window {}",
                        data_len, stream_id, stream.recv_window
                    ),
                ));
            }

            conn.conn_recv_window -= data_len;
            stream.recv_window -= data_len;

            // Accumulate body data
            stream.request_body.extend_from_slice(data);

            let end_stream = header.flags & FLAG_END_STREAM != 0;
            if end_stream {
                let headers = std::mem::take(&mut stream.request_headers);
                let body = std::mem::take(&mut stream.request_body);
                return Ok(Some((stream_id, headers, body)));
            }

            Ok(None)
        }

        FRAME_WINDOW_UPDATE => {
            if header.length != 4 {
                return Err(H2Error::Connection(
                    ERROR_FRAME_SIZE_ERROR,
                    format!("WINDOW_UPDATE must be 4 bytes, got {}", header.length),
                ));
            }

            let increment =
                u32::from_be_bytes([payload[0] & 0x7F, payload[1], payload[2], payload[3]]);

            if increment == 0 {
                if header.stream_id == 0 {
                    return Err(H2Error::Connection(
                        ERROR_PROTOCOL_ERROR,
                        "WINDOW_UPDATE with zero increment on connection".into(),
                    ));
                } else {
                    return Err(H2Error::Stream(
                        header.stream_id,
                        ERROR_PROTOCOL_ERROR,
                        "WINDOW_UPDATE with zero increment".into(),
                    ));
                }
            }

            if header.stream_id == 0 {
                // RFC 9113 Section 6.9.1: A change to SETTINGS_INITIAL_WINDOW_SIZE can cause
                // the available space in a flow-control window to become negative. A sender
                // MUST NOT allow a flow-control window to exceed 2^31-1 octets.
                let new_window = conn.conn_send_window + increment as i64;
                if new_window > MAX_FLOW_CONTROL_WINDOW {
                    return Err(H2Error::Connection(
                        ERROR_FLOW_CONTROL_ERROR,
                        format!(
                            "WINDOW_UPDATE would overflow connection send window: {} + {} > 2^31-1",
                            conn.conn_send_window, increment
                        ),
                    ));
                }
                conn.conn_send_window = new_window;
            } else if let Some(stream) = conn.streams.get_mut(&header.stream_id) {
                let new_window = stream.send_window + increment as i64;
                if new_window > MAX_FLOW_CONTROL_WINDOW {
                    return Err(H2Error::Stream(
                        header.stream_id,
                        ERROR_FLOW_CONTROL_ERROR,
                        format!(
                            "WINDOW_UPDATE would overflow stream {} send window: {} + {} > 2^31-1",
                            header.stream_id, stream.send_window, increment
                        ),
                    ));
                }
                stream.send_window = new_window;
            }
            // If stream not found, ignore (RFC 9113 Section 6.9: WINDOW_UPDATE on idle stream)

            Ok(None)
        }

        FRAME_PING => {
            if header.stream_id != 0 {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "PING on non-zero stream".into(),
                ));
            }
            if header.length != 8 {
                return Err(H2Error::Connection(
                    ERROR_FRAME_SIZE_ERROR,
                    format!("PING frame must be 8 bytes, got {}", header.length),
                ));
            }
            // ACK flag: this is a ping response, ignore
            // No ACK: we need to send a ping response (handled by caller)
            Ok(None)
        }

        FRAME_GOAWAY => {
            // Client is shutting down. We don't need to do anything special
            // other than stop accepting new streams.
            // The caller should handle graceful drain.
            Ok(None)
        }

        FRAME_RST_STREAM => {
            if header.stream_id == 0 {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "RST_STREAM on stream 0".into(),
                ));
            }
            if header.length != 4 {
                return Err(H2Error::Connection(
                    ERROR_FRAME_SIZE_ERROR,
                    format!("RST_STREAM must be 4 bytes, got {}", header.length),
                ));
            }
            // Mark stream as closed
            if let Some(stream) = conn.streams.get_mut(&header.stream_id) {
                stream.state = StreamState::Closed;
            }
            Ok(None)
        }

        FRAME_PRIORITY => {
            // Priority is advisory; we ignore it.
            if header.length != 5 {
                return Err(H2Error::Connection(
                    ERROR_FRAME_SIZE_ERROR,
                    format!("PRIORITY must be 5 bytes, got {}", header.length),
                ));
            }
            Ok(None)
        }

        FRAME_PUSH_PROMISE => {
            // Server push is out of scope; treat as protocol error from client.
            Err(H2Error::Connection(
                ERROR_PROTOCOL_ERROR,
                "PUSH_PROMISE from client is not allowed".into(),
            ))
        }

        FRAME_CONTINUATION => {
            // RFC 9113 Section 6.10: CONTINUATION must follow a HEADERS
            // (or CONTINUATION) on the same stream, before END_HEADERS.
            if conn.continuation_stream_id == 0 {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    "unexpected CONTINUATION frame (no header block in progress)".into(),
                ));
            }
            if header.stream_id != conn.continuation_stream_id {
                return Err(H2Error::Connection(
                    ERROR_PROTOCOL_ERROR,
                    format!(
                        "CONTINUATION on stream {} but expected stream {}",
                        header.stream_id, conn.continuation_stream_id
                    ),
                ));
            }

            // Append this fragment to the buffer.
            // Safety limit: prevent HPACK bomb / memory exhaustion.
            if conn.continuation_buf.len() + payload.len() > MAX_CONTINUATION_BUFFER_SIZE {
                conn.continuation_stream_id = 0;
                conn.continuation_flags = 0;
                conn.continuation_buf.clear();
                return Err(H2Error::Connection(
                    ERROR_INTERNAL_ERROR,
                    format!(
                        "CONTINUATION accumulated header block {} + {} bytes exceeds safety limit {}",
                        conn.continuation_buf.len(),
                        payload.len(),
                        MAX_CONTINUATION_BUFFER_SIZE
                    ),
                ));
            }
            conn.continuation_buf.extend_from_slice(payload);

            let end_headers = header.flags & FLAG_END_HEADERS != 0;
            if !end_headers {
                // More CONTINUATION frames expected.
                return Ok(None);
            }

            // END_HEADERS received — decode the complete header block.
            let stream_id = conn.continuation_stream_id;
            let original_flags = conn.continuation_flags;
            let header_block = std::mem::take(&mut conn.continuation_buf);
            conn.continuation_stream_id = 0;
            conn.continuation_flags = 0;

            let headers = conn
                .decoder
                .decode(&header_block)
                .map_err(|e| H2Error::Connection(ERROR_COMPRESSION_ERROR, format!("{}", e)))?;

            // Safety: enforce header list size limit (HPACK bomb protection).
            validate_header_list_size(&headers)?;

            let end_stream = original_flags & FLAG_END_STREAM != 0;

            let stream = conn.streams.get_mut(&stream_id).ok_or_else(|| {
                H2Error::Connection(
                    ERROR_INTERNAL_ERROR,
                    format!("stream {} not found after CONTINUATION", stream_id),
                )
            })?;
            stream.request_headers = headers;

            if end_stream {
                stream.state = StreamState::HalfClosedRemote;
                let headers = std::mem::take(&mut stream.request_headers);
                let body = std::mem::take(&mut stream.request_body);
                return Ok(Some((stream_id, headers, body)));
            }

            stream.state = StreamState::HalfClosedRemote;
            Ok(None)
        }

        _ => {
            // Unknown frame types MUST be ignored (RFC 9113 Section 4.1).
            Ok(None)
        }
    }
}

/// Extracted request fields: (method, path, authority, regular_headers).
pub(crate) type RequestFields = (String, String, String, Vec<(String, String)>);

/// Extract HTTP/1.1-like request fields from h2 pseudo-headers.
/// Returns (method, path, authority, headers_without_pseudo).
///
/// Validates per RFC 9113 Section 8.3:
/// - Pseudo-headers MUST appear before all regular headers.
/// - `:method`, `:path`, and `:scheme` are required for requests.
///
/// NB6-36: Accept actual stream_id to embed in errors instead of hardcoded 0.
/// Backward-compat wrapper with stream_id=0 for unit tests.
#[allow(dead_code)]
pub(crate) fn extract_request_fields(
    headers: &[(String, String)],
) -> Result<RequestFields, H2Error> {
    extract_request_fields_with_stream_id(headers, 0)
}

/// Extract request pseudo-headers and regular headers from a decoded header list.
/// `stream_id` is embedded in any error for correct RST_STREAM targeting.
pub(crate) fn extract_request_fields_with_stream_id(
    headers: &[(String, String)],
    stream_id: u32,
) -> Result<RequestFields, H2Error> {
    let mut method = None;
    let mut path = None;
    let mut authority = None;
    let mut scheme = None;
    let mut regular_headers = Vec::new();
    let mut saw_regular_header = false;

    for (name, value) in headers {
        if name.starts_with(':') {
            // RFC 9113 Section 8.3: Pseudo-header fields MUST NOT appear
            // in a header block after a regular header field.
            if saw_regular_header {
                return Err(H2Error::Stream(
                    stream_id,
                    ERROR_PROTOCOL_ERROR,
                    format!(
                        "pseudo-header {} after regular header (ordering violation)",
                        name
                    ),
                ));
            }
            match name.as_str() {
                ":method" => {
                    // RFC 9113 Section 8.3.1: Each pseudo-header MUST NOT appear
                    // more than once in a header block.
                    if method.is_some() {
                        return Err(H2Error::Stream(
                            stream_id,
                            ERROR_PROTOCOL_ERROR,
                            "duplicate :method pseudo-header".into(),
                        ));
                    }
                    method = Some(value.clone());
                }
                ":path" => {
                    if path.is_some() {
                        return Err(H2Error::Stream(
                            stream_id,
                            ERROR_PROTOCOL_ERROR,
                            "duplicate :path pseudo-header".into(),
                        ));
                    }
                    path = Some(value.clone());
                }
                ":authority" => {
                    if authority.is_some() {
                        return Err(H2Error::Stream(
                            stream_id,
                            ERROR_PROTOCOL_ERROR,
                            "duplicate :authority pseudo-header".into(),
                        ));
                    }
                    authority = Some(value.clone());
                }
                ":scheme" => {
                    if scheme.is_some() {
                        return Err(H2Error::Stream(
                            stream_id,
                            ERROR_PROTOCOL_ERROR,
                            "duplicate :scheme pseudo-header".into(),
                        ));
                    }
                    scheme = Some(value.clone());
                }
                _ => {
                    return Err(H2Error::Stream(
                        stream_id,
                        ERROR_PROTOCOL_ERROR,
                        format!("unknown pseudo-header: {}", name),
                    ));
                }
            }
        } else {
            saw_regular_header = true;
            regular_headers.push((name.clone(), value.clone()));
        }
    }

    let method = method.ok_or_else(|| {
        H2Error::Stream(
            stream_id,
            ERROR_PROTOCOL_ERROR,
            "missing :method pseudo-header".into(),
        )
    })?;
    // RFC 9113 Section 8.3.1: :method value MUST NOT be empty.
    if method.is_empty() {
        return Err(H2Error::Stream(
            stream_id,
            ERROR_PROTOCOL_ERROR,
            "empty :method pseudo-header value".into(),
        ));
    }

    let path = path.ok_or_else(|| {
        H2Error::Stream(
            stream_id,
            ERROR_PROTOCOL_ERROR,
            "missing :path pseudo-header".into(),
        )
    })?;
    // RFC 9113 Section 8.3.1: :path value MUST NOT be empty for non-CONNECT requests.
    if path.is_empty() {
        return Err(H2Error::Stream(
            stream_id,
            ERROR_PROTOCOL_ERROR,
            "empty :path pseudo-header value".into(),
        ));
    }

    // RFC 9113 Section 8.3.1: :scheme is required for request pseudo-headers.
    let _scheme = scheme.ok_or_else(|| {
        H2Error::Stream(
            stream_id,
            ERROR_PROTOCOL_ERROR,
            "missing :scheme pseudo-header".into(),
        )
    })?;

    let authority = authority.unwrap_or_default();

    Ok((method, path, authority, regular_headers))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Frame header parsing ─────────────────────────────────────

    #[test]
    fn test_frame_header_roundtrip() {
        let original = FrameHeader {
            length: 16384,
            frame_type: FRAME_DATA,
            flags: FLAG_END_STREAM,
            stream_id: 1,
        };
        let serialized = original.serialize();
        let parsed = FrameHeader::parse(&serialized);
        assert_eq!(parsed.length, 16384);
        assert_eq!(parsed.frame_type, FRAME_DATA);
        assert_eq!(parsed.flags, FLAG_END_STREAM);
        assert_eq!(parsed.stream_id, 1);
    }

    #[test]
    fn test_frame_header_zero_stream() {
        let header = FrameHeader {
            length: 0,
            frame_type: FRAME_SETTINGS,
            flags: 0,
            stream_id: 0,
        };
        let serialized = header.serialize();
        let parsed = FrameHeader::parse(&serialized);
        assert_eq!(parsed.stream_id, 0);
        assert_eq!(parsed.frame_type, FRAME_SETTINGS);
    }

    #[test]
    fn test_frame_header_max_length() {
        let header = FrameHeader {
            length: 0x00FFFFFF, // 24-bit max
            frame_type: FRAME_DATA,
            flags: 0,
            stream_id: 42,
        };
        let serialized = header.serialize();
        let parsed = FrameHeader::parse(&serialized);
        assert_eq!(parsed.length, 0x00FFFFFF);
    }

    #[test]
    fn test_frame_header_r_bit_masked() {
        // Stream ID with R bit set should be masked off
        let mut buf = [0u8; 9];
        buf[5] = 0xFF; // R bit + high bits of stream_id
        buf[6] = 0xFF;
        buf[7] = 0xFF;
        buf[8] = 0xFF;
        let parsed = FrameHeader::parse(&buf);
        assert_eq!(parsed.stream_id, 0x7FFFFFFF);
    }

    // ── HPACK integer coding ────────────────────────────────────

    #[test]
    fn test_hpack_integer_small() {
        let mut buf = Vec::new();
        encode_integer(&mut buf, 10, 5, 0x00);
        assert_eq!(&buf, &[10u8]);
        let (val, consumed) = decode_integer(&buf, 5).unwrap();
        assert_eq!(val, 10);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_hpack_integer_exactly_prefix() {
        // 30 < 31 (mask for 5 bits), so single byte
        let mut buf = Vec::new();
        encode_integer(&mut buf, 30, 5, 0x00);
        assert_eq!(&buf, &[30u8]);
        let (val, consumed) = decode_integer(&buf, 5).unwrap();
        assert_eq!(val, 30);
        assert_eq!(consumed, 1);

        // 31 == mask, so multi-byte encoding starts
        let mut buf2 = Vec::new();
        encode_integer(&mut buf2, 31, 5, 0x00);
        assert!(buf2.len() > 1);
        let (val2, _) = decode_integer(&buf2, 5).unwrap();
        assert_eq!(val2, 31);
    }

    #[test]
    fn test_hpack_integer_multi_byte() {
        let mut buf = Vec::new();
        encode_integer(&mut buf, 1337, 5, 0x00);
        let (val, consumed) = decode_integer(&buf, 5).unwrap();
        assert_eq!(val, 1337);
        assert!(consumed > 1);
    }

    // ── HPACK string coding ─────────────────────────────────────

    #[test]
    fn test_hpack_string_roundtrip() {
        let mut buf = Vec::new();
        encode_string(&mut buf, "hello");
        let (decoded, consumed) = decode_string(&buf).unwrap();
        assert_eq!(decoded, "hello");
        assert_eq!(consumed, buf.len());
    }

    #[test]
    fn test_hpack_string_empty() {
        let mut buf = Vec::new();
        encode_string(&mut buf, "");
        let (decoded, consumed) = decode_string(&buf).unwrap();
        assert_eq!(decoded, "");
        assert_eq!(consumed, 1); // just the length byte (0)
    }

    // ── HPACK encoder/decoder ───────────────────────────────────

    #[test]
    fn test_hpack_encode_decode_static_exact() {
        let mut encoder = HpackEncoder::new(4096);
        let mut decoder = HpackDecoder::new(4096);

        let headers = vec![(":status".to_string(), "200".to_string())];
        let encoded = encoder.encode(&headers);
        let decoded = decoder.decode(&encoded).unwrap();
        assert_eq!(decoded, headers);
    }

    #[test]
    fn test_hpack_encode_decode_static_name() {
        let mut encoder = HpackEncoder::new(4096);
        let mut decoder = HpackDecoder::new(4096);

        let headers = vec![("content-type".to_string(), "text/html".to_string())];
        let encoded = encoder.encode(&headers);
        let decoded = decoder.decode(&encoded).unwrap();
        assert_eq!(decoded, headers);
    }

    #[test]
    fn test_hpack_encode_decode_new_name() {
        let mut encoder = HpackEncoder::new(4096);
        let mut decoder = HpackDecoder::new(4096);

        let headers = vec![("x-custom".to_string(), "value123".to_string())];
        let encoded = encoder.encode(&headers);
        let decoded = decoder.decode(&encoded).unwrap();
        assert_eq!(decoded, headers);
    }

    #[test]
    fn test_hpack_dynamic_table_reuse() {
        let mut encoder = HpackEncoder::new(4096);
        let mut decoder = HpackDecoder::new(4096);

        // First request: new header, gets added to dynamic table
        let headers1 = vec![("x-custom".to_string(), "hello".to_string())];
        let encoded1 = encoder.encode(&headers1);
        let decoded1 = decoder.decode(&encoded1).unwrap();
        assert_eq!(decoded1, headers1);

        // Second request: same header should be more compact (indexed)
        let encoded2 = encoder.encode(&headers1);
        // The second encoding should use the dynamic table entry
        let decoded2 = decoder.decode(&encoded2).unwrap();
        assert_eq!(decoded2, headers1);
    }

    #[test]
    fn test_hpack_multiple_headers() {
        let mut encoder = HpackEncoder::new(4096);
        let mut decoder = HpackDecoder::new(4096);

        let headers = vec![
            (":status".to_string(), "200".to_string()),
            ("content-type".to_string(), "text/plain".to_string()),
            ("content-length".to_string(), "5".to_string()),
            ("x-custom".to_string(), "test".to_string()),
        ];
        let encoded = encoder.encode(&headers);
        let decoded = decoder.decode(&encoded).unwrap();
        assert_eq!(decoded, headers);
    }

    // ── Connection preface ──────────────────────────────────────

    #[test]
    fn test_connection_preface_valid() {
        let mut cursor = std::io::Cursor::new(CONNECTION_PREFACE.to_vec());
        assert!(validate_connection_preface(&mut cursor).is_ok());
    }

    #[test]
    fn test_connection_preface_invalid() {
        let mut cursor = std::io::Cursor::new(b"GET / HTTP/1.1\r\nHost: x\r\n".to_vec());
        assert!(validate_connection_preface(&mut cursor).is_err());
    }

    // ── Settings parsing ────────────────────────────────────────

    #[test]
    fn test_settings_parse_empty() {
        let settings = parse_settings(&[]).unwrap();
        assert!(settings.is_empty());
    }

    #[test]
    fn test_settings_parse_single() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&SETTINGS_MAX_CONCURRENT_STREAMS.to_be_bytes());
        payload.extend_from_slice(&100u32.to_be_bytes());
        let settings = parse_settings(&payload).unwrap();
        assert_eq!(settings, vec![(SETTINGS_MAX_CONCURRENT_STREAMS, 100)]);
    }

    #[test]
    fn test_settings_parse_invalid_length() {
        let payload = [0u8; 5]; // Not a multiple of 6
        assert!(parse_settings(&payload).is_err());
    }

    // ── Process frame ───────────────────────────────────────────

    #[test]
    fn test_process_settings_frame() {
        let mut conn = H2Connection::new();
        let header = FrameHeader {
            length: 6,
            frame_type: FRAME_SETTINGS,
            flags: 0,
            stream_id: 0,
        };
        let mut payload = Vec::new();
        payload.extend_from_slice(&SETTINGS_MAX_CONCURRENT_STREAMS.to_be_bytes());
        payload.extend_from_slice(&256u32.to_be_bytes());

        let result = process_frame(&mut conn, &header, &payload).unwrap();
        assert!(result.is_none());
        assert_eq!(conn.peer_settings.max_concurrent_streams, 256);
    }

    #[test]
    fn test_process_settings_ack() {
        let mut conn = H2Connection::new();
        let header = FrameHeader {
            length: 0,
            frame_type: FRAME_SETTINGS,
            flags: FLAG_ACK,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &header, &[]).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_process_headers_end_stream() {
        let mut conn = H2Connection::new();
        let mut encoder = HpackEncoder::new(4096);

        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":authority".to_string(), "localhost".to_string()),
        ];
        let encoded = encoder.encode(&headers);

        let header = FrameHeader {
            length: encoded.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id: 1,
        };

        let result = process_frame(&mut conn, &header, &encoded).unwrap();
        assert!(result.is_some());
        let (stream_id, decoded_headers, body) = result.unwrap();
        assert_eq!(stream_id, 1);
        assert_eq!(decoded_headers, headers);
        assert!(body.is_empty());
    }

    #[test]
    fn test_process_data_frame() {
        let mut conn = H2Connection::new();
        let mut encoder = HpackEncoder::new(4096);

        // First send HEADERS (without END_STREAM)
        let headers = vec![
            (":method".to_string(), "POST".to_string()),
            (":path".to_string(), "/data".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);
        let h = FrameHeader {
            length: encoded.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_HEADERS, // No END_STREAM
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h, &encoded).unwrap();
        assert!(result.is_none()); // Not complete yet

        // Then send DATA with END_STREAM
        let data = b"hello body";
        let d = FrameHeader {
            length: data.len() as u32,
            frame_type: FRAME_DATA,
            flags: FLAG_END_STREAM,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &d, data).unwrap();
        assert!(result.is_some());
        let (stream_id, _, body) = result.unwrap();
        assert_eq!(stream_id, 1);
        assert_eq!(body, b"hello body");
    }

    #[test]
    fn test_process_window_update_connection() {
        let mut conn = H2Connection::new();
        let initial = conn.conn_send_window;

        let payload = 1000u32.to_be_bytes();
        let header = FrameHeader {
            length: 4,
            frame_type: FRAME_WINDOW_UPDATE,
            flags: 0,
            stream_id: 0,
        };
        process_frame(&mut conn, &header, &payload).unwrap();
        assert_eq!(conn.conn_send_window, initial + 1000);
    }

    #[test]
    fn test_process_ping() {
        let mut conn = H2Connection::new();
        let opaque = [1u8, 2, 3, 4, 5, 6, 7, 8];
        let header = FrameHeader {
            length: 8,
            frame_type: FRAME_PING,
            flags: 0,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &header, &opaque).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_process_goaway() {
        let mut conn = H2Connection::new();
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u32.to_be_bytes()); // last_stream_id
        payload.extend_from_slice(&ERROR_NO_ERROR.to_be_bytes());
        let header = FrameHeader {
            length: payload.len() as u32,
            frame_type: FRAME_GOAWAY,
            flags: 0,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &header, &payload).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_extract_request_fields_valid() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/hello".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":authority".to_string(), "localhost:8443".to_string()),
            ("accept".to_string(), "text/plain".to_string()),
        ];
        let (method, path, authority, regular) = extract_request_fields(&headers).unwrap();
        assert_eq!(method, "GET");
        assert_eq!(path, "/hello");
        assert_eq!(authority, "localhost:8443");
        assert_eq!(
            regular,
            vec![("accept".to_string(), "text/plain".to_string())]
        );
    }

    #[test]
    fn test_extract_request_fields_missing_method() {
        let headers = vec![(":path".to_string(), "/hello".to_string())];
        assert!(extract_request_fields(&headers).is_err());
    }

    // ── Stream state ────────────────────────────────────────────

    #[test]
    fn test_stream_state_initial() {
        let stream = H2Stream::new(65535);
        assert_eq!(stream.state, StreamState::Idle);
        assert!(stream.request_headers.is_empty());
        assert!(stream.request_body.is_empty());
        assert_eq!(stream.send_window, 65535);
    }

    // ── Error formatting ────────────────────────────────────────

    #[test]
    fn test_h2_error_display() {
        let err = H2Error::Connection(ERROR_PROTOCOL_ERROR, "test".into());
        let s = format!("{}", err);
        assert!(s.contains("h2 connection error"));
        assert!(s.contains("0x1"));
    }

    // ── Settings application ────────────────────────────────────

    #[test]
    fn test_apply_initial_window_size_change() {
        let mut conn = H2Connection::new();
        // Add a stream
        conn.streams.insert(1, H2Stream::new(65535));
        let old_window = conn.streams[&1].send_window;

        // Change initial window size
        let settings = vec![(SETTINGS_INITIAL_WINDOW_SIZE, 131070u32)];
        apply_peer_settings(&mut conn, &settings).unwrap();

        let new_window = conn.streams[&1].send_window;
        assert_eq!(new_window, old_window + (131070 - 65535));
    }

    #[test]
    fn test_apply_invalid_enable_push() {
        let mut conn = H2Connection::new();
        let settings = vec![(SETTINGS_ENABLE_PUSH, 2)];
        assert!(apply_peer_settings(&mut conn, &settings).is_err());
    }

    #[test]
    fn test_apply_invalid_max_frame_size() {
        let mut conn = H2Connection::new();
        let settings = vec![(SETTINGS_MAX_FRAME_SIZE, 100)]; // Too small
        assert!(apply_peer_settings(&mut conn, &settings).is_err());
    }

    // ── End-to-end frame-level test ─────────────────────────────

    #[test]
    fn test_h2_full_request_response_cycle() {
        // Simulate a full h2 exchange over an in-memory pipe:
        // Client: preface → SETTINGS → SETTINGS ACK → HEADERS (GET /) → read response
        // Server: read preface → process client SETTINGS → send SETTINGS → SETTINGS ACK → process HEADERS → send response

        // Use a pair of cursors to simulate the exchange.
        // Client writes to a buffer, server reads from it.
        let mut client_to_server = Vec::new();

        // Client sends connection preface
        client_to_server.extend_from_slice(CONNECTION_PREFACE);

        // Client sends SETTINGS frame (empty = defaults)
        let client_settings_header = FrameHeader {
            length: 0,
            frame_type: FRAME_SETTINGS,
            flags: 0,
            stream_id: 0,
        };
        client_to_server.extend_from_slice(&client_settings_header.serialize());

        // Client sends HEADERS frame for GET /
        let mut client_encoder = HpackEncoder::new(4096);
        let request_headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/hello".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":authority".to_string(), "localhost".to_string()),
            ("user-agent".to_string(), "taida-test/1.0".to_string()),
        ];
        let encoded_headers = client_encoder.encode(&request_headers);
        let headers_frame = FrameHeader {
            length: encoded_headers.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id: 1,
        };
        client_to_server.extend_from_slice(&headers_frame.serialize());
        client_to_server.extend_from_slice(&encoded_headers);

        // Server processes the client data
        let mut reader = std::io::Cursor::new(client_to_server);
        let mut h2_conn = H2Connection::new();

        // 1. Validate connection preface
        validate_connection_preface(&mut reader).unwrap();

        // 2. Read and process client SETTINGS
        let (frame_hdr, payload) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(frame_hdr.frame_type, FRAME_SETTINGS);
        let result = process_frame(&mut h2_conn, &frame_hdr, &payload).unwrap();
        assert!(result.is_none());

        // 3. Read and process HEADERS
        let (frame_hdr, payload) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(frame_hdr.frame_type, FRAME_HEADERS);
        let result = process_frame(&mut h2_conn, &frame_hdr, &payload).unwrap();
        assert!(result.is_some());

        let (stream_id, headers, body) = result.unwrap();
        assert_eq!(stream_id, 1);
        assert!(body.is_empty());

        // Verify headers
        let (method, path, authority, regular) = extract_request_fields(&headers).unwrap();
        assert_eq!(method, "GET");
        assert_eq!(path, "/hello");
        assert_eq!(authority, "localhost");
        assert_eq!(regular.len(), 1);
        assert_eq!(regular[0].0, "user-agent");

        // 4. Server sends response
        let mut response_buf = Vec::new();

        // Send server SETTINGS + SETTINGS ACK
        send_settings(&mut response_buf, &h2_conn.local_settings).unwrap();
        send_settings_ack(&mut response_buf).unwrap();

        // Send response HEADERS
        let response_headers = vec![
            ("content-type".to_string(), "text/plain".to_string()),
            ("content-length".to_string(), "5".to_string()),
        ];
        send_response_headers(
            &mut response_buf,
            &mut h2_conn.encoder,
            1,
            200,
            &response_headers,
            false,
            DEFAULT_MAX_FRAME_SIZE,
        )
        .unwrap();

        // Send response DATA (with ample flow-control windows)
        let mut conn_sw = DEFAULT_INITIAL_WINDOW_SIZE as i64;
        let mut stream_sw = DEFAULT_INITIAL_WINDOW_SIZE as i64;
        send_response_data(
            &mut response_buf,
            1,
            b"hello",
            true,
            DEFAULT_MAX_FRAME_SIZE,
            &mut conn_sw,
            &mut stream_sw,
        )
        .unwrap();

        // Verify the response frames
        let mut resp_reader = std::io::Cursor::new(response_buf);

        // Read server SETTINGS
        let (hdr, _payload) = read_frame(&mut resp_reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(hdr.frame_type, FRAME_SETTINGS);
        assert_eq!(hdr.flags & FLAG_ACK, 0); // Not an ACK

        // Read SETTINGS ACK
        let (hdr, _payload) = read_frame(&mut resp_reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(hdr.frame_type, FRAME_SETTINGS);
        assert_ne!(hdr.flags & FLAG_ACK, 0); // Is an ACK

        // Read response HEADERS
        let (hdr, payload) = read_frame(&mut resp_reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(hdr.frame_type, FRAME_HEADERS);
        assert_eq!(hdr.stream_id, 1);
        assert_ne!(hdr.flags & FLAG_END_HEADERS, 0);
        assert_eq!(hdr.flags & FLAG_END_STREAM, 0); // Not END_STREAM (body follows)

        // Decode response headers with a fresh decoder
        let mut resp_decoder = HpackDecoder::new(4096);
        let resp_headers = resp_decoder.decode(&payload).unwrap();
        assert!(
            resp_headers
                .iter()
                .any(|(n, v)| n == ":status" && v == "200")
        );
        assert!(
            resp_headers
                .iter()
                .any(|(n, v)| n == "content-type" && v == "text/plain")
        );

        // Read response DATA
        let (hdr, payload) = read_frame(&mut resp_reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(hdr.frame_type, FRAME_DATA);
        assert_eq!(hdr.stream_id, 1);
        assert_ne!(hdr.flags & FLAG_END_STREAM, 0);
        assert_eq!(&payload, b"hello");
    }

    #[test]
    fn test_h2_post_with_body() {
        let mut client_data = Vec::new();

        // Connection preface
        client_data.extend_from_slice(CONNECTION_PREFACE);

        // Client SETTINGS
        client_data.extend_from_slice(
            &FrameHeader {
                length: 0,
                frame_type: FRAME_SETTINGS,
                flags: 0,
                stream_id: 0,
            }
            .serialize(),
        );

        // HEADERS for POST /data (no END_STREAM — body follows)
        let mut enc = HpackEncoder::new(4096);
        let hdrs = vec![
            (":method".to_string(), "POST".to_string()),
            (":path".to_string(), "/data".to_string()),
            (":scheme".to_string(), "https".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        let encoded = enc.encode(&hdrs);
        client_data.extend_from_slice(
            &FrameHeader {
                length: encoded.len() as u32,
                frame_type: FRAME_HEADERS,
                flags: FLAG_END_HEADERS, // No END_STREAM
                stream_id: 1,
            }
            .serialize(),
        );
        client_data.extend_from_slice(&encoded);

        // DATA frame with body and END_STREAM
        let body = b"{\"key\": \"value\"}";
        client_data.extend_from_slice(
            &FrameHeader {
                length: body.len() as u32,
                frame_type: FRAME_DATA,
                flags: FLAG_END_STREAM,
                stream_id: 1,
            }
            .serialize(),
        );
        client_data.extend_from_slice(body);

        // Process
        let mut reader = std::io::Cursor::new(client_data);
        let mut conn = H2Connection::new();

        validate_connection_preface(&mut reader).unwrap();

        // SETTINGS
        let (h, p) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        process_frame(&mut conn, &h, &p).unwrap();

        // HEADERS
        let (h, p) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        let result = process_frame(&mut conn, &h, &p).unwrap();
        assert!(result.is_none()); // Body not yet received

        // DATA
        let (h, p) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        let result = process_frame(&mut conn, &h, &p).unwrap();
        assert!(result.is_some());

        let (stream_id, headers, received_body) = result.unwrap();
        assert_eq!(stream_id, 1);
        assert_eq!(received_body, body);
        let (method, path, _, _) = extract_request_fields(&headers).unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/data");
    }

    #[test]
    fn test_h2_multiple_streams() {
        // Test multiplexed streams: stream 1 and stream 3
        let mut client_data = Vec::new();

        client_data.extend_from_slice(CONNECTION_PREFACE);
        client_data.extend_from_slice(
            &FrameHeader {
                length: 0,
                frame_type: FRAME_SETTINGS,
                flags: 0,
                stream_id: 0,
            }
            .serialize(),
        );

        let mut enc = HpackEncoder::new(4096);

        // Stream 1: GET /first
        let hdrs1 = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/first".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded1 = enc.encode(&hdrs1);
        client_data.extend_from_slice(
            &FrameHeader {
                length: encoded1.len() as u32,
                frame_type: FRAME_HEADERS,
                flags: FLAG_END_HEADERS | FLAG_END_STREAM,
                stream_id: 1,
            }
            .serialize(),
        );
        client_data.extend_from_slice(&encoded1);

        // Stream 3: GET /second
        let hdrs3 = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/second".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded3 = enc.encode(&hdrs3);
        client_data.extend_from_slice(
            &FrameHeader {
                length: encoded3.len() as u32,
                frame_type: FRAME_HEADERS,
                flags: FLAG_END_HEADERS | FLAG_END_STREAM,
                stream_id: 3,
            }
            .serialize(),
        );
        client_data.extend_from_slice(&encoded3);

        // Process
        let mut reader = std::io::Cursor::new(client_data);
        let mut conn = H2Connection::new();

        validate_connection_preface(&mut reader).unwrap();

        let (h, p) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        process_frame(&mut conn, &h, &p).unwrap(); // SETTINGS

        // Stream 1
        let (h, p) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        let result = process_frame(&mut conn, &h, &p).unwrap();
        assert!(result.is_some());
        let (id, hdrs, _) = result.unwrap();
        assert_eq!(id, 1);
        let (_, path, _, _) = extract_request_fields(&hdrs).unwrap();
        assert_eq!(path, "/first");

        // Stream 3
        let (h, p) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        let result = process_frame(&mut conn, &h, &p).unwrap();
        assert!(result.is_some());
        let (id, hdrs, _) = result.unwrap();
        assert_eq!(id, 3);
        let (_, path, _, _) = extract_request_fields(&hdrs).unwrap();
        assert_eq!(path, "/second");
    }

    #[test]
    fn test_h2_flow_control_window_update() {
        let mut conn = H2Connection::new();
        let initial_conn_window = conn.conn_recv_window;

        // Simulate receiving a DATA frame to deplete window
        let mut stream = H2Stream::new(65535);
        stream.state = StreamState::HalfClosedRemote;
        conn.streams.insert(1, stream);

        let data_len = 1000;
        let header = FrameHeader {
            length: data_len,
            frame_type: FRAME_DATA,
            flags: 0, // No END_STREAM
            stream_id: 1,
        };
        let payload = vec![0u8; data_len as usize];
        process_frame(&mut conn, &header, &payload).unwrap();

        // Connection recv window should be reduced
        assert_eq!(conn.conn_recv_window, initial_conn_window - data_len as i64);
        // Stream recv window should be reduced
        assert_eq!(
            conn.streams[&1].recv_window,
            DEFAULT_INITIAL_WINDOW_SIZE as i64 - data_len as i64
        );
    }

    #[test]
    fn test_h2_large_data_frame_splitting() {
        // Test that send_response_data splits data across multiple frames
        let mut buf = Vec::new();
        let max_frame = 16384u32;
        let data = vec![0x42u8; 32768]; // 2x max frame size

        let mut conn_sw = 1_000_000i64; // large enough window
        let mut stream_sw = 1_000_000i64;
        send_response_data(
            &mut buf,
            1,
            &data,
            true,
            max_frame,
            &mut conn_sw,
            &mut stream_sw,
        )
        .unwrap();

        // Should produce 2 DATA frames
        let mut reader = std::io::Cursor::new(buf);

        // First frame: max_frame_size bytes, no END_STREAM
        let (h1, p1) = read_frame(&mut reader, max_frame).unwrap();
        assert_eq!(h1.frame_type, FRAME_DATA);
        assert_eq!(h1.stream_id, 1);
        assert_eq!(p1.len(), max_frame as usize);
        assert_eq!(h1.flags & FLAG_END_STREAM, 0);

        // Second frame: remaining bytes, END_STREAM
        let (h2, p2) = read_frame(&mut reader, max_frame).unwrap();
        assert_eq!(h2.frame_type, FRAME_DATA);
        assert_eq!(h2.stream_id, 1);
        assert_eq!(p2.len(), 32768 - max_frame as usize);
        assert_ne!(h2.flags & FLAG_END_STREAM, 0);
    }

    // ── NB6-12: Flow control enforcement tests ─────────────────

    #[test]
    fn test_recv_flow_control_connection_window_exceeded() {
        // DATA frame that exceeds the connection receive window should be rejected
        // with FLOW_CONTROL_ERROR (connection-level).
        let mut conn = H2Connection::new();

        // Set up a stream
        let mut stream = H2Stream::new(65535);
        stream.state = StreamState::HalfClosedRemote;
        conn.streams.insert(1, stream);

        // Shrink connection recv window to something small
        conn.conn_recv_window = 100;

        // Send a DATA frame larger than the connection window
        let payload = vec![0u8; 200];
        let header = FrameHeader {
            length: 200,
            frame_type: FRAME_DATA,
            flags: 0,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &header, &payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_FLOW_CONTROL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_recv_flow_control_stream_window_exceeded() {
        // DATA frame that exceeds the stream receive window (but not connection)
        // should be rejected with FLOW_CONTROL_ERROR (stream-level RST_STREAM).
        let mut conn = H2Connection::new();

        // Set up a stream with a small recv window
        let mut stream = H2Stream::new(65535);
        stream.state = StreamState::HalfClosedRemote;
        stream.recv_window = 50;
        conn.streams.insert(1, stream);

        // Connection window is ample
        conn.conn_recv_window = 65535;

        let payload = vec![0u8; 100];
        let header = FrameHeader {
            length: 100,
            frame_type: FRAME_DATA,
            flags: 0,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &header, &payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(stream_id, code, _) => {
                assert_eq!(stream_id, 1);
                assert_eq!(code, ERROR_FLOW_CONTROL_ERROR);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_recv_flow_control_within_window_succeeds() {
        // DATA frame within both windows should succeed.
        let mut conn = H2Connection::new();

        let mut stream = H2Stream::new(65535);
        stream.state = StreamState::HalfClosedRemote;
        conn.streams.insert(1, stream);

        let payload = vec![0u8; 1000];
        let header = FrameHeader {
            length: 1000,
            frame_type: FRAME_DATA,
            flags: 0,
            stream_id: 1,
        };
        let initial_conn_w = conn.conn_recv_window;
        let initial_stream_w = conn.streams[&1].recv_window;
        process_frame(&mut conn, &header, &payload).unwrap();
        assert_eq!(conn.conn_recv_window, initial_conn_w - 1000);
        assert_eq!(conn.streams[&1].recv_window, initial_stream_w - 1000);
    }

    #[test]
    fn test_send_flow_control_window_exhaustion() {
        // send_response_data should error when window is exhausted.
        let mut buf = Vec::new();
        let mut conn_sw = 500i64;
        let mut stream_sw = 500i64;
        let data = vec![0x42u8; 1000]; // More than window

        let result = send_response_data(
            &mut buf,
            1,
            &data,
            true,
            DEFAULT_MAX_FRAME_SIZE,
            &mut conn_sw,
            &mut stream_sw,
        );
        assert!(result.is_err());

        // Verify that exactly 500 bytes were sent before exhaustion
        let mut reader = std::io::Cursor::new(buf);
        let (h1, p1) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(h1.frame_type, FRAME_DATA);
        assert_eq!(p1.len(), 500);
        // Connection window should be at 0 after the chunk
        assert_eq!(conn_sw, 0);
        assert_eq!(stream_sw, 0);
    }

    #[test]
    fn test_send_flow_control_respects_minimum_window() {
        // send_response_data should use min(frame_size, conn_w, stream_w)
        let mut buf = Vec::new();
        let mut conn_sw = 100i64; // Smallest — should be the limit
        let mut stream_sw = 500i64;
        let data = vec![0x42u8; 80]; // Less than conn_sw

        let result = send_response_data(
            &mut buf,
            1,
            &data,
            true,
            DEFAULT_MAX_FRAME_SIZE,
            &mut conn_sw,
            &mut stream_sw,
        );
        assert!(result.is_ok());
        assert_eq!(conn_sw, 100 - 80);
        assert_eq!(stream_sw, 500 - 80);

        // Should be a single frame with END_STREAM
        let mut reader = std::io::Cursor::new(buf);
        let (h, p) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(h.frame_type, FRAME_DATA);
        assert_eq!(p.len(), 80);
        assert_ne!(h.flags & FLAG_END_STREAM, 0);
    }

    // ── NB6-12: send_h2_response partial body must not be success ──

    #[test]
    fn test_send_h2_response_flow_rejects_partial_body() {
        // Integration test: simulate the full HEADERS + DATA path that
        // send_h2_response() uses. When the send window is too small for
        // the entire body, the sequence must:
        //   1. Successfully send HEADERS
        //   2. Send as much DATA as the window allows
        //   3. Return Err (NOT Ok) for the data phase
        //   4. Caller should send RST_STREAM to abort the stream cleanly
        //
        // This verifies that partial body is never treated as success.
        let mut buf = Vec::new();
        let mut encoder = HpackEncoder::new(4096);
        let stream_id = 1u32;
        let status = 200u16;
        let body = vec![0x41u8; 2000]; // 2000 bytes body

        // Step 1: Send HEADERS (no END_STREAM) — should succeed
        let headers_result = send_response_headers(
            &mut buf,
            &mut encoder,
            stream_id,
            status,
            &[("content-length".to_string(), "2000".to_string())],
            false, // no END_STREAM — body follows
            DEFAULT_MAX_FRAME_SIZE,
        );
        assert!(headers_result.is_ok(), "HEADERS should succeed");

        // Step 2: Attempt to send DATA with insufficient window (500 < 2000)
        let mut conn_sw = 500i64;
        let mut stream_sw = 500i64;
        let data_result = send_response_data(
            &mut buf,
            stream_id,
            &body,
            true,
            DEFAULT_MAX_FRAME_SIZE,
            &mut conn_sw,
            &mut stream_sw,
        );

        // Step 3: Must be Err — partial body is not success
        assert!(data_result.is_err(), "partial body must not be Ok");

        // Step 4: Caller sends RST_STREAM to abort
        let rst_result = send_rst_stream(&mut buf, stream_id, ERROR_FLOW_CONTROL_ERROR);
        assert!(rst_result.is_ok(), "RST_STREAM should succeed");

        // Verify the wire: HEADERS, partial DATA (500 bytes), RST_STREAM
        let mut reader = std::io::Cursor::new(buf);

        // Frame 1: HEADERS
        let (h1, _p1) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(h1.frame_type, FRAME_HEADERS);
        assert_eq!(h1.stream_id, stream_id);
        assert_eq!(
            h1.flags & FLAG_END_STREAM,
            0,
            "HEADERS must not have END_STREAM"
        );

        // Frame 2: DATA (partial — 500 bytes, no END_STREAM)
        let (h2, p2) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(h2.frame_type, FRAME_DATA);
        assert_eq!(h2.stream_id, stream_id);
        assert_eq!(p2.len(), 500, "only 500 bytes should have been sent");
        assert_eq!(
            h2.flags & FLAG_END_STREAM,
            0,
            "partial DATA must not have END_STREAM"
        );

        // Frame 3: RST_STREAM
        let (h3, p3) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(h3.frame_type, FRAME_RST_STREAM);
        assert_eq!(h3.stream_id, stream_id);
        let error_code = u32::from_be_bytes([p3[0], p3[1], p3[2], p3[3]]);
        assert_eq!(error_code, ERROR_FLOW_CONTROL_ERROR);

        // No more frames
        assert!(
            read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).is_err(),
            "no more frames expected"
        );
    }

    // ── NB6-11: CONTINUATION frame tests ───────────────────────

    #[test]
    fn test_continuation_inbound_reassembly() {
        // Simulate a HEADERS frame without END_HEADERS, followed by a
        // CONTINUATION frame with END_HEADERS. The header block should be
        // reassembled and decoded correctly.
        let mut conn = H2Connection::new();
        let mut encoder = HpackEncoder::new(4096);

        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/cont".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);

        // Split the encoded header block in half
        let mid = encoded.len() / 2;
        let part1 = &encoded[..mid];
        let part2 = &encoded[mid..];

        // HEADERS frame (no END_HEADERS, but END_STREAM)
        let h1 = FrameHeader {
            length: part1.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_STREAM, // No END_HEADERS
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h1, part1).unwrap();
        assert!(result.is_none()); // Not complete yet — waiting for CONTINUATION

        // CONTINUATION frame with END_HEADERS
        let h2 = FrameHeader {
            length: part2.len() as u32,
            frame_type: FRAME_CONTINUATION,
            flags: FLAG_END_HEADERS,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h2, part2).unwrap();
        assert!(result.is_some());

        let (stream_id, decoded_headers, body) = result.unwrap();
        assert_eq!(stream_id, 1);
        assert_eq!(decoded_headers, headers);
        assert!(body.is_empty());
    }

    #[test]
    fn test_continuation_wrong_stream_id() {
        // CONTINUATION with a different stream_id than the pending HEADERS
        // should be a protocol error.
        let mut conn = H2Connection::new();
        let mut encoder = HpackEncoder::new(4096);

        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);
        let mid = encoded.len() / 2;

        // HEADERS on stream 1 without END_HEADERS
        let h1 = FrameHeader {
            length: mid as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_STREAM,
            stream_id: 1,
        };
        process_frame(&mut conn, &h1, &encoded[..mid]).unwrap();

        // CONTINUATION on stream 3 (wrong!)
        let h2 = FrameHeader {
            length: (encoded.len() - mid) as u32,
            frame_type: FRAME_CONTINUATION,
            flags: FLAG_END_HEADERS,
            stream_id: 3,
        };
        let result = process_frame(&mut conn, &h2, &encoded[mid..]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_continuation_unexpected_without_headers() {
        // CONTINUATION without a preceding HEADERS should be a protocol error.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 5,
            frame_type: FRAME_CONTINUATION,
            flags: FLAG_END_HEADERS,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 5]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_continuation_sequence_blocks_other_frames() {
        // During a CONTINUATION sequence, receiving any non-CONTINUATION frame
        // is a protocol error.
        let mut conn = H2Connection::new();
        let mut encoder = HpackEncoder::new(4096);

        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);
        let mid = encoded.len() / 2;

        // Start CONTINUATION sequence
        let h1 = FrameHeader {
            length: mid as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_STREAM,
            stream_id: 1,
        };
        process_frame(&mut conn, &h1, &encoded[..mid]).unwrap();

        // Try to send a PING during the sequence — should error
        let ping = FrameHeader {
            length: 8,
            frame_type: FRAME_PING,
            flags: 0,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &ping, &[0u8; 8]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_outbound_headers_continuation_splitting() {
        // When the encoded header block exceeds max_frame_size, send_response_headers
        // should split into HEADERS + CONTINUATION frames.
        let mut buf = Vec::new();
        let mut encoder = HpackEncoder::new(4096);

        // Create headers large enough to exceed a tiny max_frame_size.
        // We use a small max_frame_size (50 bytes) to force splitting.
        // Note: actual protocol minimum is 16384, but for testing we go smaller.
        let small_max = 50u32;
        let headers = vec![
            (
                "x-long-header-name-1".to_string(),
                "some-longish-value-here-foo".to_string(),
            ),
            (
                "x-long-header-name-2".to_string(),
                "another-longish-value-bar".to_string(),
            ),
            (
                "x-long-header-name-3".to_string(),
                "yet-more-long-value-data".to_string(),
            ),
        ];

        send_response_headers(&mut buf, &mut encoder, 1, 200, &headers, true, small_max).unwrap();

        // Read frames and verify structure
        // We need to use a large enough max_frame_size for read_frame since the
        // actual frame sizes are small_max (50).
        let mut reader = std::io::Cursor::new(buf);

        // First frame should be HEADERS without END_HEADERS
        let (h1, _p1) = read_frame(&mut reader, MAX_MAX_FRAME_SIZE).unwrap();
        assert_eq!(h1.frame_type, FRAME_HEADERS);
        assert_eq!(h1.stream_id, 1);
        assert_ne!(h1.flags & FLAG_END_STREAM, 0); // END_STREAM on first frame
        assert_eq!(h1.flags & FLAG_END_HEADERS, 0); // No END_HEADERS
        assert!(h1.length <= small_max);

        // Read subsequent CONTINUATION frames until END_HEADERS
        let mut end_headers_found = false;
        let mut frame_count = 1;
        while !end_headers_found {
            let (hc, _pc) = read_frame(&mut reader, MAX_MAX_FRAME_SIZE).unwrap();
            assert_eq!(hc.frame_type, FRAME_CONTINUATION);
            assert_eq!(hc.stream_id, 1);
            assert!(hc.length <= small_max);
            frame_count += 1;
            if hc.flags & FLAG_END_HEADERS != 0 {
                end_headers_found = true;
            }
        }
        // There should be more than 1 frame total
        assert!(
            frame_count >= 2,
            "expected HEADERS + at least 1 CONTINUATION, got {} frames",
            frame_count
        );
    }

    #[test]
    fn test_outbound_headers_single_frame_when_small() {
        // When the encoded header block fits in max_frame_size,
        // send_response_headers should produce a single HEADERS frame with END_HEADERS.
        let mut buf = Vec::new();
        let mut encoder = HpackEncoder::new(4096);
        let headers = vec![("content-type".to_string(), "text/plain".to_string())];

        send_response_headers(
            &mut buf,
            &mut encoder,
            1,
            200,
            &headers,
            true,
            DEFAULT_MAX_FRAME_SIZE,
        )
        .unwrap();

        let mut reader = std::io::Cursor::new(buf);
        let (h, _) = read_frame(&mut reader, DEFAULT_MAX_FRAME_SIZE).unwrap();
        assert_eq!(h.frame_type, FRAME_HEADERS);
        assert_ne!(h.flags & FLAG_END_HEADERS, 0);
        assert_ne!(h.flags & FLAG_END_STREAM, 0);
    }

    // ── NB6-13: Pseudo-header validation tests ─────────────────

    #[test]
    fn test_pseudo_header_after_regular_header_rejected() {
        // Pseudo-headers after regular headers must be rejected.
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            ("accept".to_string(), "text/html".to_string()),
            (":path".to_string(), "/".to_string()), // Out of order!
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, msg) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
                assert!(msg.contains("ordering violation"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_missing_scheme_rejected() {
        // :scheme is required per RFC 9113 Section 8.3.1.
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            // No :scheme!
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, msg) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
                assert!(msg.contains(":scheme"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_all_pseudo_headers_before_regular_accepted() {
        // Valid ordering: all pseudo-headers first, then regular headers.
        let headers = vec![
            (":method".to_string(), "POST".to_string()),
            (":path".to_string(), "/api".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":authority".to_string(), "example.com".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_ok());
        let (method, path, authority, regular) = result.unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/api");
        assert_eq!(authority, "example.com");
        assert_eq!(regular.len(), 1);
    }

    // ── NET6-5a: Malformed input / edge case hardening tests ─────

    #[test]
    fn test_window_update_overflow_connection() {
        // RFC 9113 Section 6.9.1: flow-control window MUST NOT exceed 2^31-1.
        let mut conn = H2Connection::new();
        // Set connection send window near max
        conn.conn_send_window = 0x7FFFFF00;

        let wu_header = FrameHeader {
            length: 4,
            frame_type: FRAME_WINDOW_UPDATE,
            flags: 0,
            stream_id: 0,
        };
        // Increment that would push past 2^31-1
        let increment: u32 = 0x00000200;
        let payload = increment.to_be_bytes();

        let result = process_frame(&mut conn, &wu_header, &payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, msg) => {
                assert_eq!(code, ERROR_FLOW_CONTROL_ERROR);
                assert!(msg.contains("overflow"), "msg: {}", msg);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_window_update_overflow_stream() {
        // RFC 9113 Section 6.9.1: stream-level window MUST NOT exceed 2^31-1.
        let mut conn = H2Connection::new();
        // Create a stream via HEADERS
        let mut encoder = HpackEncoder::new(4096);
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);
        let h = FrameHeader {
            length: encoded.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id: 1,
        };
        let _ = process_frame(&mut conn, &h, &encoded);

        // Set stream send window near max
        if let Some(stream) = conn.streams.get_mut(&1) {
            stream.send_window = 0x7FFFFF00;
        }

        let wu_header = FrameHeader {
            length: 4,
            frame_type: FRAME_WINDOW_UPDATE,
            flags: 0,
            stream_id: 1,
        };
        let increment: u32 = 0x00000200;
        let payload = increment.to_be_bytes();

        let result = process_frame(&mut conn, &wu_header, &payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(sid, code, msg) => {
                assert_eq!(sid, 1);
                assert_eq!(code, ERROR_FLOW_CONTROL_ERROR);
                assert!(msg.contains("overflow"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_window_update_within_limit_succeeds() {
        // Normal WINDOW_UPDATE that stays within limits should succeed.
        let mut conn = H2Connection::new();
        let wu_header = FrameHeader {
            length: 4,
            frame_type: FRAME_WINDOW_UPDATE,
            flags: 0,
            stream_id: 0,
        };
        let increment: u32 = 1000;
        let payload = increment.to_be_bytes();

        let old_window = conn.conn_send_window;
        let result = process_frame(&mut conn, &wu_header, &payload);
        assert!(result.is_ok());
        assert_eq!(conn.conn_send_window, old_window + 1000);
    }

    #[test]
    fn test_duplicate_method_pseudo_header_rejected() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":method".to_string(), "POST".to_string()), // Duplicate!
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, msg) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
                assert!(msg.contains("duplicate"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_duplicate_path_pseudo_header_rejected() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/a".to_string()),
            (":path".to_string(), "/b".to_string()), // Duplicate!
            (":scheme".to_string(), "https".to_string()),
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, msg) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
                assert!(msg.contains("duplicate"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_duplicate_scheme_pseudo_header_rejected() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":scheme".to_string(), "http".to_string()), // Duplicate!
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, msg) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
                assert!(msg.contains("duplicate"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_duplicate_authority_pseudo_header_rejected() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":authority".to_string(), "a.com".to_string()),
            (":authority".to_string(), "b.com".to_string()), // Duplicate!
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, msg) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
                assert!(msg.contains("duplicate"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_method_value_rejected() {
        let headers = vec![
            (":method".to_string(), "".to_string()), // Empty!
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, msg) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
                assert!(msg.contains("empty"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_empty_path_value_rejected() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "".to_string()), // Empty!
            (":scheme".to_string(), "https".to_string()),
        ];
        let result = extract_request_fields(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, msg) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
                assert!(msg.contains("empty"), "msg: {}", msg);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_header_list_size_validation_within_limit() {
        let headers = vec![
            ("content-type".to_string(), "text/plain".to_string()),
            ("accept".to_string(), "*/*".to_string()),
        ];
        assert!(validate_header_list_size(&headers).is_ok());
    }

    #[test]
    fn test_header_list_size_validation_exceeds_limit() {
        // Create a header list that exceeds 64KB
        let mut headers = Vec::new();
        for i in 0..1000 {
            headers.push((
                format!("x-header-{}", i),
                "a".repeat(100), // each entry ~142 bytes (name + value + 32)
            ));
        }
        let result = validate_header_list_size(&headers);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, msg) => {
                assert_eq!(code, ERROR_INTERNAL_ERROR);
                assert!(msg.contains("safety limit"), "msg: {}", msg);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_continuation_buffer_size_limit() {
        // Simulate CONTINUATION accumulation beyond safety limit.
        let mut conn = H2Connection::new();
        let mut encoder = HpackEncoder::new(4096);
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);

        // Start a HEADERS without END_HEADERS
        let h = FrameHeader {
            length: encoded.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_STREAM, // No END_HEADERS
            stream_id: 1,
        };
        let _ = process_frame(&mut conn, &h, &encoded);
        assert_eq!(conn.continuation_stream_id, 1);

        // Try to send a very large CONTINUATION frame (> MAX_CONTINUATION_BUFFER_SIZE)
        let large_payload = vec![0u8; MAX_CONTINUATION_BUFFER_SIZE + 1];
        let cont = FrameHeader {
            length: large_payload.len() as u32,
            frame_type: FRAME_CONTINUATION,
            flags: FLAG_END_HEADERS,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &cont, &large_payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, msg) => {
                assert_eq!(code, ERROR_INTERNAL_ERROR);
                assert!(msg.contains("safety limit"), "msg: {}", msg);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_settings_initial_window_size_overflow_rejected() {
        // RFC 9113 Section 6.5.2: SETTINGS_INITIAL_WINDOW_SIZE > 2^31-1 is
        // FLOW_CONTROL_ERROR.
        let mut conn = H2Connection::new();
        let settings = vec![(SETTINGS_INITIAL_WINDOW_SIZE, 0x80000000)]; // > 2^31-1
        let result = apply_peer_settings(&mut conn, &settings);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_FLOW_CONTROL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_settings_max_frame_size_out_of_range_rejected() {
        // MAX_FRAME_SIZE must be in [16384, 16777215].
        let mut conn = H2Connection::new();

        // Too small
        let settings = vec![(SETTINGS_MAX_FRAME_SIZE, 100)];
        let result = apply_peer_settings(&mut conn, &settings);
        assert!(result.is_err());

        // Too large
        let mut conn2 = H2Connection::new();
        let settings2 = vec![(SETTINGS_MAX_FRAME_SIZE, 16_777_216)]; // 2^24
        let result2 = apply_peer_settings(&mut conn2, &settings2);
        assert!(result2.is_err());

        // Just right
        let mut conn3 = H2Connection::new();
        let settings3 = vec![(SETTINGS_MAX_FRAME_SIZE, 16_384)]; // minimum valid
        assert!(apply_peer_settings(&mut conn3, &settings3).is_ok());
    }

    #[test]
    fn test_settings_enable_push_invalid_rejected() {
        // ENABLE_PUSH must be 0 or 1.
        let mut conn = H2Connection::new();
        let settings = vec![(SETTINGS_ENABLE_PUSH, 2)];
        let result = apply_peer_settings(&mut conn, &settings);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_settings_payload_not_multiple_of_six_rejected() {
        // SETTINGS payload length must be a multiple of 6.
        let bad_payload = [0u8; 7]; // 7 is not a multiple of 6
        let result = parse_settings(&bad_payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_FRAME_SIZE_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_data_on_stream_zero_rejected() {
        // DATA frame on stream 0 is a PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 5,
            frame_type: FRAME_DATA,
            flags: 0,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 5]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_headers_on_even_stream_rejected() {
        // HEADERS on even stream ID is a PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        let mut encoder = HpackEncoder::new(4096);
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);
        let h = FrameHeader {
            length: encoded.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id: 2, // Even!
        };
        let result = process_frame(&mut conn, &h, &encoded);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_headers_decreasing_stream_id_rejected() {
        // New stream ID must be strictly greater than previous.
        let mut conn = H2Connection::new();
        let mut encoder = HpackEncoder::new(4096);
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);

        // First request on stream 3
        let h1 = FrameHeader {
            length: encoded.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id: 3,
        };
        assert!(process_frame(&mut conn, &h1, &encoded).is_ok());

        // Second request on stream 1 (lower!) should fail
        let encoded2 = encoder.encode(&headers);
        let h2 = FrameHeader {
            length: encoded2.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h2, &encoded2);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_rst_stream_on_stream_zero_rejected() {
        // RST_STREAM on stream 0 is a PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 4,
            frame_type: FRAME_RST_STREAM,
            flags: 0,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 4]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_rst_stream_wrong_length_rejected() {
        // RST_STREAM must be exactly 4 bytes.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 3,
            frame_type: FRAME_RST_STREAM,
            flags: 0,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 3]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_FRAME_SIZE_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_priority_wrong_length_rejected() {
        // PRIORITY must be exactly 5 bytes.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 4,
            frame_type: FRAME_PRIORITY,
            flags: 0,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 4]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_FRAME_SIZE_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_push_promise_rejected() {
        // PUSH_PROMISE from client is always rejected.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 4,
            frame_type: FRAME_PUSH_PROMISE,
            flags: 0,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 4]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_ping_wrong_stream_rejected() {
        // PING on non-zero stream is a PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 8,
            frame_type: FRAME_PING,
            flags: 0,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 8]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_ping_wrong_length_rejected() {
        // PING must be exactly 8 bytes.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 7,
            frame_type: FRAME_PING,
            flags: 0,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 7]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_FRAME_SIZE_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_settings_ack_with_payload_rejected() {
        // SETTINGS ACK with non-zero length is a FRAME_SIZE_ERROR.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 6,
            frame_type: FRAME_SETTINGS,
            flags: FLAG_ACK,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 6]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_FRAME_SIZE_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_settings_on_non_zero_stream_rejected() {
        // SETTINGS on non-zero stream is a PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 0,
            frame_type: FRAME_SETTINGS,
            flags: 0,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h, &[]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_window_update_zero_increment_connection_rejected() {
        // WINDOW_UPDATE with zero increment on connection is PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 4,
            frame_type: FRAME_WINDOW_UPDATE,
            flags: 0,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 4]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_window_update_zero_increment_stream_rejected() {
        // WINDOW_UPDATE with zero increment on stream is PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 4,
            frame_type: FRAME_WINDOW_UPDATE,
            flags: 0,
            stream_id: 1,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 4]);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Stream(_, code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Stream error, got {:?}", other),
        }
    }

    #[test]
    fn test_unknown_frame_type_ignored() {
        // Unknown frame types MUST be ignored (RFC 9113 Section 4.1).
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 10,
            frame_type: 0xFE, // Unknown type
            flags: 0,
            stream_id: 0,
        };
        let result = process_frame(&mut conn, &h, &[0u8; 10]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[test]
    fn test_headers_padded_exceeds_payload() {
        // HEADERS with padding length exceeding payload is PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        let h = FrameHeader {
            length: 3,
            frame_type: FRAME_HEADERS,
            flags: FLAG_PADDED | FLAG_END_HEADERS | FLAG_END_STREAM,
            stream_id: 1,
        };
        // Padding length byte = 100, but only 2 more bytes available
        let payload = [100, 0, 0];
        let result = process_frame(&mut conn, &h, &payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_data_padded_exceeds_payload() {
        // DATA with padding length exceeding payload is PROTOCOL_ERROR.
        let mut conn = H2Connection::new();
        // Create a stream first
        let mut encoder = HpackEncoder::new(4096);
        let headers = vec![
            (":method".to_string(), "POST".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let encoded = encoder.encode(&headers);
        let hdr = FrameHeader {
            length: encoded.len() as u32,
            frame_type: FRAME_HEADERS,
            flags: FLAG_END_HEADERS,
            stream_id: 1,
        };
        let _ = process_frame(&mut conn, &hdr, &encoded);

        // Now send DATA with bad padding
        let data_h = FrameHeader {
            length: 3,
            frame_type: FRAME_DATA,
            flags: FLAG_PADDED,
            stream_id: 1,
        };
        let payload = [100, 0, 0]; // pad_len=100 but only 2 bytes left
        let result = process_frame(&mut conn, &data_h, &payload);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_PROTOCOL_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }

    #[test]
    fn test_hpack_integer_overflow_rejected() {
        // HPACK integer with too many continuation bytes should be rejected.
        let data = [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]; // prefix=0xFF, then 5 continuation bytes
        let result = decode_integer(&data, 7);
        assert!(result.is_err());
    }

    #[test]
    fn test_hpack_truncated_string_rejected() {
        // HPACK string with length exceeding available data should be rejected.
        let data = [0x0A]; // length=10, but no data follows
        let result = decode_string(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_hpack_index_zero_rejected() {
        // HPACK index 0 is reserved and must be rejected.
        let mut decoder = HpackDecoder::new(4096);
        // 0x80 = indexed header, index = 0 (after stripping the prefix bit)
        // Actually, 0x80 with 7-bit prefix means value = 0, which is index 0
        let data = [0x80]; // indexed, index=0
        let result = decoder.decode(&data);
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_preface_too_short() {
        let data = b"PRI * HTTP/2.0";
        let mut cursor = std::io::Cursor::new(data.as_slice());
        let result = validate_connection_preface(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn test_connection_preface_wrong_content() {
        let data = b"PRI * HTTP/1.1\r\n\r\nSM\r\n\r\n"; // 1.1 instead of 2.0
        let mut cursor = std::io::Cursor::new(data.as_slice());
        let result = validate_connection_preface(&mut cursor);
        assert!(result.is_err());
    }

    #[test]
    fn test_frame_size_exceeds_max_rejected() {
        // read_frame should reject frames exceeding max_frame_size.
        let header = FrameHeader {
            length: DEFAULT_MAX_FRAME_SIZE + 1,
            frame_type: FRAME_DATA,
            flags: 0,
            stream_id: 1,
        };
        let header_bytes = header.serialize();
        let mut data = Vec::from(header_bytes);
        data.extend(vec![0u8; (DEFAULT_MAX_FRAME_SIZE + 1) as usize]);
        let mut cursor = std::io::Cursor::new(data);
        let result = read_frame(&mut cursor, DEFAULT_MAX_FRAME_SIZE);
        assert!(result.is_err());
        match result.unwrap_err() {
            H2Error::Connection(code, _) => {
                assert_eq!(code, ERROR_FRAME_SIZE_ERROR);
            }
            other => panic!("expected Connection error, got {:?}", other),
        }
    }
}
