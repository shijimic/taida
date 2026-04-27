/// Runtime values for Taida Lang.
///
/// Every type has a default value. There is no null/undefined.
/// All values are immutable — operations return new values.
use std::collections::HashMap;
use std::fmt;
use std::ops::Deref;
use std::sync::{Arc, Mutex, OnceLock};

use crate::parser::{FieldDef, Param, Statement};

use super::runtime::str_rope::GapBuffer;

/// D29B-016 / Phase 10-B (Track-θ): threshold above which `concat_with` and
/// the `+` operator promote a Flat string to the Rope path.
///
/// Lock-K verdict V-3 confirmed 1024 bytes — small enough that LineEditor's
/// keystroke-by-keystroke growth crosses the threshold within a few dozen
/// inserts and starts amortising mutation cost over the rope's gap buffer.
pub const STR_ROPE_PROMOTION_THRESHOLD: usize = 1024;

/// # C26B-018 (A) / Round 8 wU (2026-04-24): char-index cache layer
///
/// A string value with a lazy char-boundary cache. The `data` field holds
/// the UTF-8 payload; `char_offsets` is populated on first access to a
/// char-indexed operation (`char_at` / `char_count` / `char_slice`) and
/// maps each char index `i` to its byte offset in `data`. Subsequent
/// char-indexed access is then O(1) instead of O(n).
///
/// This type is wrapped by `Value::Str(Arc<StrValue>)`, so:
/// - cloning a `Value::Str` still costs one atomic increment (Round 6 wP
///   foundation preserved);
/// - the cache is shared across all Arc clones and only ever computed once;
/// - the public surface (ABI, error strings, test fixtures) is unchanged
///   because `StrValue` implements `Deref<Target = String>`, so
///   `s.chars()`, `s.len()`, `s.as_str()`, `s.as_ptr()`, `&s[..]`,
///   `format!("{}", s)`, `a == b`, `a < b`, etc. all continue to work
///   transparently.
///
/// `OnceLock` gives us thread-safe lock-free reads of the cache after the
/// first write (single `Acquire` load), which matches Taida's
/// immutable-first execution model and the Cluster 4 abstraction pin
/// (Arc + try_unwrap COW, Round 3 wG LOCKED).
/// D29B-016 / Phase 10-B (Track-θ): internal storage variant for `StrValue`.
///
/// Lock-K V-1 verdict (Option A) is implemented as **interior wrapping**
/// (DEVIATION matching Track-ε commit `e179238`) rather than a new
/// `Value::StrRope` outer enum variant — the latter would have required
/// updating ~146 `Value::Str(_)` match arms across the codebase, with the
/// same silent-bug risk Track-ε identified for Bytes (47 sites).
///
/// * `Flat(String)` — legacy path, also the default for short strings.
/// * `Rope(Box<RopeBuffer>)` — gap-buffer storage used after a `concat_with`
///   crosses `STR_ROPE_PROMOTION_THRESHOLD`. Rope reads (`as_str`,
///   `as_string`, `cached_char_*`) lazily flatten via the inner `OnceLock`.
#[derive(Debug)]
pub enum StrRepr {
    Flat(String),
    Rope(Box<RopeBuffer>),
}

/// D29B-016 / Phase 10-B: rope storage backing a `StrRepr::Rope` variant.
///
/// Wraps the raw `GapBuffer` (defined in `super::runtime::str_rope`) with a
/// `OnceLock<String>` that caches the flat representation on first read.
/// Subsequent `as_str()` / `cached_char_*` reads return slices into this
/// cache for `&str` lifetimes that match the `&self` borrow.
///
/// The cache is populated lazily and discarded if the buffer is mutated
/// (mutation APIs reside on `StrValue` and reset both caches).
#[derive(Debug)]
pub struct RopeBuffer {
    buf: GapBuffer,
    /// Lazily-cached flat `String`. Filled on first `flatten_cached`.
    flat_cache: OnceLock<String>,
}

impl RopeBuffer {
    /// Construct a fresh rope seeded from `initial`. Distinct from
    /// `std::str::FromStr::from_str` (different return shape) — clippy
    /// warns about confusion, but we keep this name for symmetry with
    /// `GapBuffer::from_str`. Allow attribute applied locally.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(initial: &str) -> Self {
        RopeBuffer {
            buf: GapBuffer::from_str(initial),
            flat_cache: OnceLock::new(),
        }
    }

    /// Construct a rope by concatenating two segments. Used by the
    /// `concat_with` promotion path.
    pub fn from_concat(left: &str, right: &str) -> Self {
        let mut buf = GapBuffer::from_str_with_gap(left, right.len() + 256);
        buf.insert_at_byte(left.len(), right);
        RopeBuffer {
            buf,
            flat_cache: OnceLock::new(),
        }
    }

    /// Borrow the underlying gap buffer (read-only access).
    #[inline]
    pub fn buf(&self) -> &GapBuffer {
        &self.buf
    }

    /// Effective byte length.
    #[inline]
    pub fn byte_len(&self) -> usize {
        self.buf.byte_len()
    }

    /// Flatten on demand and cache for subsequent reads. The returned
    /// `&str` has the lifetime of `&self`.
    #[inline]
    pub fn flatten_cached(&self) -> &str {
        self.flat_cache.get_or_init(|| self.buf.flatten()).as_str()
    }
}

impl Clone for RopeBuffer {
    fn clone(&self) -> Self {
        // Cloning a rope flattens it into a fresh Flat-style seed; the
        // resulting RopeBuffer drops both caches. Outer `Arc<StrValue>`
        // sharing means this path is only hit when a consumer already
        // holds a uniquely-owned StrValue and calls Clone explicitly.
        RopeBuffer::from_str(self.flatten_cached())
    }
}

#[derive(Debug)]
pub struct StrValue {
    /// The UTF-8 payload (Flat or Rope; see `StrRepr`).
    ///
    /// Phase 10-B (D29B-016) replaced the prior `data: String` field with
    /// this enum. The change is internal — `as_str()` / `as_string()` /
    /// `cached_char_*` keep their previous signatures and call sites are
    /// unaffected.
    data: StrRepr,
    /// Lazily-populated char-index → byte-offset table.
    ///
    /// When present, `char_offsets[i]` is the byte offset of the `i`-th
    /// `char` in the flattened payload, and `char_offsets.len()` equals
    /// the total char count plus one sentinel entry (== byte length) so
    /// that `char_offsets[i+1] - char_offsets[i]` gives the byte width of
    /// the `i`-th char without a separate bound check.
    ///
    /// For `StrRepr::Rope`, the table is computed against the rope's
    /// `flatten_cached()` output, so accessing this field implicitly
    /// triggers flatten if not already cached.
    char_offsets: OnceLock<Vec<usize>>,
}

impl StrValue {
    /// Construct a new `StrValue` without populating the cache. Uses the
    /// `StrRepr::Flat` variant (legacy path).
    pub fn new(data: String) -> Self {
        StrValue {
            data: StrRepr::Flat(data),
            char_offsets: OnceLock::new(),
        }
    }

    /// D29B-016 / Phase 10-B: construct a `StrValue` whose internal
    /// representation is a rope. Caller is asserting the rope path is
    /// beneficial — most call sites should use `concat_with` instead and
    /// let the threshold heuristic decide.
    pub fn from_rope(rope: RopeBuffer) -> Self {
        StrValue {
            data: StrRepr::Rope(Box::new(rope)),
            char_offsets: OnceLock::new(),
        }
    }

    /// True iff the storage is the rope path. Intended for tests and the
    /// LineEditor promotion verification (Phase 10-G).
    #[inline]
    pub fn is_rope(&self) -> bool {
        matches!(self.data, StrRepr::Rope(_))
    }

    /// D29B-016 / Phase 10-B: concatenate with `other` and decide whether
    /// to keep the result Flat or promote to Rope.
    ///
    /// Promotion rules (Lock-K V-3 = 1024 bytes):
    /// * Either operand is already Rope → result is Rope.
    /// * Combined byte length >= `STR_ROPE_PROMOTION_THRESHOLD` → Rope.
    /// * Otherwise → Flat (legacy fast path).
    pub fn concat_with(&self, other: &StrValue) -> StrValue {
        let left_is_rope = matches!(self.data, StrRepr::Rope(_));
        let right_is_rope = matches!(other.data, StrRepr::Rope(_));
        let combined_len = self.as_str().len() + other.as_str().len();
        if left_is_rope || right_is_rope || combined_len >= STR_ROPE_PROMOTION_THRESHOLD {
            StrValue::from_rope(RopeBuffer::from_concat(self.as_str(), other.as_str()))
        } else {
            let mut s = String::with_capacity(combined_len);
            s.push_str(self.as_str());
            s.push_str(other.as_str());
            StrValue::new(s)
        }
    }

    /// Borrow the UTF-8 payload as `&str`. For `StrRepr::Rope`, this
    /// triggers a one-time flatten + cache; subsequent calls reuse the
    /// cached flat representation.
    #[inline]
    pub fn as_str(&self) -> &str {
        match &self.data {
            StrRepr::Flat(s) => s.as_str(),
            StrRepr::Rope(rope) => rope.flatten_cached(),
        }
    }

    /// Borrow the underlying string as a cached owned `&String`.
    ///
    /// For `StrRepr::Flat`, this returns the inner `String` directly. For
    /// `StrRepr::Rope`, this returns a reference into the OnceLock-cached
    /// flatten output (so the call site retains its `&String` lifetime
    /// invariant). The first rope call triggers the flatten.
    #[inline]
    pub fn as_string(&self) -> &String {
        match &self.data {
            StrRepr::Flat(s) => s,
            StrRepr::Rope(rope) => {
                // flatten_cached returns &str backed by the cache; we need
                // the &String form for callers that match the legacy
                // signature. Reach into the OnceLock to obtain it.
                rope.flat_cache.get_or_init(|| rope.buf.flatten())
            }
        }
    }

    /// Consume this `StrValue` and return the owned `String`. For Rope,
    /// this either reuses the cached flatten or computes a fresh one.
    #[inline]
    pub fn into_string(self) -> String {
        match self.data {
            StrRepr::Flat(s) => s,
            StrRepr::Rope(rope) => {
                let RopeBuffer { buf, flat_cache } = *rope;
                if let Some(cached) = flat_cache.into_inner() {
                    cached
                } else {
                    buf.flatten()
                }
            }
        }
    }

    /// Get the char-offset table, computing and caching it on first call.
    ///
    /// The returned slice has `char_count + 1` entries: indices `0..N` are
    /// the byte offsets of each char, and the final entry is the total
    /// byte length (a sentinel that simplifies width computation).
    ///
    /// For `StrRepr::Rope`, this implicitly triggers a flatten + cache
    /// (the table is computed against the flat representation).
    #[inline]
    fn offsets(&self) -> &[usize] {
        self.char_offsets.get_or_init(|| {
            // Reserve char_count + 1 entries (sentinel). Byte length is a
            // safe upper bound because each char is at least 1 byte in
            // UTF-8, so char_count ≤ byte_len.
            let payload = self.as_str();
            let byte_len = payload.len();
            let mut table = Vec::with_capacity(byte_len + 1);
            for (byte_idx, _) in payload.char_indices() {
                table.push(byte_idx);
            }
            table.push(byte_len); // sentinel
            table.shrink_to_fit();
            table
        })
    }

    /// O(1) after first call: total number of Unicode scalar values (chars).
    #[inline]
    pub fn cached_char_count(&self) -> usize {
        // Sentinel entry is present, so subtract 1.
        self.offsets().len().saturating_sub(1)
    }

    /// O(1) after first call: get the char at index `idx` as an owned
    /// single-char `String`. Returns `None` if `idx >= char_count`.
    ///
    /// Note: callers that receive a signed integer from user code must
    /// bounds-check against negative values themselves before casting to
    /// `usize` — wU Round 8 bug fix: the original draft used `idx + 1 >=
    /// len` which overflowed when `idx == usize::MAX` (the result of
    /// casting `-1i64 as usize`). Saturating arithmetic below keeps this
    /// defensive.
    #[inline]
    pub fn cached_char_at(&self, idx: usize) -> Option<String> {
        let offs = self.offsets();
        // sentinel means offs.len() = char_count + 1 (when non-empty).
        // Use saturating addition so usize::MAX + 1 → usize::MAX, which
        // is unambiguously >= offs.len() and returns None.
        if idx.saturating_add(1) >= offs.len() {
            return None;
        }
        let start = offs[idx];
        let end = offs[idx + 1];
        Some(self.as_str()[start..end].to_string())
    }

    /// O(1) after first call: slice `self` by char indices and return the
    /// substring as an owned `String`. Bounds are clamped to `[0, char_count]`
    /// and `start <= end` is assumed by the caller (caller should invoke
    /// `clamp_slice_bounds` first; this helper does not clamp).
    #[inline]
    pub fn cached_char_slice(&self, start: usize, end: usize) -> String {
        let offs = self.offsets();
        let last = offs.len().saturating_sub(1); // char_count
        let s = start.min(last);
        let e = end.min(last);
        if s >= e {
            return String::new();
        }
        let byte_start = offs[s];
        let byte_end = offs[e];
        self.as_str()[byte_start..byte_end].to_string()
    }

    /// O(1) after first call: convert a byte offset that lies on a char
    /// boundary into its char index. Returns `None` if `byte_pos` is not
    /// on a boundary. Used by `indexOf` / `lastIndexOf` to translate
    /// `str::find` results.
    #[inline]
    pub fn cached_byte_to_char_index(&self, byte_pos: usize) -> Option<usize> {
        let offs = self.offsets();
        offs.binary_search(&byte_pos).ok()
    }
}

impl Deref for StrValue {
    /// Phase 10-B (D29B-016): Deref keeps the legacy `&String` target so
    /// that all 146 `Value::Str(s)` sites continue to compile unchanged.
    /// For `StrRepr::Rope`, this triggers a one-time flatten + cache via
    /// `as_string()`.
    type Target = String;
    #[inline]
    fn deref(&self) -> &String {
        self.as_string()
    }
}

impl Clone for StrValue {
    fn clone(&self) -> Self {
        // Cloning a `StrValue` drops the cache (it will be recomputed on
        // demand). This path is only hit when the outer `Arc` is not
        // uniquely owned and a consumer calls `Arc::try_unwrap` followed
        // by an explicit clone. The common case — `Value::clone` on
        // `Value::Str` — is an `Arc::clone` and does NOT invoke this.
        //
        // Phase 10-B: a Rope clone flattens to Flat (semantically equal,
        // simpler invariant for downstream consumers).
        StrValue::new(self.as_str().to_string())
    }
}

impl PartialEq for StrValue {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for StrValue {}

impl PartialOrd for StrValue {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StrValue {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl fmt::Display for StrValue {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl std::hash::Hash for StrValue {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state)
    }
}

impl From<String> for StrValue {
    #[inline]
    fn from(s: String) -> Self {
        StrValue::new(s)
    }
}

impl From<&str> for StrValue {
    #[inline]
    fn from(s: &str) -> Self {
        StrValue::new(s.to_string())
    }
}

impl Default for StrValue {
    #[inline]
    fn default() -> Self {
        StrValue::new(String::new())
    }
}

impl AsRef<str> for StrValue {
    #[inline]
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<std::ffi::OsStr> for StrValue {
    #[inline]
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.as_str().as_ref()
    }
}

impl std::borrow::Borrow<str> for StrValue {
    #[inline]
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

/// # D29B-004 / Track-ε (2026-04-27): Bytes view value
///
/// `BytesValue` represents a (possibly sub-range) view over an `Arc<Vec<u8>>`
/// byte buffer. The view is defined by `offset` and `len` into `buf`.
///
/// # Zero-copy invariants
///
/// - `Slice[bytes(b), s, e]` returns a new `BytesValue` with the *same* `buf`
///   `Arc` (verified by `Arc::ptr_eq`) and adjusted `offset`/`len`.
/// - The byte slice is `&buf[offset..offset+len]`, returned by `as_slice()`.
/// - Cloning `Arc<BytesValue>` is O(1) atomic; cloning `BytesValue` itself
///   bumps the inner `Arc<Vec<u8>>` rcount (one atomic).
///
/// # Equality / ordering / hashing
///
/// All comparisons use the byte slice (`as_slice()`), so two BytesValues with
/// different `buf` Arcs but the same byte content compare equal. This matches
/// the pre-D29 contract of `Value::Bytes` equality on `Vec<u8>` content.
#[derive(Debug, Clone)]
pub struct BytesValue {
    /// The underlying byte buffer. Shared via `Arc::clone` across views.
    pub buf: Arc<Vec<u8>>,
    /// Byte offset of the view start within `buf`.
    pub offset: usize,
    /// Byte length of the view.
    pub len: usize,
}

impl BytesValue {
    /// An empty Bytes view (offset=0, len=0, fresh empty Arc).
    pub fn empty() -> Self {
        BytesValue {
            buf: Arc::new(Vec::new()),
            offset: 0,
            len: 0,
        }
    }

    /// Borrow the view as a byte slice. Always returns
    /// `&buf[offset..offset+len]`.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[self.offset..self.offset + self.len]
    }

    /// Length of the view (NOT the underlying buffer).
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// True iff the view length is zero.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Iterator over the bytes in the view.
    #[inline]
    pub fn iter(&self) -> std::slice::Iter<'_, u8> {
        self.as_slice().iter()
    }

    /// True iff this view shares its underlying `buf` Arc with `other`.
    /// Used by D29B-004 zero-copy regression tests to confirm that
    /// `Slice[bytes]` returns a view rather than a deep copy.
    #[inline]
    pub fn shares_buf_with(&self, other: &BytesValue) -> bool {
        Arc::ptr_eq(&self.buf, &other.buf)
    }
}

impl Deref for BytesValue {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl PartialEq for BytesValue {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl Eq for BytesValue {}

impl PartialOrd for BytesValue {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BytesValue {
    #[inline]
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl std::hash::Hash for BytesValue {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state);
    }
}

/// A runtime value in Taida.
#[derive(Debug, Clone)]
pub enum Value {
    /// Integer value
    Int(i64),
    /// Floating-point value
    Float(f64),
    /// String value.
    ///
    /// # C26B-018 (A) / Round 8 wU (2026-04-24): char-index cache layer
    ///
    /// The interior `StrValue` holds the UTF-8 payload plus a
    /// lazily-computed char-offset table so that `CharAt` / `Slice` /
    /// `length` / `get` / `indexOf` / `lastIndexOf` amortize to O(1) per
    /// call after the cache is populated (previously O(n) per call due to
    /// `chars().nth(idx)` / `chars().count()`). The cache is stored in an
    /// `OnceLock<Vec<usize>>`, giving thread-safe lock-free reads after
    /// the first write — matching Taida's immutable-first model.
    ///
    /// Wrapping `StrValue` in `Arc` preserves the Round 6 wP foundation
    /// (O(1) clone via one atomic increment) and lets the cache be shared
    /// across all clones of a given string.
    ///
    /// * **Construction**: prefer [`Value::str`] (wraps in `Arc::new`).
    /// * **Reading from a match binding**: `s.as_str()` / `&s[..]` /
    ///   `s.chars()` / `s.len()` / `s.is_empty()` / `s.as_ptr()` /
    ///   `format!("{}", s)` work via `Deref<Target = String>`.
    /// * **Char-indexed fast paths**: call `s.cached_char_count()`,
    ///   `s.cached_char_at(idx)`, `s.cached_char_slice(start, end)`,
    ///   `s.cached_byte_to_char_index(byte_pos)` — O(1) after first
    ///   touch.
    /// * **Consuming the inner `String`**: use [`Value::str_take`] —
    ///   `Arc::try_unwrap` fast path (drops the cache), else
    ///   `(*arc).clone_into_string()` fallback.
    ///
    /// Equality, ordering, display, hashing semantics are unchanged
    /// because `StrValue` forwards all of them to its inner `String`.
    Str(Arc<StrValue>),
    /// Bytes value (immutable byte sequence).
    ///
    /// # D29B-004 / Track-ε (2026-04-27): interior `Arc<BytesValue>`
    ///
    /// Migrated from `Arc<Vec<u8>>` to `Arc<BytesValue { buf, offset, len }>`
    /// so that `Slice[bytes]` can return a **zero-copy view** sharing the
    /// underlying byte buffer (`Arc::ptr_eq(&result.buf, &source.buf) == true`)
    /// while still expressing a sub-range. This eliminates the per-request
    /// body memcpy in the 1-arg handler hot path
    /// (`body <= Slice[req.raw, body.start, body.start + body.len]`).
    ///
    /// # C26B-020 柱 2 / Round 5 wO (2026-04-24): interior Arc origin
    ///
    /// The original migration from plain `Vec<u8>` to `Arc<Vec<u8>>` ensured
    /// that `Value::clone()` on Bytes is one atomic increment instead of a
    /// full byte-by-byte deep-clone. This Track-ε wrapping preserves that
    /// property — cloning `Value::Bytes` now bumps the outer `Arc<BytesValue>`
    /// rcount; the inner `Arc<Vec<u8>>` is shared via the BytesValue field.
    ///
    /// * **Construction**: prefer [`Value::bytes`] (full-buffer view) or
    ///   [`Value::bytes_view`] (sub-range view sharing an existing buf Arc).
    /// * **Reading from a match binding**: `b.iter()` / `b.len()` /
    ///   `b.is_empty()` / `b.as_slice()` work as `BytesValue` inherent
    ///   methods; the slice is `&buf[offset..offset+len]`.
    /// * **Consuming bytes**: use [`Value::bytes_take`] — Arc try_unwrap
    ///   fast path on the outer BytesValue, else returns `as_slice().to_vec()`.
    ///
    /// Equality, ordering, display semantics are based on the byte slice
    /// `&buf[offset..offset+len]`, so Bytes from different buf Arcs with the
    /// same byte content compare equal. Hashing follows the same rule.
    Bytes(Arc<BytesValue>),
    /// Boolean value
    Bool(bool),
    /// Buchi pack (named fields, ordered).
    ///
    /// # C26B-012 / Round 6 wQ (2026-04-24): interior `Arc<Vec<(String, Value)>>`
    ///
    /// Migrated from plain `Vec<(String, Value)>` to
    /// `Arc<Vec<(String, Value)>>` so that `Value::clone()` on a pack becomes
    /// an `Arc::clone()` (one atomic increment) instead of a full
    /// field-by-field deep-clone. This follows the same Cluster 4 abstraction
    /// pattern (Arc + try_unwrap COW, LOCKED in Round 3 wG) applied to
    /// `Value::List` (Phase 5-F2-1) and `Value::Bytes` (Round 5 wO).
    ///
    /// Pattern-match bindings such as `Value::BuchiPack(fields)` now yield
    /// `fields: Arc<Vec<(String, Value)>>`, which derefs transparently to
    /// `&Vec<(String, Value)>` for read operations.
    ///
    /// * **Construction**: prefer [`Value::pack`] (wraps in `Arc::new`).
    /// * **Iteration from a match binding**: `fields.iter()` works via deref;
    ///   `for f in &**fields` and `for f in fields.as_ref()` are both valid.
    /// * **Mutation with COW**: `Arc::make_mut(&mut inner)` clones if shared,
    ///   otherwise mutates in place.
    /// * **Consuming the inner `Vec<(String, Value)>`**: use
    ///   [`Value::pack_take`] — `Arc::try_unwrap` fast path, else
    ///   `(*arc).clone()` fallback.
    ///
    /// Equality, ordering, display, hashing semantics are unchanged because
    /// `Arc<T>` transparently forwards read access to `T`.
    BuchiPack(Arc<Vec<(String, Value)>>),
    /// List of values.
    ///
    /// # C25B-029 / Phase 5-F2-1 (2026-04-23): interior `Arc<Vec<Value>>`
    ///
    /// The interior was migrated from plain `Vec<Value>` to
    /// `Arc<Vec<Value>>` so that `Value::clone()` on a list becomes an
    /// `Arc::clone()` (one atomic increment) instead of a full
    /// element-by-element deep-clone. Pattern-match bindings such as
    /// `Value::List(items)` now yield `items: Arc<Vec<Value>>`, which
    /// derefs transparently to `&Vec<Value>` for read operations.
    ///
    /// * **Construction**: prefer [`Value::list`] (wraps in `Arc::new`).
    /// * **Iteration from a match binding**: `items.iter()` works via
    ///   deref; `for x in &*items` and `for x in items.as_ref()` are
    ///   both valid.
    /// * **Mutation with COW**: `Arc::make_mut(&mut inner)` clones if
    ///   shared, otherwise mutates in place.
    /// * **Consuming the inner `Vec<Value>`**: `Arc::try_unwrap(arc)`
    ///   returns `Ok(Vec)` if unique, else `Err(Arc)`; fall back to
    ///   `(*arc).clone()` or `arc.as_ref().clone()` for unconditional
    ///   ownership.
    ///
    /// Equality, ordering, display, hashing semantics are unchanged
    /// because `Arc<T>` transparently forwards read access to `T`.
    List(Arc<Vec<Value>>),
    /// Function closure
    Function(FuncValue),
    /// Gorilla — immediate program termination
    Gorilla,
    /// Unit value (empty buchi pack)
    Unit,
    /// Error value (for throw/catch via error ceiling)
    Error(ErrorValue),
    /// Async value — Mold[T] for asynchronous operations
    Async(AsyncValue),
    /// JSON value — 外部データの型安全なエアロック
    Json(serde_json::Value),
    /// Molten value — opaque primitive for external (JS) interop data.
    /// No methods, no field access. Only usable inside Cage.
    Molten,
    /// Stream value — time-series mold type. Values flow over time.
    /// `]=>` collects all values into `@[T]` (blocking).
    Stream(StreamValue),
    /// C18-2 / C18-3: Tagged enum value — the ordinal carries its owning
    /// enum name so that `jsonEncode` can emit the variant name Str and
    /// `Ordinal[]` can assert the argument came from an Enum variant.
    ///
    /// Interop with `Value::Int(ordinal)`:
    /// - equality: `Int(n) == EnumVal(_, n)` (same ordinal matches).
    /// - ordering: same-enum `EnumVal` pairs compare by ordinal; cross-
    ///   enum or Enum↔Int ordering still falls through to `None`.
    /// - arithmetic: addition / subtraction treat EnumVal like Int(n).
    /// - `.toString()` / `to_display_string()` returns the ordinal as a
    ///   Str, preserving the C16 contract (the variant-name representation
    ///   is only used by jsonEncode, not by display).
    ///
    /// This is strictly additive: every existing code path that produces
    /// `Value::Int(ordinal)` for an Enum continues to work. Code paths
    /// that need the enum identity (Phase 2 jsonEncode, Phase 3 Ordinal[],
    /// Phase 4 ordering) use pattern-match on `EnumVal`.
    EnumVal(String, i64),
}

/// A function closure.
#[derive(Debug, Clone)]
pub struct FuncValue {
    pub name: String,
    pub params: Vec<Param>,
    pub body: Vec<Statement>,
    /// Captured environment (lexical scope closure).
    /// Shared to avoid recursive deep-clone blow-up when many functions capture prior functions.
    pub closure: Arc<HashMap<String, Value>>,
    /// RCB-242: Declared return type from function definition (if any).
    /// Used for introspection. Runtime type enforcement is handled by the checker.
    pub return_type: Option<crate::parser::TypeExpr>,
    /// C20B-015 / ROOT-18: TypeDef registry from the function's defining module.
    ///
    /// When a function is imported into another module and references a schema
    /// via `JSON[raw, Schema]()`, the schema must be resolved against the
    /// defining module's TypeDef scope, not the caller module's. This field
    /// carries the defining module's full TypeDef table (including non-
    /// exported typedefs) so that `resolve_json_schema` can find the schema
    /// even when the caller never imported it.
    ///
    /// `None` for locally-defined functions — resolution falls back to
    /// `Interpreter::type_defs` as before. Also `None` for lambdas / partials /
    /// internal helpers — those can only reference the currently-visible scope.
    pub module_type_defs: Option<Arc<HashMap<String, Vec<FieldDef>>>>,
    /// C20B-015 / ROOT-18: Enum registry from the function's defining module.
    /// Same semantics as `module_type_defs` but for enum declarations.
    pub module_enum_defs: Option<Arc<HashMap<String, Vec<String>>>>,
}

/// An error value.
#[derive(Debug, Clone)]
pub struct ErrorValue {
    pub error_type: String,
    pub message: String,
    pub fields: Vec<(String, Value)>,
}

/// Async status.
#[derive(Debug, Clone, PartialEq)]
pub enum AsyncStatus {
    Pending,
    Fulfilled,
    Rejected,
}

/// Internal state for a pending async task.
/// Shared via Arc<Mutex<...>> so that AsyncValue can be Clone.
#[derive(Debug)]
pub enum PendingState {
    /// Waiting for a result from a tokio task.
    Waiting(tokio::sync::oneshot::Receiver<Result<Value, String>>),
    /// The task completed and its result has been consumed.
    Done,
}

/// An async value — Mold[T] for asynchronous operations.
///
/// Two modes:
/// - **Resolved** (status = Fulfilled | Rejected): value/error set immediately.
///   This is backward-compatible with the original synchronous simulation.
/// - **Pending** (status = Pending, task = Some(...)): a real tokio task is running.
///   When `]=>` is used, the interpreter calls `block_on` to wait for the result.
#[derive(Debug, Clone)]
pub struct AsyncValue {
    pub status: AsyncStatus,
    /// The resolved value (when Fulfilled)
    pub value: Box<Value>,
    /// The error (when Rejected)
    pub error: Box<Value>,
    /// Handle to a pending tokio task. None for immediately resolved values.
    /// Wrapped in Arc<Mutex<>> so AsyncValue can be Clone.
    pub task: Option<Arc<Mutex<PendingState>>>,
}

/// Stream status.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamStatus {
    Active,
    Completed,
}

/// A stream transform operation — stored as a deferred computation.
#[derive(Debug, Clone)]
pub enum StreamTransform {
    /// Map: apply function to each element
    Map(FuncValue),
    /// Filter: keep only elements where function returns true
    Filter(FuncValue),
    /// Take: take at most N elements
    Take(usize),
    /// TakeWhile: take while predicate returns true
    TakeWhile(FuncValue),
}

/// A stream value — Mold[@[T]] for time-series data.
///
/// Stream[T] holds source items and a chain of lazy transforms.
/// When `]=>` is used, the transforms are applied and items collected into a list.
/// PHILOSOPHY.md III: カタめたいなら、鋳型を作りましょう
#[derive(Debug, Clone)]
pub struct StreamValue {
    /// Source items (already materialized for sync mode)
    pub items: Vec<Value>,
    /// Chain of lazy transforms to apply on collect
    pub transforms: Vec<StreamTransform>,
    /// Stream status
    pub status: StreamStatus,
}

impl Value {
    /// Default value for Int.
    pub fn default_int() -> Self {
        Value::Int(0)
    }

    /// Default value for Float.
    pub fn default_float() -> Self {
        Value::Float(0.0)
    }

    /// Default value for Str.
    pub fn default_str() -> Self {
        Value::Str(Arc::new(StrValue::new(String::new())))
    }

    /// Construct a `Value::Str` from an owned `String`, hiding the
    /// `Arc<StrValue>` wrapping. See the doc comment on `Value::Str`
    /// for the rationale (C26B-018 (A) char-index cache, Round 8 wU
    /// extends Round 6 wP interior-Arc migration).
    pub fn str(s: String) -> Self {
        Value::Str(Arc::new(StrValue::new(s)))
    }

    /// COW helper: take ownership of the inner `String` from an
    /// `Arc<StrValue>`. If the `Arc` is uniquely owned, avoids allocation
    /// (the `StrValue` is destructured and its `data` field returned);
    /// otherwise clones the `String`. Used at legacy consumer sites that
    /// previously moved `String` out of `Value::Str`. C26B-018 (A) / wU.
    pub fn str_take(s: Arc<StrValue>) -> String {
        match Arc::try_unwrap(s) {
            Ok(sv) => sv.into_string(),
            Err(arc) => arc.as_string().clone(),
        }
    }

    /// Default value for Bytes.
    pub fn default_bytes() -> Self {
        Value::Bytes(Arc::new(BytesValue::empty()))
    }

    /// Construct a `Value::Bytes` from an owned `Vec<u8>`, hiding the
    /// `Arc` wrapping. See the doc comment on `Value::Bytes` for the
    /// rationale (C26B-020 柱 2 interior migration + D29B-004 Track-ε
    /// view wrapping).
    pub fn bytes(data: Vec<u8>) -> Self {
        let len = data.len();
        Value::Bytes(Arc::new(BytesValue {
            buf: Arc::new(data),
            offset: 0,
            len,
        }))
    }

    /// D29B-004 / Track-ε: Construct a `Value::Bytes` as a zero-copy view
    /// over an existing buffer Arc. Used by `Slice[bytes]` and other
    /// sub-range operations to avoid the per-request body memcpy.
    /// `Arc::ptr_eq(&new.buf, &source.buf) == true` if `source.buf` is
    /// passed as `buf`.
    pub fn bytes_view(buf: Arc<Vec<u8>>, offset: usize, len: usize) -> Self {
        debug_assert!(
            offset.saturating_add(len) <= buf.len(),
            "bytes_view range out of bounds: offset={offset}, len={len}, buf.len={}",
            buf.len()
        );
        Value::Bytes(Arc::new(BytesValue { buf, offset, len }))
    }

    /// COW helper: take ownership of the inner `Vec<u8>` from an
    /// `Arc<BytesValue>`. If both Arcs are uniquely owned and `offset==0`
    /// and `len==buf.len()`, avoids allocation; otherwise allocates a new
    /// `Vec<u8>` containing the slice. C26B-020 柱 2 + D29B-004 Track-ε.
    pub fn bytes_take(data: Arc<BytesValue>) -> Vec<u8> {
        match Arc::try_unwrap(data) {
            Ok(bv) => {
                let BytesValue { buf, offset, len } = bv;
                if offset == 0 && len == buf.len() {
                    Arc::try_unwrap(buf).unwrap_or_else(|arc| (*arc).clone())
                } else {
                    buf[offset..offset + len].to_vec()
                }
            }
            Err(arc) => arc.as_slice().to_vec(),
        }
    }

    /// Default value for Bool.
    pub fn default_bool() -> Self {
        Value::Bool(false)
    }

    /// Default value for List.
    pub fn default_list() -> Self {
        Value::List(Arc::new(Vec::new()))
    }

    /// Construct a `Value::List` from an owned `Vec<Value>`, hiding the
    /// `Arc` wrapping. See the doc comment on `Value::List` for the
    /// rationale (Phase 5-F2-1 interior migration).
    pub fn list(items: Vec<Value>) -> Self {
        Value::List(Arc::new(items))
    }

    /// COW helper: take ownership of the inner `Vec<Value>` from an
    /// `Arc<Vec<Value>>`. If the `Arc` is uniquely owned, avoids allocation;
    /// otherwise clones the vec. Used at all legacy consumer sites that
    /// previously moved `Vec<Value>` out of `Value::List`. Phase 5-F2-1.
    pub fn list_take(items: Arc<Vec<Value>>) -> Vec<Value> {
        Arc::try_unwrap(items).unwrap_or_else(|arc| (*arc).clone())
    }

    /// Default value for BuchiPack (empty).
    pub fn default_buchi() -> Self {
        Value::BuchiPack(Arc::new(Vec::new()))
    }

    /// Construct a `Value::BuchiPack` from an owned `Vec<(String, Value)>`,
    /// hiding the `Arc` wrapping. See the doc comment on `Value::BuchiPack`
    /// for the rationale (C26B-012 interior migration, Round 6 wQ).
    pub fn pack(fields: Vec<(String, Value)>) -> Self {
        Value::BuchiPack(Arc::new(fields))
    }

    /// COW helper: take ownership of the inner `Vec<(String, Value)>` from an
    /// `Arc<Vec<(String, Value)>>`. If the `Arc` is uniquely owned, avoids
    /// allocation; otherwise clones the vec. Used at legacy consumer sites
    /// that previously moved `Vec<(String, Value)>` out of `Value::BuchiPack`.
    /// C26B-012 / Round 6 wQ.
    pub fn pack_take(fields: Arc<Vec<(String, Value)>>) -> Vec<(String, Value)> {
        Arc::try_unwrap(fields).unwrap_or_else(|arc| (*arc).clone())
    }

    /// Default value for JSON (empty object `{}`).
    pub fn default_json() -> Self {
        Value::Json(serde_json::Value::Object(serde_json::Map::new()))
    }

    /// Default value for Molten (empty molten iron).
    pub fn default_molten() -> Self {
        Value::Molten
    }

    /// Default value for Stream (empty completed stream).
    pub fn default_stream() -> Self {
        Value::Stream(StreamValue {
            items: Vec::new(),
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        })
    }

    /// Infer the default value from a list's element type.
    /// Looks at the first element to determine what type the list holds,
    /// then returns the appropriate default for that type.
    /// PHILOSOPHY.md I: all types must have default values.
    pub fn default_for_list(items: &[Value]) -> Value {
        match items.first() {
            Some(Value::Int(_)) => Value::Int(0),
            Some(Value::Float(_)) => Value::Float(0.0),
            Some(Value::Str(_)) => Value::default_str(),
            Some(Value::Bytes(_)) => Value::bytes(Vec::new()),
            Some(Value::Bool(_)) => Value::Bool(false),
            Some(Value::BuchiPack(_)) => Value::default_buchi(),
            Some(Value::List(_)) => Value::list(Vec::new()),
            Some(Value::Json(_)) => Value::default_json(),
            Some(Value::Molten) => Value::Molten,
            Some(Value::Stream(_)) => Value::default_stream(),
            Some(Value::Unit) => Value::Unit,
            _ => Value::Int(0), // empty list fallback
        }
    }

    /// Check if this value is truthy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::Bytes(b) => !b.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::BuchiPack(_) => true,
            Value::Function(_) => true,
            Value::Unit => false,
            Value::Gorilla => false,
            Value::Error(_) => true,
            Value::Molten => false,
            Value::Stream(s) => !s.items.is_empty() || s.status == StreamStatus::Active,
            Value::Async(a) => a.status == AsyncStatus::Fulfilled,
            Value::Json(j) => match j {
                serde_json::Value::Null => false,
                serde_json::Value::Bool(b) => *b,
                serde_json::Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
                serde_json::Value::String(s) => !s.is_empty(),
                serde_json::Value::Array(a) => !a.is_empty(),
                serde_json::Value::Object(o) => !o.is_empty(),
            },
            // C18-2: EnumVal shares Int(ordinal) truthiness: the first
            // variant (ordinal 0) is falsy, every other variant is truthy.
            Value::EnumVal(_, n) => *n != 0,
        }
    }

    /// Get a field from a buchi pack or error value.
    pub fn get_field(&self, name: &str) -> Option<&Value> {
        match self {
            Value::BuchiPack(fields) => fields.iter().find(|(n, _)| n == name).map(|(_, v)| v),
            Value::Error(err) => {
                // Check built-in fields first
                if name == "type" {
                    return None; // Handled specially
                }
                if name == "message" {
                    return None; // Handled specially
                }
                err.fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
            }
            _ => None,
        }
    }

    /// Get a field from an error, including built-in fields.
    pub fn get_error_field(&self, name: &str) -> Option<Value> {
        if let Value::Error(err) = self {
            match name {
                "type" => Some(Value::str(err.error_type.clone())),
                "message" => Some(Value::str(err.message.clone())),
                _ => err
                    .fields
                    .iter()
                    .find(|(n, _)| n == name)
                    .map(|(_, v)| v.clone()),
            }
        } else {
            None
        }
    }

    /// Convert to string representation.
    pub fn to_display_string(&self) -> String {
        match self {
            Value::Int(n) => n.to_string(),
            Value::Float(n) => {
                if *n == n.floor() && n.is_finite() {
                    format!("{:.1}", n)
                } else {
                    n.to_string()
                }
            }
            Value::Str(s) => s.as_string().clone(),
            Value::Bytes(bytes) => {
                let elems: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
                format!("Bytes[@[{}]]", elems.join(", "))
            }
            Value::Bool(b) => b.to_string(),
            Value::BuchiPack(fields) => {
                let mut s = String::from("@(");
                for (i, (name, val)) in fields.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&format!("{} <= {}", name, val.to_debug_string()));
                }
                s.push(')');
                s
            }
            Value::List(items) => {
                let mut s = String::from("@[");
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&item.to_debug_string());
                }
                s.push(']');
                s
            }
            Value::Function(f) => format!("<function {}>", f.name),
            Value::Gorilla => "><".to_string(),
            Value::Unit => "@()".to_string(),
            Value::Error(err) => format!("Error({}: {})", err.error_type, err.message),
            Value::Async(a) => match a.status {
                AsyncStatus::Fulfilled => {
                    format!("Async[fulfilled: {}]", a.value.to_display_string())
                }
                AsyncStatus::Rejected => {
                    format!("Async[rejected: {}]", a.error.to_display_string())
                }
                AsyncStatus::Pending => {
                    if a.task.is_some() {
                        "Async[pending (task)]".to_string()
                    } else {
                        "Async[pending]".to_string()
                    }
                }
            },
            Value::Json(j) => serde_json::to_string(j).unwrap_or_default(),
            Value::Molten => "Molten".to_string(),
            Value::Stream(s) => match s.status {
                StreamStatus::Active => "Stream[active]".to_string(),
                StreamStatus::Completed => format!("Stream[completed: {} items]", s.items.len()),
            },
            // C18-2: `.toString()` and display preserve the ordinal Str
            // contract (`docs/guide/01_types.md:609` and ROOT-4 in
            // `.dev/C18_BLOCKERS.md`). jsonEncode uses a dedicated path
            // to emit the variant name.
            Value::EnumVal(_, n) => n.to_string(),
        }
    }

    /// Convert to debug string (with quotes around strings).
    pub fn to_debug_string(&self) -> String {
        match self {
            Value::Str(s) => format!("\"{}\"", s),
            other => other.to_display_string(),
        }
    }

    /// SEC-007: Truncated display for error messages.
    /// Limits output to approximately `max_len` characters to prevent
    /// large data structures from producing multi-megabyte error messages.
    pub fn to_error_display(&self, max_len: usize) -> String {
        let full = self.to_display_string();
        if full.len() <= max_len {
            return full;
        }
        let suffix = match self {
            Value::List(items) => format!("... ({} items)]", items.len()),
            Value::BuchiPack(fields) => format!("... ({} fields))", fields.len()),
            _ => "...".to_string(),
        };
        let truncate_at = max_len.saturating_sub(suffix.len());
        // Find a safe truncation point (don't split multi-byte chars)
        let mut safe = truncate_at;
        while safe > 0 && !full.is_char_boundary(safe) {
            safe -= 1;
        }
        format!("{}{}", &full[..safe], suffix)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_display_string())
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
            (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Bytes(a), Value::Bytes(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::BuchiPack(a), Value::BuchiPack(b)) => {
                if a.len() != b.len() {
                    return false;
                }
                // Order-independent comparison: each field in a must exist in b with equal value
                a.iter().all(|(name_a, val_a)| {
                    b.iter()
                        .any(|(name_b, val_b)| name_a == name_b && val_a == val_b)
                })
            }
            (Value::Unit, Value::Unit) => true,
            (Value::Gorilla, Value::Gorilla) => true,
            (Value::Async(a), Value::Async(b)) => a.status == b.status && *a.value == *b.value,
            (Value::Json(a), Value::Json(b)) => a == b,
            (Value::Molten, Value::Molten) => true,
            (Value::Stream(a), Value::Stream(b)) => a.status == b.status && a.items == b.items,
            // C18-2: EnumVal equality — same enum + same ordinal. Cross-enum
            // `EnumVal` comparisons return false (the type checker blocks them
            // with [E1605]; this is a defence in depth). `EnumVal` is also
            // equal to `Int(ordinal)` and `Float(ordinal)` with the same
            // numeric value so that existing callers who look at ordinals
            // directly continue to work (e.g. the JSON schema code returns
            // `Value::Int(ordinal)` from `JSON[..., Enum]()`).
            (Value::EnumVal(a_name, a_n), Value::EnumVal(b_name, b_n)) => {
                a_name == b_name && a_n == b_n
            }
            (Value::EnumVal(_, n), Value::Int(m)) | (Value::Int(m), Value::EnumVal(_, n)) => n == m,
            (Value::EnumVal(_, n), Value::Float(f)) | (Value::Float(f), Value::EnumVal(_, n)) => {
                (*n as f64) == *f
            }
            _ => false,
        }
    }
}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (Value::Int(a), Value::Int(b)) => a.partial_cmp(b),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::Str(a), Value::Str(b)) => a.partial_cmp(b),
            (Value::Bool(a), Value::Bool(b)) => a.partial_cmp(b),
            // C18-4: Same-enum `EnumVal` pairs order by ordinal (declared
            // order is the contract). Cross-enum and Enum↔Int ordering stays
            // `None` so the type checker's [E1605] path still fires when it
            // has to (the checker never reaches here for cross-enum cases,
            // but this is defence in depth for runtime comparisons inside
            // `--no-check` builds).
            (Value::EnumVal(a_name, a_n), Value::EnumVal(b_name, b_n)) if a_name == b_name => {
                a_n.partial_cmp(b_n)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        // PHILOSOPHY.md: all types must have default values
        assert_eq!(Value::default_int(), Value::Int(0));
        assert_eq!(Value::default_float(), Value::Float(0.0));
        assert_eq!(Value::default_str(), Value::str(String::new()));
        assert_eq!(Value::default_bytes(), Value::bytes(Vec::new()));
        assert_eq!(Value::default_bool(), Value::Bool(false));
        assert_eq!(Value::default_list(), Value::list(Vec::new()));
    }

    // ── C26B-018 (A) / Round 8 wU: char-index cache tests ──

    #[test]
    fn wu_char_cache_ascii_char_count() {
        let s = StrValue::new("hello".to_string());
        assert_eq!(s.cached_char_count(), 5);
        // Second call must still return the same count (cache hit path).
        assert_eq!(s.cached_char_count(), 5);
    }

    #[test]
    fn wu_char_cache_utf8_char_count() {
        // Mix of 1-byte, 3-byte, and 4-byte UTF-8 sequences.
        // "aあ🙂b" = a (1B) + あ (3B) + 🙂 (4B) + b (1B) = 9 bytes, 4 chars.
        let s = StrValue::new("aあ🙂b".to_string());
        assert_eq!(s.as_string().len(), 9);
        assert_eq!(s.cached_char_count(), 4);
    }

    #[test]
    fn wu_char_cache_empty_string() {
        let s = StrValue::new(String::new());
        assert_eq!(s.cached_char_count(), 0);
        assert_eq!(s.cached_char_at(0), None);
        assert_eq!(s.cached_char_slice(0, 0), String::new());
    }

    #[test]
    fn wu_char_cache_char_at_ascii() {
        let s = StrValue::new("hello".to_string());
        assert_eq!(s.cached_char_at(0).as_deref(), Some("h"));
        assert_eq!(s.cached_char_at(4).as_deref(), Some("o"));
        assert_eq!(s.cached_char_at(5), None);
        assert_eq!(s.cached_char_at(100), None);
    }

    #[test]
    fn wu_char_cache_char_at_usize_max_is_none() {
        // Regression: `n as usize` for `n = -1i64` yields `usize::MAX`.
        // `cached_char_at(usize::MAX)` must return `None` without
        // triggering `idx + 1` overflow. Covers
        // `test_bt13_char_at_negative_index` regression path.
        let s = StrValue::new("hello".to_string());
        assert_eq!(s.cached_char_at(usize::MAX), None);
    }

    #[test]
    fn wu_char_cache_char_at_utf8() {
        let s = StrValue::new("aあ🙂b".to_string());
        assert_eq!(s.cached_char_at(0).as_deref(), Some("a"));
        assert_eq!(s.cached_char_at(1).as_deref(), Some("あ"));
        assert_eq!(s.cached_char_at(2).as_deref(), Some("🙂"));
        assert_eq!(s.cached_char_at(3).as_deref(), Some("b"));
        assert_eq!(s.cached_char_at(4), None);
    }

    #[test]
    fn wu_char_cache_slice_ascii() {
        let s = StrValue::new("abcdef".to_string());
        assert_eq!(s.cached_char_slice(0, 3), "abc");
        assert_eq!(s.cached_char_slice(2, 5), "cde");
        assert_eq!(s.cached_char_slice(0, 6), "abcdef");
        assert_eq!(s.cached_char_slice(3, 3), "");
        // Out-of-range end clamps to char_count.
        assert_eq!(s.cached_char_slice(2, 100), "cdef");
    }

    #[test]
    fn wu_char_cache_slice_utf8() {
        let s = StrValue::new("aあ🙂b".to_string());
        assert_eq!(s.cached_char_slice(0, 1), "a");
        assert_eq!(s.cached_char_slice(1, 3), "あ🙂");
        assert_eq!(s.cached_char_slice(2, 4), "🙂b");
        assert_eq!(s.cached_char_slice(0, 4), "aあ🙂b");
    }

    #[test]
    fn wu_char_cache_byte_to_char_index() {
        // "aあ🙂b": byte offsets 0, 1, 4, 8; sentinel 9.
        let s = StrValue::new("aあ🙂b".to_string());
        assert_eq!(s.cached_byte_to_char_index(0), Some(0));
        assert_eq!(s.cached_byte_to_char_index(1), Some(1));
        assert_eq!(s.cached_byte_to_char_index(4), Some(2));
        assert_eq!(s.cached_byte_to_char_index(8), Some(3));
        // Sentinel byte offset maps to char_count (past-the-end).
        assert_eq!(s.cached_byte_to_char_index(9), Some(4));
        // Mid-char bytes are not on a boundary.
        assert_eq!(s.cached_byte_to_char_index(2), None);
        assert_eq!(s.cached_byte_to_char_index(3), None);
    }

    #[test]
    fn wu_char_cache_shared_across_arc_clones() {
        // The cache should be shared across all `Arc<StrValue>` clones —
        // populating via one handle makes the same table visible through
        // any other handle.
        let a = Arc::new(StrValue::new("hello world".to_string()));
        let b = Arc::clone(&a);
        assert_eq!(a.cached_char_count(), 11); // populates the cache
        assert_eq!(b.cached_char_count(), 11); // must be cache-hit path
        // Identity check: both Arc clones point to the same StrValue,
        // so they share the same OnceLock storage by definition.
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn wu_char_cache_deref_transparency() {
        // StrValue must Deref to String so existing byte-level
        // operations (contains, starts_with, as_ptr, len, chars, etc.)
        // keep working unchanged through the Arc wrapper.
        let s = StrValue::new("hello 🙂".to_string());
        assert!(s.starts_with("hello"));
        assert!(s.ends_with("🙂"));
        assert!(s.contains("lo 🙂"));
        assert_eq!(s.len(), 10); // byte length, not char count
        assert_eq!(s.chars().count(), 7);
    }

    #[test]
    fn wu_value_str_clone_is_cheap() {
        // Round 6 wP guarantee: Value::clone on Str is an Arc::clone.
        // Round 8 wU preserves it — clones share the cache.
        let v = Value::str("the quick brown fox".to_string());
        let Value::Str(arc1) = &v else { panic!() };
        let v2 = v.clone();
        let Value::Str(arc2) = &v2 else { panic!() };
        assert!(Arc::ptr_eq(arc1, arc2));
    }

    #[test]
    fn wu_str_take_unique_fast_path() {
        // Unique Arc → try_unwrap succeeds → original String is returned.
        let arc = Arc::new(StrValue::new("owned".to_string()));
        let s = Value::str_take(arc);
        assert_eq!(s, "owned");
    }

    #[test]
    fn wu_str_take_shared_clone_path() {
        // Shared Arc → try_unwrap fails → fall back to clone.
        let arc = Arc::new(StrValue::new("shared".to_string()));
        let _retained = Arc::clone(&arc);
        let s = Value::str_take(arc);
        assert_eq!(s, "shared");
    }

    // ── BT-18: Default value guarantee exhaustive tests ──

    #[test]
    fn test_bt18_default_buchi_pack() {
        // BuchiPack default is empty @()
        let d = Value::default_buchi();
        assert_eq!(d, Value::default_buchi());
        // Note: empty BuchiPack truthiness depends on implementation
        // (may be truthy since it's a valid struct, just with no fields)
    }

    #[test]
    fn test_bt18_default_json() {
        // JSON default is empty object {}
        let d = Value::default_json();
        match &d {
            Value::Json(v) => assert!(v.is_object(), "JSON default should be object"),
            other => panic!("Expected Json, got: {:?}", other),
        }
    }

    #[test]
    fn test_bt18_default_molten() {
        // Molten default is Molten itself
        assert_eq!(Value::default_molten(), Value::Molten);
    }

    #[test]
    fn test_bt18_default_stream() {
        // Stream default is completed empty stream
        match &Value::default_stream() {
            Value::Stream(sv) => {
                assert!(sv.items.is_empty(), "Default stream should have no items");
                assert_eq!(
                    sv.status,
                    StreamStatus::Completed,
                    "Default stream should be completed"
                );
            }
            other => panic!("Expected Stream, got: {:?}", other),
        }
    }

    #[test]
    fn test_bt18_default_for_list_inference() {
        // default_for_list should infer from first element type
        let int_items = vec![Value::Int(1), Value::Int(2)];
        assert_eq!(Value::default_for_list(&int_items), Value::Int(0));

        let str_items = vec![Value::str("a".to_string())];
        assert_eq!(
            Value::default_for_list(&str_items),
            Value::str(String::new())
        );

        let float_items = vec![Value::Float(1.5)];
        assert_eq!(Value::default_for_list(&float_items), Value::Float(0.0));

        // Empty list should return Int(0) as default
        let empty: Vec<Value> = vec![];
        assert_eq!(Value::default_for_list(&empty), Value::Int(0));
    }

    #[test]
    fn test_truthiness() {
        assert!(Value::Bool(true).is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Int(1).is_truthy());
        assert!(!Value::Int(0).is_truthy());
        assert!(Value::str("hello".to_string()).is_truthy());
        assert!(!Value::str(String::new()).is_truthy());
        assert!(!Value::Unit.is_truthy());
    }

    #[test]
    fn test_equality() {
        assert_eq!(Value::Int(42), Value::Int(42));
        assert_ne!(Value::Int(42), Value::Int(43));
        // Int-Float cross comparison
        assert_eq!(Value::Int(42), Value::Float(42.0));
        assert_eq!(
            Value::str("hello".to_string()),
            Value::str("hello".to_string())
        );
        assert_eq!(Value::bytes(vec![1, 2]), Value::bytes(vec![1, 2]));
    }

    #[test]
    fn test_ordering() {
        assert!(Value::Int(1) < Value::Int(2));
        assert!(Value::Float(1.0) < Value::Float(2.0));
        assert!(Value::Int(1) < Value::Float(2.0));
        assert!(Value::str("a".to_string()) < Value::str("b".to_string()));
    }

    #[test]
    fn test_display() {
        assert_eq!(Value::Int(42).to_string(), "42");
        assert_eq!(Value::Float(314.0 / 100.0).to_string(), "3.14");
        assert_eq!(Value::str("hello".to_string()).to_string(), "hello");
        assert_eq!(Value::bytes(vec![1, 2]).to_string(), "Bytes[@[1, 2]]");
        assert_eq!(Value::Bool(true).to_string(), "true");
        assert_eq!(Value::Unit.to_string(), "@()");
    }

    #[test]
    fn test_buchi_pack_field_access() {
        let pack = Value::pack(vec![
            ("name".to_string(), Value::str("Alice".to_string())),
            ("age".to_string(), Value::Int(30)),
        ]);
        assert_eq!(
            pack.get_field("name"),
            Some(&Value::str("Alice".to_string()))
        );
        assert_eq!(pack.get_field("age"), Some(&Value::Int(30)));
        assert_eq!(pack.get_field("email"), None);
    }

    #[test]
    fn test_json_default() {
        let json = Value::default_json();
        if let Value::Json(j) = &json {
            assert!(j.is_object());
            assert!(j.as_object().unwrap().is_empty());
        } else {
            panic!("Expected Value::Json");
        }
    }

    #[test]
    fn test_json_truthiness() {
        // null → false
        assert!(!Value::Json(serde_json::Value::Null).is_truthy());
        // empty object → false
        assert!(!Value::Json(serde_json::json!({})).is_truthy());
        // empty array → false
        assert!(!Value::Json(serde_json::json!([])).is_truthy());
        // empty string → false
        assert!(!Value::Json(serde_json::json!("")).is_truthy());
        // 0 → false
        assert!(!Value::Json(serde_json::json!(0)).is_truthy());
        // non-empty object → true
        assert!(Value::Json(serde_json::json!({"a":1})).is_truthy());
        // non-empty array → true
        assert!(Value::Json(serde_json::json!([1])).is_truthy());
        // true → true
        assert!(Value::Json(serde_json::json!(true)).is_truthy());
    }

    #[test]
    fn test_json_equality() {
        let a = Value::Json(serde_json::json!({"x": 1}));
        let b = Value::Json(serde_json::json!({"x": 1}));
        let c = Value::Json(serde_json::json!({"x": 2}));
        assert_eq!(a, b);
        assert_ne!(a, c);
        // JSON != BuchiPack (different types)
        assert_ne!(a, Value::pack(vec![("x".to_string(), Value::Int(1))]));
    }

    #[test]
    fn test_json_display() {
        let json = Value::Json(serde_json::json!({"a": 1}));
        let s = json.to_display_string();
        assert!(s.contains("\"a\""));
        assert!(s.contains("1"));
    }

    #[test]
    fn test_molten_default() {
        // Molten default is Molten itself (empty molten iron)
        assert_eq!(Value::default_molten(), Value::Molten);
    }

    #[test]
    fn test_molten_truthiness() {
        // Molten (empty molten iron) is falsy
        assert!(!Value::Molten.is_truthy());
    }

    #[test]
    fn test_molten_equality() {
        assert_eq!(Value::Molten, Value::Molten);
        // Molten is not equal to other types
        assert_ne!(Value::Molten, Value::Unit);
        assert_ne!(Value::Molten, Value::Int(0));
    }

    #[test]
    fn test_molten_display() {
        assert_eq!(Value::Molten.to_display_string(), "Molten");
        assert_eq!(Value::Molten.to_string(), "Molten");
    }

    #[test]
    fn test_molten_debug_string() {
        assert_eq!(Value::Molten.to_debug_string(), "Molten");
    }

    #[test]
    fn test_stream_default() {
        let stream = Value::default_stream();
        if let Value::Stream(s) = &stream {
            assert!(s.items.is_empty());
            assert_eq!(s.status, StreamStatus::Completed);
            assert!(s.transforms.is_empty());
        } else {
            panic!("Expected Value::Stream");
        }
    }

    #[test]
    fn test_stream_truthiness() {
        // Empty completed stream → false
        assert!(!Value::default_stream().is_truthy());
        // Active stream → true
        assert!(
            Value::Stream(StreamValue {
                items: Vec::new(),
                transforms: Vec::new(),
                status: StreamStatus::Active,
            })
            .is_truthy()
        );
        // Completed with items → true
        assert!(
            Value::Stream(StreamValue {
                items: vec![Value::Int(1)],
                transforms: Vec::new(),
                status: StreamStatus::Completed,
            })
            .is_truthy()
        );
    }

    #[test]
    fn test_stream_display() {
        let active = Value::Stream(StreamValue {
            items: Vec::new(),
            transforms: Vec::new(),
            status: StreamStatus::Active,
        });
        assert_eq!(active.to_display_string(), "Stream[active]");

        let completed = Value::Stream(StreamValue {
            items: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        });
        assert_eq!(completed.to_display_string(), "Stream[completed: 3 items]");
    }

    #[test]
    fn test_stream_equality() {
        let a = Value::Stream(StreamValue {
            items: vec![Value::Int(1)],
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        });
        let b = Value::Stream(StreamValue {
            items: vec![Value::Int(1)],
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        });
        let c = Value::Stream(StreamValue {
            items: vec![Value::Int(2)],
            transforms: Vec::new(),
            status: StreamStatus::Completed,
        });
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
