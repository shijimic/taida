//! Safe wrappers around the frozen ABI v1 value bridge.
//!
//! This module is the **addon-author ergonomic layer** on top of the
//! raw `TaidaAddonValueV1` / `TaidaHostV1` C ABI types in [`crate::abi`].
//! It gives addon authors a Rusty way to:
//!
//! 1. Read borrowed input values out of the host-supplied argument
//!    vector (`BorrowedValue`).
//! 2. Build host-owned return values via the host capability table
//!    without ever touching raw pointers directly (`HostValueBuilder`).
//!
//! Nothing in this module allocates bridge values directly — every
//! construction goes through the host callback table, which keeps the
//! "allocator unification" guarantee from `.dev/RC1_DESIGN.md` Phase 3
//! Lock (single allocator = single free-er = no double-free).
//!
//! # Ownership model (quick reference)
//!
//! - The host builds an `&[TaidaAddonValueV1]` input vector and hands it
//!   to the addon as `args_ptr` + `args_len`. Each element is a
//!   **borrowed view** — the addon may read it during the call but
//!   must not retain a reference past return.
//! - The addon uses [`HostValueBuilder`] to build an output value via
//!   host callbacks; ownership immediately transfers to the host. The
//!   addon writes the returned pointer into `*out_value` and returns
//!   `TaidaAddonStatus::Ok`. The host is now responsible for releasing
//!   it.
//! - On error, the addon uses [`HostValueBuilder::error`] to build an
//!   error via `error_new`, writes it into `*out_error`, and returns
//!   `TaidaAddonStatus::Error`.

use core::ffi::{CStr, c_char};
use core::slice;

use crate::abi::{TaidaAddonErrorV1, TaidaAddonValueTag, TaidaAddonValueV1, TaidaHostV1};

/// Read-only view of a single host-provided argument.
///
/// This type is produced by [`borrowed_args`] (which takes the raw
/// `args_ptr` / `args_len` pair the host passes to the addon) and lets
/// the addon inspect the tag and payload of a borrowed value without
/// touching raw pointers directly.
///
/// Lifetime `'a` is tied to the addon's call frame so the borrow cannot
/// outlive the call.
#[derive(Clone, Copy)]
pub struct BorrowedValue<'a> {
    raw: &'a TaidaAddonValueV1,
}

impl<'a> BorrowedValue<'a> {
    /// Safely narrow the raw `u32` tag to a [`TaidaAddonValueTag`].
    /// Returns `None` if the host sent an out-of-range tag (the addon
    /// should respond with `TaidaAddonStatus::UnsupportedValue`).
    pub fn tag(&self) -> Option<TaidaAddonValueTag> {
        TaidaAddonValueTag::from_u32(self.raw.tag)
    }

    /// Raw `u32` tag, useful for diagnostics when [`Self::tag`] returns `None`.
    pub fn raw_tag(&self) -> u32 {
        self.raw.tag
    }

    /// Access the underlying `TaidaAddonValueV1` by reference. Only use
    /// this if you need to forward the borrow into a host callback that
    /// takes a raw pointer — higher-level accessors below cover the
    /// common cases.
    pub fn as_raw(&self) -> *const TaidaAddonValueV1 {
        self.raw as *const _
    }

    /// If this value is an `Int`, return its i64 payload.
    pub fn as_int(&self) -> Option<i64> {
        if self.raw.tag != TaidaAddonValueTag::Int as u32 {
            return None;
        }
        if self.raw.payload.is_null() {
            return None;
        }
        // SAFETY: tag matches Int, payload is a host-owned
        // TaidaAddonIntPayload for the duration of the call.
        let p = self.raw.payload as *const crate::abi::TaidaAddonIntPayload;
        Some(unsafe { (*p).value })
    }

    /// If this value is a `Float`, return its f64 payload.
    pub fn as_float(&self) -> Option<f64> {
        if self.raw.tag != TaidaAddonValueTag::Float as u32 {
            return None;
        }
        if self.raw.payload.is_null() {
            return None;
        }
        // SAFETY: see `as_int`.
        let p = self.raw.payload as *const crate::abi::TaidaAddonFloatPayload;
        Some(unsafe { (*p).value })
    }

    /// If this value is a `Bool`, return its boolean payload.
    ///
    /// Per the frozen ABI v1 contract the payload byte is strictly
    /// `0` (false) or `1` (true). Any other value is a malformed
    /// input and yields `None` so the addon can treat it as a failure
    /// (RC1B-109). The previous `!= 0` coercion silently mapped
    /// `2..=255` to `true`, hiding bugs on the producer side.
    pub fn as_bool(&self) -> Option<bool> {
        if self.raw.tag != TaidaAddonValueTag::Bool as u32 {
            return None;
        }
        if self.raw.payload.is_null() {
            return None;
        }
        // SAFETY: see `as_int`.
        let p = self.raw.payload as *const crate::abi::TaidaAddonBoolPayload;
        match unsafe { (*p).value } {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    /// If this value is a `Str`, return its UTF-8 bytes as `&str`. The
    /// borrow is valid for the call.
    pub fn as_str(&self) -> Option<&'a str> {
        if self.raw.tag != TaidaAddonValueTag::Str as u32 {
            return None;
        }
        let bytes = self.as_raw_bytes_slot()?;
        // SAFETY: ABI contract — Str payloads must be valid UTF-8.
        core::str::from_utf8(bytes).ok()
    }

    /// If this value is a `Bytes`, return its octets as `&[u8]`. The
    /// borrow is valid for the call.
    pub fn as_bytes(&self) -> Option<&'a [u8]> {
        if self.raw.tag != TaidaAddonValueTag::Bytes as u32 {
            return None;
        }
        self.as_raw_bytes_slot()
    }

    fn as_raw_bytes_slot(&self) -> Option<&'a [u8]> {
        if self.raw.payload.is_null() {
            return None;
        }
        // SAFETY: Str and Bytes share the same payload struct.
        let p = self.raw.payload as *const crate::abi::TaidaAddonBytesPayload;
        let p_ref = unsafe { &*p };
        if p_ref.len == 0 {
            return Some(&[]);
        }
        if p_ref.ptr.is_null() {
            return None;
        }
        // SAFETY: the host keeps the bytes alive for the duration of
        // the call, and the slice length is bounded by the host.
        Some(unsafe { slice::from_raw_parts(p_ref.ptr, p_ref.len) })
    }

    /// If this value is a `List`, return an iterator over borrowed
    /// child views.
    pub fn as_list(&self) -> Option<BorrowedList<'a>> {
        if self.raw.tag != TaidaAddonValueTag::List as u32 {
            return None;
        }
        if self.raw.payload.is_null() {
            return Some(BorrowedList { items: &[] });
        }
        // SAFETY: List payload is host-owned for the call.
        let p = self.raw.payload as *const crate::abi::TaidaAddonListPayload;
        let p_ref = unsafe { &*p };
        if p_ref.len == 0 {
            return Some(BorrowedList { items: &[] });
        }
        if p_ref.items.is_null() {
            return None;
        }
        // SAFETY: host guarantees `items` points to a valid array of
        // `len` `*mut TaidaAddonValueV1` for the call.
        let slice = unsafe { slice::from_raw_parts(p_ref.items, p_ref.len) };
        Some(BorrowedList { items: slice })
    }

    /// If this value is a `Pack`, return an iterator over `(name,
    /// BorrowedValue)` pairs.
    pub fn as_pack(&self) -> Option<BorrowedPack<'a>> {
        if self.raw.tag != TaidaAddonValueTag::Pack as u32 {
            return None;
        }
        if self.raw.payload.is_null() {
            return Some(BorrowedPack { entries: &[] });
        }
        // SAFETY: pack payload is host-owned for the call.
        let p = self.raw.payload as *const crate::abi::TaidaAddonPackPayload;
        let p_ref = unsafe { &*p };
        if p_ref.len == 0 {
            return Some(BorrowedPack { entries: &[] });
        }
        if p_ref.entries.is_null() {
            return None;
        }
        // SAFETY: host guarantees the entries array.
        let slice = unsafe { slice::from_raw_parts(p_ref.entries, p_ref.len) };
        Some(BorrowedPack { entries: slice })
    }
}

/// Borrowed view of a list payload.
#[derive(Clone, Copy)]
pub struct BorrowedList<'a> {
    items: &'a [*mut TaidaAddonValueV1],
}

impl<'a> BorrowedList<'a> {
    /// Number of elements in the list.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Borrow element at `index`.
    pub fn get(&self, index: usize) -> Option<BorrowedValue<'a>> {
        let raw_ptr = *self.items.get(index)?;
        if raw_ptr.is_null() {
            return None;
        }
        // SAFETY: host guarantees non-null children are valid for the call.
        Some(BorrowedValue {
            raw: unsafe { &*raw_ptr },
        })
    }

    /// Iterate over borrowed children.
    pub fn iter(&self) -> impl Iterator<Item = BorrowedValue<'a>> + '_ {
        (0..self.items.len()).filter_map(move |i| self.get(i))
    }
}

/// Borrowed view of a pack payload.
#[derive(Clone, Copy)]
pub struct BorrowedPack<'a> {
    entries: &'a [crate::abi::TaidaAddonPackEntryV1],
}

impl<'a> BorrowedPack<'a> {
    /// Number of named fields in the pack.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pack has no fields.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over `(name, BorrowedValue)` pairs. `name` is a borrowed
    /// UTF-8 `&str` for the duration of the call.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, BorrowedValue<'a>)> + '_ {
        self.entries.iter().filter_map(|entry| {
            if entry.name.is_null() || entry.value.is_null() {
                return None;
            }
            // SAFETY: host guarantees nul-terminated UTF-8 for Pack
            // entry names.
            let cstr = unsafe { CStr::from_ptr(entry.name) };
            let name = cstr.to_str().ok()?;
            let value = BorrowedValue {
                // SAFETY: non-null checked above; host keeps it alive.
                raw: unsafe { &*entry.value },
            };
            Some((name, value))
        })
    }
}

/// Build a slice view of the raw host-supplied arg vector.
///
/// # Safety
///
/// - `args_ptr` must either be null (and `args_len == 0`) or point to a
///   valid array of `args_len` `TaidaAddonValueV1` entries for the
///   duration of the addon call.
/// - The host owns those entries; the addon must not write through the
///   pointers and must not retain them past return.
pub unsafe fn borrowed_args<'a>(
    args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
) -> &'a [TaidaAddonValueV1] {
    if args_len == 0 {
        return &[];
    }
    if args_ptr.is_null() {
        return &[];
    }
    // SAFETY: delegated to the caller's preconditions (see doc-comment).
    unsafe { slice::from_raw_parts(args_ptr, args_len as usize) }
}

/// Borrow the argument at `index` from the host-supplied vector.
///
/// # Safety
///
/// Same requirements as [`borrowed_args`]. `index` must be less than
/// `args_len`.
pub unsafe fn borrow_arg<'a>(
    args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    index: usize,
) -> Option<BorrowedValue<'a>> {
    // SAFETY: delegated to the caller.
    let slice = unsafe { borrowed_args(args_ptr, args_len) };
    slice.get(index).map(|raw| BorrowedValue { raw })
}

/// Ergonomic wrapper around [`TaidaHostV1`] for building host-owned
/// return values.
///
/// Every method here forwards to the corresponding callback in the host
/// capability table. The returned raw pointers are already host-owned —
/// write them into `*out_value` / `*out_error` and return the
/// appropriate [`crate::abi::TaidaAddonStatus`]. Do **not** release
/// them yourself; the host releases after the call returns.
#[derive(Clone, Copy)]
pub struct HostValueBuilder<'a> {
    host: &'a TaidaHostV1,
}

impl<'a> HostValueBuilder<'a> {
    /// Wrap a raw host pointer.
    ///
    /// # Safety
    ///
    /// `host` must point to a valid `TaidaHostV1` whose callback table
    /// is live for the duration of the current addon call. The host
    /// passes this pointer to the addon through the addon's `init`
    /// callback (and, in future phases, through every call).
    pub unsafe fn from_raw(host: *const TaidaHostV1) -> Option<Self> {
        if host.is_null() {
            return None;
        }
        // SAFETY: non-null checked; caller promises validity.
        Some(Self {
            host: unsafe { &*host },
        })
    }

    /// Raw host pointer, for when you need to forward the table manually.
    pub fn as_raw(&self) -> *const TaidaHostV1 {
        self.host as *const _
    }

    /// Construct a `Unit` value.
    pub fn unit(&self) -> *mut TaidaAddonValueV1 {
        (self.host.value_new_unit)(self.host as *const _)
    }

    /// Construct an `Int` value.
    pub fn int(&self, v: i64) -> *mut TaidaAddonValueV1 {
        (self.host.value_new_int)(self.host as *const _, v)
    }

    /// Construct a `Float` value.
    pub fn float(&self, v: f64) -> *mut TaidaAddonValueV1 {
        (self.host.value_new_float)(self.host as *const _, v)
    }

    /// Construct a `Bool` value.
    pub fn bool(&self, v: bool) -> *mut TaidaAddonValueV1 {
        (self.host.value_new_bool)(self.host as *const _, u8::from(v))
    }

    /// Construct a `Str` value. `s` is copied by the host.
    pub fn str(&self, s: &str) -> *mut TaidaAddonValueV1 {
        let bytes = s.as_bytes();
        (self.host.value_new_str)(self.host as *const _, bytes.as_ptr(), bytes.len())
    }

    /// Construct a `Bytes` value. `data` is copied by the host.
    pub fn bytes(&self, data: &[u8]) -> *mut TaidaAddonValueV1 {
        (self.host.value_new_bytes)(self.host as *const _, data.as_ptr(), data.len())
    }

    /// Construct a `List` value from already-built child pointers.
    ///
    /// Ownership of each child **moves into** the new list; do not
    /// release them afterwards.
    pub fn list(&self, items: &[*mut TaidaAddonValueV1]) -> *mut TaidaAddonValueV1 {
        (self.host.value_new_list)(self.host as *const _, items.as_ptr(), items.len())
    }

    /// Construct a `Pack` value from parallel name / value arrays.
    ///
    /// `names` must all be nul-terminated UTF-8 C strings. Ownership of
    /// each value pointer moves into the new pack.
    pub fn pack(
        &self,
        names: &[*const c_char],
        values: &[*mut TaidaAddonValueV1],
    ) -> *mut TaidaAddonValueV1 {
        // Parallel arrays must be the same length; callers typically
        // construct them from a Vec<(&CStr, *mut _)> split.
        let len = core::cmp::min(names.len(), values.len());
        (self.host.value_new_pack)(self.host as *const _, names.as_ptr(), values.as_ptr(), len)
    }

    /// Release a host-owned value. Addons should normally not call this
    /// — the host frees return values automatically. It exists so
    /// addons can roll back a partially built list / pack on error.
    ///
    /// # Safety
    ///
    /// `value` must be a pointer previously returned by one of the
    /// `value_new_*` callbacks on this same host, and must not have
    /// already been released or handed to the host.
    pub unsafe fn release(&self, value: *mut TaidaAddonValueV1) {
        (self.host.value_release)(self.host as *const _, value);
    }

    /// Construct an error value. `msg` is copied by the host.
    pub fn error(&self, code: u32, msg: &str) -> *mut TaidaAddonErrorV1 {
        let bytes = msg.as_bytes();
        (self.host.error_new)(self.host as *const _, code, bytes.as_ptr(), bytes.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::{
        TaidaAddonBoolPayload, TaidaAddonBytesPayload, TaidaAddonFloatPayload,
        TaidaAddonIntPayload, TaidaAddonListPayload, TaidaAddonPackEntryV1, TaidaAddonPackPayload,
    };
    use core::ffi::c_void;

    // Unit tests here construct synthetic `TaidaAddonValueV1` instances
    // on the stack so we can exercise the borrow side of the bridge
    // without running a real addon. The host-callback side is
    // exercised by the workspace's `taida` crate (`src/addon/loader.rs`
    // and `tests/addon_loader_smoke.rs`).

    fn make_int(payload: &TaidaAddonIntPayload) -> TaidaAddonValueV1 {
        TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Int as u32,
            _reserved: 0,
            payload: payload as *const _ as *mut c_void,
        }
    }

    fn make_float(payload: &TaidaAddonFloatPayload) -> TaidaAddonValueV1 {
        TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Float as u32,
            _reserved: 0,
            payload: payload as *const _ as *mut c_void,
        }
    }

    fn make_bool(payload: &TaidaAddonBoolPayload) -> TaidaAddonValueV1 {
        TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Bool as u32,
            _reserved: 0,
            payload: payload as *const _ as *mut c_void,
        }
    }

    fn make_str(payload: &TaidaAddonBytesPayload) -> TaidaAddonValueV1 {
        TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Str as u32,
            _reserved: 0,
            payload: payload as *const _ as *mut c_void,
        }
    }

    fn make_bytes(payload: &TaidaAddonBytesPayload) -> TaidaAddonValueV1 {
        TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Bytes as u32,
            _reserved: 0,
            payload: payload as *const _ as *mut c_void,
        }
    }

    fn make_list(payload: &TaidaAddonListPayload) -> TaidaAddonValueV1 {
        TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::List as u32,
            _reserved: 0,
            payload: payload as *const _ as *mut c_void,
        }
    }

    fn make_pack(payload: &TaidaAddonPackPayload) -> TaidaAddonValueV1 {
        TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Pack as u32,
            _reserved: 0,
            payload: payload as *const _ as *mut c_void,
        }
    }

    #[test]
    fn borrowed_value_reads_int() {
        let p = TaidaAddonIntPayload { value: 1234 };
        let v = make_int(&p);
        let b = BorrowedValue { raw: &v };
        assert_eq!(b.tag(), Some(TaidaAddonValueTag::Int));
        assert_eq!(b.as_int(), Some(1234));
        assert_eq!(b.as_float(), None);
    }

    #[test]
    fn borrowed_value_reads_float() {
        let p = TaidaAddonFloatPayload { value: -2.5 };
        let v = make_float(&p);
        let b = BorrowedValue { raw: &v };
        assert_eq!(b.tag(), Some(TaidaAddonValueTag::Float));
        assert_eq!(b.as_float(), Some(-2.5));
    }

    #[test]
    fn borrowed_value_reads_bool() {
        let t = TaidaAddonBoolPayload { value: 1 };
        let f = TaidaAddonBoolPayload { value: 0 };
        assert_eq!(
            BorrowedValue {
                raw: &make_bool(&t)
            }
            .as_bool(),
            Some(true)
        );
        assert_eq!(
            BorrowedValue {
                raw: &make_bool(&f)
            }
            .as_bool(),
            Some(false)
        );
    }

    // RC1B-109 regression: invalid bool payload (anything outside
    // the 0/1 whitelist) must be rejected, not silently coerced to
    // `true`.
    #[test]
    fn borrowed_value_rejects_invalid_bool_payload_2() {
        let p = TaidaAddonBoolPayload { value: 2 };
        let v = make_bool(&p);
        let b = BorrowedValue { raw: &v };
        assert_eq!(
            b.as_bool(),
            None,
            "invalid bool payload (2) must yield None, not true"
        );
    }

    #[test]
    fn borrowed_value_rejects_invalid_bool_payload_255() {
        let p = TaidaAddonBoolPayload { value: 255 };
        let v = make_bool(&p);
        let b = BorrowedValue { raw: &v };
        assert_eq!(
            b.as_bool(),
            None,
            "invalid bool payload (255) must yield None, not true"
        );
    }

    #[test]
    fn borrowed_value_reads_str() {
        let s = b"hello-addon";
        let p = TaidaAddonBytesPayload {
            ptr: s.as_ptr(),
            len: s.len(),
        };
        let v = make_str(&p);
        let b = BorrowedValue { raw: &v };
        assert_eq!(b.tag(), Some(TaidaAddonValueTag::Str));
        assert_eq!(b.as_str(), Some("hello-addon"));
        // Str is not a Bytes — the accessor must reject cross-tag reads.
        assert_eq!(b.as_bytes(), None);
    }

    #[test]
    fn borrowed_value_reads_bytes() {
        let buf = [0x00u8, 0x01, 0xff, 0x7f];
        let p = TaidaAddonBytesPayload {
            ptr: buf.as_ptr(),
            len: buf.len(),
        };
        let v = make_bytes(&p);
        let b = BorrowedValue { raw: &v };
        assert_eq!(b.tag(), Some(TaidaAddonValueTag::Bytes));
        assert_eq!(b.as_bytes(), Some(&buf[..]));
        assert_eq!(b.as_str(), None);
    }

    #[test]
    fn borrowed_value_reads_list() {
        let a_payload = TaidaAddonIntPayload { value: 10 };
        let b_payload = TaidaAddonIntPayload { value: 20 };
        let mut a = make_int(&a_payload);
        let mut b = make_int(&b_payload);
        let items: [*mut TaidaAddonValueV1; 2] = [&mut a as *mut _, &mut b as *mut _];
        let lp = TaidaAddonListPayload {
            items: items.as_ptr(),
            len: items.len(),
        };
        let v = make_list(&lp);
        let bv = BorrowedValue { raw: &v };
        let list = bv.as_list().expect("list view");
        assert_eq!(list.len(), 2);
        assert_eq!(list.get(0).and_then(|x| x.as_int()), Some(10));
        assert_eq!(list.get(1).and_then(|x| x.as_int()), Some(20));
        let iter_vals: Vec<i64> = list.iter().filter_map(|v| v.as_int()).collect();
        assert_eq!(iter_vals, vec![10, 20]);
    }

    #[test]
    fn borrowed_value_reads_pack() {
        let ip = TaidaAddonIntPayload { value: 99 };
        let mut iv = make_int(&ip);
        // Name must be nul-terminated.
        let name_bytes = b"count\0";
        let entries = [TaidaAddonPackEntryV1 {
            name: name_bytes.as_ptr() as *const c_char,
            value: &mut iv as *mut _,
        }];
        let pp = TaidaAddonPackPayload {
            entries: entries.as_ptr(),
            len: entries.len(),
        };
        let v = make_pack(&pp);
        let bv = BorrowedValue { raw: &v };
        let pack = bv.as_pack().expect("pack view");
        let collected: Vec<(&str, i64)> = pack
            .iter()
            .filter_map(|(n, v)| v.as_int().map(|i| (n, i)))
            .collect();
        assert_eq!(collected, vec![("count", 99)]);
    }

    #[test]
    fn borrowed_value_rejects_cross_tag_access() {
        // An Int must not decode as Float even if the payload pointer
        // is reinterpretable — the accessor keys on `tag`.
        let p = TaidaAddonIntPayload { value: 7 };
        let v = make_int(&p);
        let b = BorrowedValue { raw: &v };
        assert_eq!(b.as_float(), None);
        assert_eq!(b.as_bool(), None);
        assert_eq!(b.as_str(), None);
        assert_eq!(b.as_bytes(), None);
        assert!(b.as_list().is_none());
        assert!(b.as_pack().is_none());
    }

    #[test]
    fn borrowed_args_rejects_null_and_zero() {
        // SAFETY: we pass null + 0 which is always safe (documented).
        let slice = unsafe { borrowed_args(core::ptr::null(), 0) };
        assert!(slice.is_empty());
    }

    #[test]
    fn borrowed_args_reads_single_element() {
        let p = TaidaAddonIntPayload { value: 42 };
        let v = make_int(&p);
        let args = [v];
        // SAFETY: args lives for the whole test scope.
        let slice = unsafe { borrowed_args(args.as_ptr(), 1) };
        assert_eq!(slice.len(), 1);
        let b = BorrowedValue { raw: &slice[0] };
        assert_eq!(b.as_int(), Some(42));
    }

    #[test]
    fn unknown_tag_yields_none() {
        let v = TaidaAddonValueV1 {
            tag: 999, // outside the frozen table
            _reserved: 0,
            payload: core::ptr::null_mut(),
        };
        let b = BorrowedValue { raw: &v };
        assert_eq!(b.tag(), None);
        assert_eq!(b.raw_tag(), 999);
    }
}
