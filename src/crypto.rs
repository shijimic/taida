//! SHA-256 digest (hand-written, no external crate).
//!
//! This module provides a streaming SHA-256 implementation for the
//! prebuild fetcher's integrity verification. It was extracted from
//! `src/interpreter/prelude.rs` so the hasher can be reused across
//! modules without depending on the interpreter.

use std::fmt::Write;

pub struct Sha256 {
    h: [u32; 8],
    buffer: Vec<u8>,
    total_len: u64,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            h: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: Vec::with_capacity(64),
            total_len: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u64;
        let mut block = [0u8; 64];
        let mut data = data;

        // Complete a block left partially filled by an earlier call.
        if !self.buffer.is_empty() {
            let take = (64 - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() < 64 {
                return;
            }
            block.copy_from_slice(&self.buffer);
            Self::process_block(&mut self.h, &block);
            self.buffer.clear();
        }

        // Hash whole blocks straight from the input and buffer only the
        // tail, so one call over a large input stays linear in its length.
        let mut blocks = data.chunks_exact(64);
        for chunk in &mut blocks {
            block.copy_from_slice(chunk);
            Self::process_block(&mut self.h, &block);
        }
        self.buffer.extend_from_slice(blocks.remainder());
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len * 8;
        self.buffer.push(0x80);
        while self.buffer.len() % 64 != 56 {
            self.buffer.push(0);
        }
        self.buffer.extend_from_slice(&bit_len.to_be_bytes());

        for block in self.buffer.chunks(64) {
            let mut padded = [0u8; 64];
            padded[..block.len()].copy_from_slice(block);
            Self::process_block(&mut self.h, &padded);
        }

        let mut out = [0u8; 32];
        for (i, word) in self.h.iter().enumerate() {
            out[i * 4..(i + 1) * 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    pub fn finalize_hex(self) -> String {
        let bytes = self.finalize();
        let mut out = String::with_capacity(64);
        for b in &bytes {
            let _ = write!(out, "{:02x}", b);
        }
        out
    }

    fn process_block(h: &mut [u32; 8], block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 4;
            *word = u32::from_be_bytes([block[j], block[j + 1], block[j + 2], block[j + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
}

/// Convenience: compute SHA-256 hex digest in one shot.
pub fn sha256_hex_bytes(input: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher.finalize_hex()
}

/// Raw 32-byte SHA-256 digest in one shot (used by HMAC).
pub fn sha256_bytes(input: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher.finalize()
}

// ── SHA-512 family (64-bit words, 1024-bit blocks) ─────────────────────
//
// SHA-512 / SHA-384 / SHA-224 share the SHA-2 construction but operate on a
// different word size or initial vector than SHA-256:
//   * SHA-512 / SHA-384 use 64-bit words, 128-byte (1024-bit) blocks, and a
//     16-byte length field. They differ only by the initial hash value and
//     the truncated output length (64 vs 48 bytes).
//   * SHA-224 reuses the SHA-256 core (32-bit words, 512-bit blocks) with a
//     different initial vector and a 28-byte truncated output.
// All implementations are hand-written with no external crate.

const SHA512_K: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

/// SHA-512 / SHA-384 streaming core. The output length (64 vs 48) and the
/// initial vector are chosen by the constructor.
struct Sha512Core {
    h: [u64; 8],
    buffer: Vec<u8>,
    total_len: u128,
    out_len: usize,
}

impl Sha512Core {
    fn new(iv: [u64; 8], out_len: usize) -> Self {
        Self {
            h: iv,
            buffer: Vec::with_capacity(128),
            total_len: 0,
            out_len,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.total_len += data.len() as u128;
        let mut block = [0u8; 128];
        let mut data = data;

        // Same scheme as `Sha256::update`: finish the pending block, then
        // hash whole blocks from the input and buffer only the tail.
        if !self.buffer.is_empty() {
            let take = (128 - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.buffer.len() < 128 {
                return;
            }
            block.copy_from_slice(&self.buffer);
            Self::process_block(&mut self.h, &block);
            self.buffer.clear();
        }

        let mut blocks = data.chunks_exact(128);
        for chunk in &mut blocks {
            block.copy_from_slice(chunk);
            Self::process_block(&mut self.h, &block);
        }
        self.buffer.extend_from_slice(blocks.remainder());
    }

    fn finalize(mut self) -> Vec<u8> {
        let bit_len = self.total_len.wrapping_mul(8);
        self.buffer.push(0x80);
        while self.buffer.len() % 128 != 112 {
            self.buffer.push(0);
        }
        self.buffer.extend_from_slice(&bit_len.to_be_bytes());

        for block in self.buffer.chunks(128) {
            let mut padded = [0u8; 128];
            padded[..block.len()].copy_from_slice(block);
            Self::process_block(&mut self.h, &padded);
        }

        let mut out = Vec::with_capacity(64);
        for word in &self.h {
            out.extend_from_slice(&word.to_be_bytes());
        }
        out.truncate(self.out_len);
        out
    }

    fn finalize_hex(self) -> String {
        bytes_to_hex(&self.finalize())
    }

    fn process_block(h: &mut [u64; 8], block: &[u8; 128]) {
        let mut w = [0u64; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 8;
            *word = u64::from_be_bytes([
                block[j],
                block[j + 1],
                block[j + 2],
                block[j + 3],
                block[j + 4],
                block[j + 5],
                block[j + 6],
                block[j + 7],
            ]);
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA512_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
}

const SHA512_IV: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

const SHA384_IV: [u64; 8] = [
    0xcbbb9d5dc1059ed8,
    0x629a292a367cd507,
    0x9159015a3070dd17,
    0x152fecd8f70e5939,
    0x67332667ffc00b31,
    0x8eb44a8768581511,
    0xdb0c2e0d64f98fa7,
    0x47b5481dbefa4fa4,
];

const SHA224_IV: [u32; 8] = [
    0xc1059ed8, 0x367cd507, 0x3070dd17, 0xf70e5939, 0xffc00b31, 0x68581511, 0x64f98fa7, 0xbefa4fa4,
];

/// Compute SHA-512 hex digest (128 chars) in one shot.
pub fn sha512_hex_bytes(input: &[u8]) -> String {
    let mut core = Sha512Core::new(SHA512_IV, 64);
    core.update(input);
    core.finalize_hex()
}

/// Compute SHA-384 hex digest (96 chars) in one shot.
pub fn sha384_hex_bytes(input: &[u8]) -> String {
    let mut core = Sha512Core::new(SHA384_IV, 48);
    core.update(input);
    core.finalize_hex()
}

/// Compute SHA-224 hex digest (56 chars) in one shot. Reuses the SHA-256
/// 32-bit core with the SHA-224 initial vector, truncating to 28 bytes.
pub fn sha224_hex_bytes(input: &[u8]) -> String {
    let mut hasher = Sha256 {
        h: SHA224_IV,
        buffer: Vec::with_capacity(64),
        total_len: 0,
    };
    hasher.update(input);
    let full = hasher.finalize();
    bytes_to_hex(&full[..28])
}

// ── HMAC-SHA256 (RFC 2104) ─────────────────────────────────────────────

/// HMAC-SHA256 hex digest (64 chars). Block size 64, ipad/opad per RFC 2104.
pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    const BLOCK: usize = 64;
    // Keys longer than the block size are hashed first.
    let mut key_block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let hashed = sha256_bytes(key);
        key_block[..32].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] = key_block[i] ^ 0x36;
        opad[i] = key_block[i] ^ 0x5c;
    }

    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(data);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(&inner_digest);
    outer.finalize_hex()
}

// ── Constant-time equality ─────────────────────────────────────────────

/// Constant-time byte-slice comparison. Returns `false` when the lengths
/// differ, but still walks the full length of `a` so the timing does not
/// depend on where a mismatch occurs within the compared region. The
/// length itself is not hidden (a length mismatch is observable).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    // Fold the length mismatch into the difference so that a length
    // mismatch can never short-circuit to an early `true`. The lengths
    // themselves are public (see above), so a plain comparison is fine;
    // only the byte walk below must stay constant-time.
    let mut diff: u8 = u8::from(a.len() != b.len());
    let n = a.len();
    for i in 0..n {
        // `b` is indexed modulo its length (with a guard for empty `b`) so
        // every byte of `a` participates in the comparison even when the
        // lengths differ.
        let bb = if b.is_empty() { 0u8 } else { b[i % b.len()] };
        diff |= a[i] ^ bb;
    }
    diff == 0
}

// ── Hex encode / decode ────────────────────────────────────────────────

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{:02x}", b);
    }
    out
}

/// Lowercase hex encoding.
pub fn hex_encode(data: &[u8]) -> String {
    bytes_to_hex(data)
}

/// Hex decoding. Accepts upper- or lower-case hex; rejects odd length and
/// non-hex characters by returning `None` (failure side of `Lax[Bytes]`).
pub fn hex_decode(hex: &str) -> Option<Vec<u8>> {
    let bytes = hex.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

// ── Base64 encode / decode (RFC 4648 standard alphabet + padding) ──────

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 encoding with `=` padding (RFC 4648).
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut chunks = data.chunks_exact(3);
    for chunk in &mut chunks {
        let n = ((chunk[0] as u32) << 16) | ((chunk[1] as u32) << 8) | (chunk[2] as u32);
        out.push(B64_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        out.push(B64_ALPHABET[(n & 0x3f) as usize] as char);
    }
    let rem = chunks.remainder();
    match rem.len() {
        1 => {
            let n = (rem[0] as u32) << 16;
            out.push(B64_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(B64_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let n = ((rem[0] as u32) << 16) | ((rem[1] as u32) << 8);
            out.push(B64_ALPHABET[((n >> 18) & 0x3f) as usize] as char);
            out.push(B64_ALPHABET[((n >> 12) & 0x3f) as usize] as char);
            out.push(B64_ALPHABET[((n >> 6) & 0x3f) as usize] as char);
            out.push('=');
        }
        _ => {}
    }
    out
}

/// Standard base64 decoding (RFC 4648). Rejects invalid alphabet bytes,
/// malformed padding, and lengths that are not a multiple of 4 by returning
/// `None` (failure side of `Lax[Bytes]`). Whitespace is not accepted.
pub fn base64_decode(b64: &str) -> Option<Vec<u8>> {
    let bytes = b64.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let n_chunks = bytes.len() / 4;
    for c in 0..n_chunks {
        let q = &bytes[c * 4..c * 4 + 4];
        let is_last = c == n_chunks - 1;
        // Padding is only legal as the trailing 1-2 chars of the final quad.
        let pad = q.iter().filter(|&&b| b == b'=').count();
        if pad > 0 && !is_last {
            return None;
        }
        if pad > 2 {
            return None;
        }
        // Padding must be trailing-only: 1 pad => only q[3]; 2 pad => q[2],q[3].
        if (pad == 1 && q[3] != b'=') || (pad == 2 && (q[2] != b'=' || q[3] != b'=')) {
            return None;
        }
        let v0 = b64_val(q[0])?;
        let v1 = b64_val(q[1])?;
        let n_data = 4 - pad;
        let v2 = if n_data > 2 { b64_val(q[2])? } else { 0 };
        let v3 = if n_data > 3 { b64_val(q[3])? } else { 0 };
        let triple = (v0 as u32) << 18 | (v1 as u32) << 12 | (v2 as u32) << 6 | (v3 as u32);
        out.push(((triple >> 16) & 0xff) as u8);
        if n_data >= 3 {
            out.push(((triple >> 8) & 0xff) as u8);
        }
        if n_data >= 4 {
            out.push((triple & 0xff) as u8);
        }
    }
    Some(out)
}

fn b64_val(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

// ── Random bytes (OS entropy) ──────────────────────────────────────────

/// Read `n` cryptographically secure random bytes from the OS entropy
/// source. Returns an error string on failure so callers can surface a
/// throw to the Taida surface.
pub fn random_bytes(n: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut buf = Vec::new();
    if buf.try_reserve_exact(n).is_err() {
        return Err(format!("randomBytes: cannot allocate {} bytes", n));
    }
    buf.resize(n, 0);
    if n == 0 {
        return Ok(buf);
    }
    let mut file = std::fs::File::open("/dev/urandom")
        .map_err(|e| format!("randomBytes: failed to open OS entropy source: {}", e))?;
    file.read_exact(&mut buf)
        .map_err(|e| format!("randomBytes: failed to read OS entropy source: {}", e))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_empty_string() {
        let empty = [0u8; 0];
        let mut hasher = Sha256::new();
        hasher.update(&empty);
        let h = hasher.finalize_hex();
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_hello_world() {
        let mut hasher = Sha256::new();
        hasher.update(b"hello world");
        assert_eq!(
            hasher.finalize_hex(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn sha256_streaming_update() {
        let mut hasher = Sha256::new();
        hasher.update(b"hello");
        hasher.update(b" ");
        hasher.update(b"world");
        let hex = hasher.finalize_hex();

        let mut hasher2 = Sha256::new();
        hasher2.update(b"hello world");
        assert_eq!(hex, hasher2.finalize_hex());
    }

    #[test]
    fn sha256_larger_than_block() {
        let data = vec![0xABu8; 128];
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let hex = hasher.finalize_hex();
        // Verify against one-shot
        let mut hasher2 = Sha256::new();
        hasher2.update(&data);
        assert_eq!(hex, hasher2.finalize_hex());
    }

    // ── SHA-512 / 384 / 224 known vectors (NIST) ───────────────────────

    #[test]
    fn sha512_empty_and_abc() {
        assert_eq!(
            sha512_hex_bytes(b""),
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        );
        assert_eq!(
            sha512_hex_bytes(b"abc"),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
    }

    #[test]
    fn sha512_two_block() {
        // 112-byte message crosses into a second 1024-bit block.
        assert_eq!(
            sha512_hex_bytes(
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmno\
                  ijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu"
            ),
            "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018\
             501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"
        );
    }

    #[test]
    fn sha384_empty_and_abc() {
        assert_eq!(
            sha384_hex_bytes(b""),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be0743\
             4c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
        );
        assert_eq!(
            sha384_hex_bytes(b"abc"),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded163\
             1a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
    }

    #[test]
    fn sha224_empty_and_abc() {
        assert_eq!(
            sha224_hex_bytes(b""),
            "d14a028c2a3a2bc9476102bb288234c415a2b01f828ea62ac5b3e42f"
        );
        assert_eq!(
            sha224_hex_bytes(b"abc"),
            "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7"
        );
    }

    #[test]
    fn sha_streaming_matches_one_shot() {
        let mut c = Sha512Core::new(SHA512_IV, 64);
        c.update(b"hello");
        c.update(b" ");
        c.update(b"world");
        assert_eq!(c.finalize_hex(), sha512_hex_bytes(b"hello world"));
    }

    #[test]
    fn sha_split_points_match_one_shot() {
        // Every split of inputs around the block sizes must give the same
        // digest as hashing the whole input at once.
        let data: Vec<u8> = (0..300u32).map(|i| (i * 7 + 3) as u8).collect();
        for len in [0usize, 1, 63, 64, 65, 127, 128, 129, 191, 255, 256, 300] {
            let input = &data[..len];
            let one_shot_256 = sha256_hex_bytes(input);
            let one_shot_512 = sha512_hex_bytes(input);
            for split in 0..=len {
                for second in [split, (split + len) / 2] {
                    let (a, rest) = input.split_at(split);
                    let (b, c) = rest.split_at(second - split);
                    let mut h256 = Sha256::new();
                    let mut h512 = Sha512Core::new(SHA512_IV, 64);
                    for part in [a, b, c] {
                        h256.update(part);
                        h512.update(part);
                    }
                    assert_eq!(h256.finalize_hex(), one_shot_256, "len={len} split={split}");
                    assert_eq!(h512.finalize_hex(), one_shot_512, "len={len} split={split}");
                }
            }
        }
    }

    #[test]
    fn sha_million_a_vectors() {
        // NIST long-message vectors; also a one-call input far larger
        // than one block.
        let data = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256_hex_bytes(&data),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        assert_eq!(
            sha224_hex_bytes(&data),
            "20794655980c91d8bbb4c1ea97618a4bf03f42581948b2ee4ee7ad67"
        );
        assert_eq!(
            sha512_hex_bytes(&data),
            "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973eb\
             de0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"
        );
        assert_eq!(
            sha384_hex_bytes(&data),
            "9d0e1809716474cb086e834e310a4a1ced149e9c00f24852\
             7972cec5704c2a5b07b8b3dc38ecc4ebae97ddd87f3d8985"
        );
    }

    // ── HMAC-SHA256 RFC 4231 ───────────────────────────────────────────

    #[test]
    fn hmac_sha256_rfc4231_case1() {
        // Key = 0x0b * 20, Data = "Hi There"
        let key = vec![0x0bu8; 20];
        assert_eq!(
            hmac_sha256_hex(&key, b"Hi There"),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_sha256_rfc4231_case2() {
        // Key = "Jefe", Data = "what do ya want for nothing?"
        assert_eq!(
            hmac_sha256_hex(b"Jefe", b"what do ya want for nothing?"),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn hmac_sha256_rfc4231_case3_long_key() {
        // Case 6: key larger than block size (131 bytes of 0xaa).
        let key = vec![0xaau8; 131];
        assert_eq!(
            hmac_sha256_hex(
                &key,
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            ),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    // ── constant_time_eq ───────────────────────────────────────────────

    #[test]
    fn constant_time_eq_basic() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(!constant_time_eq(b"ab", b"abc"));
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"", b"x"));
        assert!(!constant_time_eq(b"x", b""));
    }

    /// Regression: a shift-based length fold used to drop bits 24-31 /
    /// 40-63 of the length XOR, so inputs whose length difference sat
    /// only in those bits (e.g. 0 vs 2^24 bytes) compared *equal* — with
    /// an empty `a` the byte walk never runs, so the verdict came from
    /// the (broken) fold alone. The same shape doubled as an HMAC
    /// verification bypass: a 32-byte expected MAC vs "same 32-byte
    /// prefix + 2^24 padding" walked only the matching prefix.
    #[test]
    fn constant_time_eq_length_fold_regression() {
        let big = vec![0u8; 1 << 24];
        assert!(!constant_time_eq(b"", &big));
        assert!(!constant_time_eq(&big, b""));

        let prefix = [0xabu8; 32];
        let mut padded = vec![0xabu8; 32];
        padded.resize(32 + (1 << 24), 0u8);
        assert!(!constant_time_eq(&prefix, &padded));
    }

    // ── hex round-trip + RFC vectors ───────────────────────────────────

    #[test]
    fn hex_encode_decode_roundtrip() {
        assert_eq!(hex_encode(b""), "");
        assert_eq!(hex_encode(&[0x00, 0xff, 0x10, 0xab]), "00ff10ab");
        assert_eq!(
            hex_decode("00ff10ab").unwrap(),
            vec![0x00, 0xff, 0x10, 0xab]
        );
        // Uppercase accepted on decode.
        assert_eq!(
            hex_decode("00FF10AB").unwrap(),
            vec![0x00, 0xff, 0x10, 0xab]
        );
        assert_eq!(hex_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn hex_decode_rejects_invalid() {
        assert!(hex_decode("abc").is_none()); // odd length
        assert!(hex_decode("zz").is_none()); // non-hex
        assert!(hex_decode("0g").is_none());
    }

    // ── base64 round-trip + RFC 4648 vectors ───────────────────────────

    #[test]
    fn base64_rfc4648_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");

        assert_eq!(base64_decode("").unwrap(), b"");
        assert_eq!(base64_decode("Zg==").unwrap(), b"f");
        assert_eq!(base64_decode("Zm8=").unwrap(), b"fo");
        assert_eq!(base64_decode("Zm9v").unwrap(), b"foo");
        assert_eq!(base64_decode("Zm9vYg==").unwrap(), b"foob");
        assert_eq!(base64_decode("Zm9vYmE=").unwrap(), b"fooba");
        assert_eq!(base64_decode("Zm9vYmFy").unwrap(), b"foobar");
    }

    #[test]
    fn base64_decode_rejects_invalid() {
        assert!(base64_decode("Zm9").is_none()); // length not multiple of 4
        assert!(base64_decode("Zm9*").is_none()); // invalid char
        assert!(base64_decode("=Zm9").is_none()); // misplaced padding
        assert!(base64_decode("Z===").is_none()); // too much padding
    }

    #[test]
    fn base64_binary_roundtrip() {
        let data: Vec<u8> = (0u8..=255).collect();
        let encoded = base64_encode(&data);
        assert_eq!(base64_decode(&encoded).unwrap(), data);
    }

    // ── random_bytes smoke ─────────────────────────────────────────────

    #[test]
    fn random_bytes_length_and_nondeterminism() {
        let a = random_bytes(32).expect("entropy available");
        assert_eq!(a.len(), 32);
        let b = random_bytes(32).expect("entropy available");
        // Two 32-byte reads colliding is cryptographically impossible.
        assert_ne!(a, b);
        assert_eq!(random_bytes(0).unwrap(), Vec::<u8>::new());
    }
}
