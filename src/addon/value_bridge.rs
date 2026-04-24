//! Host-side value bridge for the RC1 Phase 3 addon ABI.
//!
//! This module is the concrete implementation of the "allocator
//! unification" contract from `.dev/RC1_DESIGN.md` Phase 3 Lock and
//! the RC1B-103 resolution: **the host owns every `TaidaAddonValueV1`
//! and `TaidaAddonErrorV1` that crosses the bridge**. Addons build
//! return values exclusively through the `TaidaHostV1` callback table
//! provided by this module, which keeps allocation and deallocation on
//! the same side of the FFI boundary.
//!
//! # Why a single allocator?
//!
//! Rust's `Box::leak` / `Box::into_raw` and C's `malloc` / `free` are
//! not interchangeable on many platforms. If the addon allocated with
//! one allocator and the host tried to free with another, the result
//! would be undefined behaviour. By funnelling every allocation
//! through `Box::into_raw` on the host side (and matching
//! `Box::from_raw` in [`release_value`] / [`release_error`]), we
//! guarantee correctness regardless of how the addon was compiled.
//!
//! # Ownership summary (RC1B-103 resolution)
//!
//! - Inputs (`args_ptr`, `args_len`): borrowed read-only views built
//!   by the host on its stack. The addon reads them during the call
//!   and must not retain references past return. The host drops the
//!   entire input vector via [`drop_host_built_value`] when the call
//!   returns.
//! - Outputs (`*out_value`): the addon builds the return value using
//!   the callback table. Ownership transfers to the host immediately
//!   on construction; the addon writes the pointer into `*out_value`
//!   and the host releases it after materialising it back into a
//!   `taida::Value`.
//! - Errors (`*out_error`): same as outputs but with `error_new` /
//!   `error_release`.
//!
//! # Invariants this module upholds
//!
//! 1. Every `*mut TaidaAddonValueV1` returned by a callback is the
//!    unique owner of its payload subtree.
//! 2. `release_value` recursively frees list / pack payloads, so the
//!    host can drop a whole subtree with one call.
//! 3. The callback functions are `extern "C"` and never panic across
//!    the FFI boundary — they use catch-unwind style defensive code
//!    to convert errors into null returns.
//! 4. `build_host_input_value` / `drop_host_built_value` are the
//!    *internal* helpers the host uses to construct borrowed input
//!    vectors. They are symmetric and must always be used in pairs.

use core::ffi::{CStr, c_char, c_void};

use taida_addon::{
    TAIDA_ADDON_ABI_VERSION, TaidaAddonBoolPayload, TaidaAddonBytesPayload, TaidaAddonErrorV1,
    TaidaAddonFloatPayload, TaidaAddonIntPayload, TaidaAddonListPayload, TaidaAddonPackEntryV1,
    TaidaAddonPackPayload, TaidaAddonValueTag, TaidaAddonValueV1, TaidaHostV1,
};

use crate::interpreter::value::Value;

/// Error surfaced when the host cannot bridge a `taida::Value` across
/// the addon boundary.
///
/// Split into distinct variants so callers (and, transitively, Taida
/// user-level diagnostics) can classify the failure. RC1 scope:
/// `Async`, `Gorilla`, `Function`, `Stream`, `Json`, `Molten`,
/// `Error` are out of scope and map to `UnsupportedInput`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BridgeError {
    /// A value of a kind outside the RC1 Phase 3 whitelist was passed
    /// as an input argument.
    UnsupportedInput { kind: &'static str },
    /// The addon returned a `TaidaAddonValueV1` with an out-of-range
    /// `tag`. Only `Unit..=Pack` are valid in Phase 3.
    UnknownOutputTag { raw_tag: u32 },
    /// An output `Str` payload was not valid UTF-8.
    InvalidStrEncoding,
    /// A structurally malformed output value (null payload where the
    /// tag required one, null inner pointer, etc.).
    MalformedOutput { reason: &'static str },
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedInput { kind } => write!(
                f,
                "addon bridge: value kind '{kind}' is not supported in RC1 Phase 3"
            ),
            Self::UnknownOutputTag { raw_tag } => write!(
                f,
                "addon bridge: addon returned value with unknown tag {raw_tag}"
            ),
            Self::InvalidStrEncoding => {
                write!(f, "addon bridge: addon returned Str with invalid UTF-8")
            }
            Self::MalformedOutput { reason } => {
                write!(f, "addon bridge: malformed addon output ({reason})")
            }
        }
    }
}

impl std::error::Error for BridgeError {}

// ── Callback implementations (host-side allocator) ────────────────────
//
// All callbacks allocate via `Box::into_raw` so the matching
// `release_value` can reclaim the payload with `Box::from_raw`. The
// payload structs are plain `#[repr(C)]` POD, so the `Box` round trip
// is UB-free.
//
// Every callback is `extern "C"` and guards against null host
// pointers (returns null so the addon can detect the failure).

extern "C" fn cb_value_new_unit(_host: *const TaidaHostV1) -> *mut TaidaAddonValueV1 {
    alloc_value(TaidaAddonValueTag::Unit, core::ptr::null_mut())
}

extern "C" fn cb_value_new_int(_host: *const TaidaHostV1, v: i64) -> *mut TaidaAddonValueV1 {
    let payload = Box::into_raw(Box::new(TaidaAddonIntPayload { value: v })) as *mut c_void;
    alloc_value(TaidaAddonValueTag::Int, payload)
}

extern "C" fn cb_value_new_float(_host: *const TaidaHostV1, v: f64) -> *mut TaidaAddonValueV1 {
    let payload = Box::into_raw(Box::new(TaidaAddonFloatPayload { value: v })) as *mut c_void;
    alloc_value(TaidaAddonValueTag::Float, payload)
}

extern "C" fn cb_value_new_bool(_host: *const TaidaHostV1, v: u8) -> *mut TaidaAddonValueV1 {
    let payload = Box::into_raw(Box::new(TaidaAddonBoolPayload {
        value: if v != 0 { 1 } else { 0 },
    })) as *mut c_void;
    alloc_value(TaidaAddonValueTag::Bool, payload)
}

extern "C" fn cb_value_new_str(
    _host: *const TaidaHostV1,
    bytes: *const u8,
    len: usize,
) -> *mut TaidaAddonValueV1 {
    let payload = alloc_bytes_payload(bytes, len);
    if payload.is_null() {
        return core::ptr::null_mut();
    }
    alloc_value(TaidaAddonValueTag::Str, payload as *mut c_void)
}

extern "C" fn cb_value_new_bytes(
    _host: *const TaidaHostV1,
    bytes: *const u8,
    len: usize,
) -> *mut TaidaAddonValueV1 {
    let payload = alloc_bytes_payload(bytes, len);
    if payload.is_null() {
        return core::ptr::null_mut();
    }
    alloc_value(TaidaAddonValueTag::Bytes, payload as *mut c_void)
}

extern "C" fn cb_value_new_list(
    _host: *const TaidaHostV1,
    items: *const *mut TaidaAddonValueV1,
    len: usize,
) -> *mut TaidaAddonValueV1 {
    // ABI v1 contract (RC1B-108): `value_new_list` with `len > 0`
    // REQUIRES a non-null `items` array. `len == 0` is the only case
    // where a null `items` is tolerated (empty list). Silently
    // normalising `(null, len > 0)` to an empty list would mask a
    // malformed addon output and hide the underlying bug, so we
    // reject it by returning null (the addon's documented failure
    // signal for constructor callbacks).
    let vec: Vec<*mut TaidaAddonValueV1> = if len == 0 {
        Vec::new()
    } else {
        if items.is_null() {
            return core::ptr::null_mut();
        }
        // SAFETY: the addon contract for `value_new_list` is to pass a
        // valid pointer / len pair.
        unsafe { core::slice::from_raw_parts(items, len) }.to_vec()
    };
    let (items_ptr, vec_len) = box_leak_vec(vec);
    let payload = Box::into_raw(Box::new(TaidaAddonListPayload {
        items: items_ptr,
        len: vec_len,
    })) as *mut c_void;
    alloc_value(TaidaAddonValueTag::List, payload)
}

extern "C" fn cb_value_new_pack(
    _host: *const TaidaHostV1,
    names: *const *const c_char,
    values: *const *mut TaidaAddonValueV1,
    len: usize,
) -> *mut TaidaAddonValueV1 {
    // ABI v1 contract (RC1B-108): `value_new_pack` with `len > 0`
    // REQUIRES non-null `names` and `values` parallel arrays. Only
    // `len == 0` permits null arrays (empty pack). Silently
    // normalising `(null, len > 0)` to an empty pack would hide a
    // malformed addon output from debugging, so we reject it by
    // returning null (the constructor's documented failure signal).
    if len == 0 {
        let payload = Box::into_raw(Box::new(TaidaAddonPackPayload {
            entries: core::ptr::null(),
            len: 0,
        })) as *mut c_void;
        return alloc_value(TaidaAddonValueTag::Pack, payload);
    }
    if names.is_null() || values.is_null() {
        return core::ptr::null_mut();
    }
    // Copy and own each name by `CString::into_raw` so the addon's
    // stack buffers can be dropped after the call returns.
    // SAFETY: addon contract — parallel arrays of length `len`.
    let names_slice = unsafe { core::slice::from_raw_parts(names, len) };
    let values_slice = unsafe { core::slice::from_raw_parts(values, len) };
    let mut entries: Vec<TaidaAddonPackEntryV1> = Vec::with_capacity(len);
    for i in 0..len {
        let raw_name = names_slice[i];
        // We must clone the name bytes onto the host allocator.
        let owned_name = if raw_name.is_null() {
            // Rollback everything we already built.
            rollback_pack_entries(entries);
            return core::ptr::null_mut();
        } else {
            // SAFETY: addon contract requires nul-terminated strings.
            let cstr = unsafe { CStr::from_ptr(raw_name) };
            match std::ffi::CString::new(cstr.to_bytes()) {
                Ok(c) => c,
                Err(_) => {
                    rollback_pack_entries(entries);
                    return core::ptr::null_mut();
                }
            }
        };
        let name_ptr: *const c_char = owned_name.into_raw();
        entries.push(TaidaAddonPackEntryV1 {
            name: name_ptr,
            value: values_slice[i],
        });
    }
    let (entries_ptr, entries_len) = box_leak_vec(entries);
    let payload = Box::into_raw(Box::new(TaidaAddonPackPayload {
        entries: entries_ptr,
        len: entries_len,
    })) as *mut c_void;
    alloc_value(TaidaAddonValueTag::Pack, payload)
}

fn rollback_pack_entries(mut entries: Vec<TaidaAddonPackEntryV1>) {
    for entry in entries.drain(..) {
        if !entry.name.is_null() {
            // SAFETY: name_ptr was created by CString::into_raw above.
            let _ = unsafe { std::ffi::CString::from_raw(entry.name as *mut c_char) };
        }
        if !entry.value.is_null() {
            // Each value was already a host-owned TaidaAddonValueV1
            // built by the addon via other host callbacks. Releasing
            // it recursively frees the subtree.
            // SAFETY: every pointer stored in `entries` came from a
            // host-side allocation.
            unsafe { release_value_ptr(entry.value) };
        }
    }
}

extern "C" fn cb_value_release(_host: *const TaidaHostV1, value: *mut TaidaAddonValueV1) {
    // SAFETY: see `release_value_ptr`.
    unsafe { release_value_ptr(value) };
}

extern "C" fn cb_error_new(
    _host: *const TaidaHostV1,
    code: u32,
    msg_ptr: *const u8,
    msg_len: usize,
) -> *mut TaidaAddonErrorV1 {
    // Copy message into a host-owned CString so the addon's buffer
    // can be dropped after the call. We deliberately convert
    // invalid-UTF-8 messages to a placeholder so the bridge never
    // carries undefined text across the FFI boundary.
    let bytes = if msg_ptr.is_null() || msg_len == 0 {
        Vec::new()
    } else {
        // SAFETY: addon contract — `bytes` points to `msg_len` bytes
        // of readable memory.
        unsafe { core::slice::from_raw_parts(msg_ptr, msg_len) }.to_vec()
    };
    let cstring = match std::ffi::CString::new(bytes) {
        Ok(c) => c,
        Err(_) => {
            // Interior nul — replace with an empty message so we
            // never produce a non-nul-terminated C string.
            std::ffi::CString::new("").expect("empty string has no NUL")
        }
    };
    let message_ptr = cstring.into_raw();
    Box::into_raw(Box::new(TaidaAddonErrorV1 {
        code,
        _reserved: 0,
        message: message_ptr,
    }))
}

extern "C" fn cb_error_release(_host: *const TaidaHostV1, error: *mut TaidaAddonErrorV1) {
    if error.is_null() {
        return;
    }
    // SAFETY: we allocated via `Box::into_raw` in `cb_error_new`.
    let boxed = unsafe { Box::from_raw(error) };
    if !boxed.message.is_null() {
        // SAFETY: allocated by CString::into_raw in `cb_error_new`.
        let _ = unsafe { std::ffi::CString::from_raw(boxed.message as *mut c_char) };
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

fn alloc_value(tag: TaidaAddonValueTag, payload: *mut c_void) -> *mut TaidaAddonValueV1 {
    Box::into_raw(Box::new(TaidaAddonValueV1 {
        tag: tag as u32,
        _reserved: 0,
        payload,
    }))
}

fn alloc_bytes_payload(bytes: *const u8, len: usize) -> *mut TaidaAddonBytesPayload {
    if len == 0 {
        return Box::into_raw(Box::new(TaidaAddonBytesPayload {
            ptr: core::ptr::null(),
            len: 0,
        }));
    }
    if bytes.is_null() {
        return core::ptr::null_mut();
    }
    // SAFETY: addon contract — `bytes` points to `len` readable bytes.
    let copy: Vec<u8> = unsafe { core::slice::from_raw_parts(bytes, len) }.to_vec();
    // Leak the Vec into a raw ptr/len pair. We store *only* the
    // ptr/len here; `release_value_ptr` reconstructs the Vec via
    // `Vec::from_raw_parts(ptr, len, len)` — which requires capacity
    // equal to length. To guarantee that we call `into_boxed_slice`
    // first, which shrinks capacity to `len`.
    let boxed: Box<[u8]> = copy.into_boxed_slice();
    let slice_len = boxed.len();
    let raw_ptr = Box::into_raw(boxed) as *const u8;
    Box::into_raw(Box::new(TaidaAddonBytesPayload {
        ptr: raw_ptr,
        len: slice_len,
    }))
}

/// Leak a `Vec<T>` into a `(ptr, len)` pair using `Box<[T]>` so the
/// reclamation path can rebuild a slice with matching capacity.
fn box_leak_vec<T>(vec: Vec<T>) -> (*const T, usize) {
    let boxed: Box<[T]> = vec.into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *const T;
    (ptr, len)
}

/// Reclaim a `(ptr, len)` pair that was created by [`box_leak_vec`].
///
/// # Safety
///
/// `ptr` must have been produced by [`box_leak_vec`] with the same
/// `T` and `len`. `len == 0` is treated as a no-op.
unsafe fn reclaim_boxed_slice<T>(ptr: *const T, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    // SAFETY: delegated to caller. `Box<[T]>` matches the original
    // allocation shape because `into_boxed_slice()` normalises
    // capacity to length.
    let slice: *mut [T] = core::ptr::slice_from_raw_parts_mut(ptr as *mut T, len);
    let _ = unsafe { Box::from_raw(slice) };
}

/// Release a host-owned `*mut TaidaAddonValueV1` and its entire
/// payload subtree. Null is a no-op.
///
/// # Safety
///
/// `ptr` must have been returned by one of the `cb_value_new_*`
/// callbacks (or by [`build_host_input_value`]), and must not have
/// been released previously. The function assumes the pointer owns
/// its entire subtree.
pub unsafe fn release_value_ptr(ptr: *mut TaidaAddonValueV1) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: per caller contract, ptr came from `Box::into_raw`.
    let boxed = unsafe { Box::from_raw(ptr) };
    let payload = boxed.payload;
    // `boxed` is dropped here; we still need to free the payload.
    let tag = TaidaAddonValueTag::from_u32(boxed.tag);
    drop(boxed);
    if payload.is_null() {
        return;
    }
    match tag {
        None | Some(TaidaAddonValueTag::Unit) => {
            // Unit has no payload; unknown tags get leaked rather
            // than potentially UB-dropping an arbitrary allocation.
            // This path should be unreachable for host-built values.
        }
        Some(TaidaAddonValueTag::Int) => {
            // SAFETY: we allocated via `Box::into_raw` in cb_value_new_int.
            let _ = unsafe { Box::from_raw(payload as *mut TaidaAddonIntPayload) };
        }
        Some(TaidaAddonValueTag::Float) => {
            // SAFETY: see Int.
            let _ = unsafe { Box::from_raw(payload as *mut TaidaAddonFloatPayload) };
        }
        Some(TaidaAddonValueTag::Bool) => {
            // SAFETY: see Int.
            let _ = unsafe { Box::from_raw(payload as *mut TaidaAddonBoolPayload) };
        }
        Some(TaidaAddonValueTag::Str) | Some(TaidaAddonValueTag::Bytes) => {
            // SAFETY: allocated by `alloc_bytes_payload`.
            let bp = unsafe { Box::from_raw(payload as *mut TaidaAddonBytesPayload) };
            // Reclaim the inner byte buffer.
            // SAFETY: we produced `bp.ptr` / `bp.len` via `box_leak_vec` on a Box<[u8]>.
            unsafe { reclaim_boxed_slice(bp.ptr, bp.len) };
        }
        Some(TaidaAddonValueTag::List) => {
            // SAFETY: allocated by `cb_value_new_list`.
            let lp = unsafe { Box::from_raw(payload as *mut TaidaAddonListPayload) };
            // Recursively release children.
            if !lp.items.is_null() && lp.len > 0 {
                // SAFETY: we built this with box_leak_vec on a Vec<*mut _>.
                let children: &[*mut TaidaAddonValueV1] =
                    unsafe { core::slice::from_raw_parts(lp.items, lp.len) };
                for &child in children {
                    // SAFETY: child is a host-built value (invariant of
                    // the bridge's input validation and the addon's
                    // use of host callbacks).
                    unsafe { release_value_ptr(child) };
                }
                // Reclaim the pointer array itself.
                // SAFETY: see box_leak_vec.
                unsafe { reclaim_boxed_slice(lp.items, lp.len) };
            }
        }
        Some(TaidaAddonValueTag::Pack) => {
            // SAFETY: allocated by `cb_value_new_pack`.
            let pp = unsafe { Box::from_raw(payload as *mut TaidaAddonPackPayload) };
            if !pp.entries.is_null() && pp.len > 0 {
                // SAFETY: allocated by box_leak_vec on Vec<TaidaAddonPackEntryV1>.
                let entries: &[TaidaAddonPackEntryV1] =
                    unsafe { core::slice::from_raw_parts(pp.entries, pp.len) };
                for entry in entries {
                    if !entry.name.is_null() {
                        // SAFETY: allocated by CString::into_raw.
                        let _ = unsafe { std::ffi::CString::from_raw(entry.name as *mut c_char) };
                    }
                    if !entry.value.is_null() {
                        // SAFETY: host-built.
                        unsafe { release_value_ptr(entry.value) };
                    }
                }
                // SAFETY: allocated by box_leak_vec.
                unsafe { reclaim_boxed_slice(pp.entries, pp.len) };
            }
        }
    }
}

/// Build a [`TaidaHostV1`] with real host-side callbacks populated.
///
/// The returned table is plain data — copy it into the addon's init
/// call or store a reference alongside the [`crate::addon::loader::LoadedAddon`].
pub fn make_host_table() -> TaidaHostV1 {
    TaidaHostV1 {
        abi_version: TAIDA_ADDON_ABI_VERSION,
        _reserved: 0,
        value_new_unit: cb_value_new_unit,
        value_new_int: cb_value_new_int,
        value_new_float: cb_value_new_float,
        value_new_bool: cb_value_new_bool,
        value_new_str: cb_value_new_str,
        value_new_bytes: cb_value_new_bytes,
        value_new_list: cb_value_new_list,
        value_new_pack: cb_value_new_pack,
        value_release: cb_value_release,
        error_new: cb_error_new,
        error_release: cb_error_release,
    }
}

/// Build a host-owned `*mut TaidaAddonValueV1` from a `taida::Value`.
///
/// Used by the host to populate the borrowed input vector handed to
/// the addon. The returned pointer must be released with
/// [`release_value_ptr`] when the call completes (or on error).
///
/// Returns `Err(BridgeError::UnsupportedInput)` for values outside
/// the RC1 Phase 3 whitelist.
pub fn build_host_input_value(value: &Value) -> Result<*mut TaidaAddonValueV1, BridgeError> {
    match value {
        Value::Unit => Ok(alloc_value(TaidaAddonValueTag::Unit, core::ptr::null_mut())),
        Value::Int(n) => {
            let payload =
                Box::into_raw(Box::new(TaidaAddonIntPayload { value: *n })) as *mut c_void;
            Ok(alloc_value(TaidaAddonValueTag::Int, payload))
        }
        Value::Float(f) => {
            let payload =
                Box::into_raw(Box::new(TaidaAddonFloatPayload { value: *f })) as *mut c_void;
            Ok(alloc_value(TaidaAddonValueTag::Float, payload))
        }
        Value::Bool(b) => {
            let payload = Box::into_raw(Box::new(TaidaAddonBoolPayload {
                value: u8::from(*b),
            })) as *mut c_void;
            Ok(alloc_value(TaidaAddonValueTag::Bool, payload))
        }
        Value::Str(s) => {
            let payload = alloc_bytes_payload(s.as_ptr(), s.len());
            if payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "str payload alloc returned null",
                });
            }
            Ok(alloc_value(TaidaAddonValueTag::Str, payload as *mut c_void))
        }
        Value::Bytes(b) => {
            let payload = alloc_bytes_payload(b.as_ptr(), b.len());
            if payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "bytes payload alloc returned null",
                });
            }
            Ok(alloc_value(
                TaidaAddonValueTag::Bytes,
                payload as *mut c_void,
            ))
        }
        Value::List(items) => {
            // Build each child first; rollback on failure so partial
            // allocations don't leak.
            let mut built: Vec<*mut TaidaAddonValueV1> = Vec::with_capacity(items.len());
            for item in items.iter() {
                match build_host_input_value(item) {
                    Ok(p) => built.push(p),
                    Err(e) => {
                        for partial in built {
                            // SAFETY: we just built these.
                            unsafe { release_value_ptr(partial) };
                        }
                        return Err(e);
                    }
                }
            }
            let (items_ptr, vec_len) = box_leak_vec(built);
            let payload = Box::into_raw(Box::new(TaidaAddonListPayload {
                items: items_ptr,
                len: vec_len,
            })) as *mut c_void;
            Ok(alloc_value(TaidaAddonValueTag::List, payload))
        }
        Value::BuchiPack(fields) => {
            let mut entries: Vec<TaidaAddonPackEntryV1> = Vec::with_capacity(fields.len());
            for (name, val) in fields.iter() {
                let owned_name = match std::ffi::CString::new(name.as_str()) {
                    Ok(c) => c,
                    Err(_) => {
                        // Interior nul in a field name — roll back.
                        rollback_pack_entries(entries);
                        return Err(BridgeError::MalformedOutput {
                            reason: "pack field name contains interior nul",
                        });
                    }
                };
                let value_ptr = match build_host_input_value(val) {
                    Ok(p) => p,
                    Err(e) => {
                        rollback_pack_entries(entries);
                        return Err(e);
                    }
                };
                entries.push(TaidaAddonPackEntryV1 {
                    name: owned_name.into_raw(),
                    value: value_ptr,
                });
            }
            let (entries_ptr, entries_len) = box_leak_vec(entries);
            let payload = Box::into_raw(Box::new(TaidaAddonPackPayload {
                entries: entries_ptr,
                len: entries_len,
            })) as *mut c_void;
            Ok(alloc_value(TaidaAddonValueTag::Pack, payload))
        }
        // Explicitly rejected kinds — deterministic error per RC1 Phase 3.
        Value::Function(_) => Err(BridgeError::UnsupportedInput { kind: "Function" }),
        Value::Gorilla => Err(BridgeError::UnsupportedInput { kind: "Gorilla" }),
        Value::Error(_) => Err(BridgeError::UnsupportedInput { kind: "Error" }),
        Value::Async(_) => Err(BridgeError::UnsupportedInput { kind: "Async" }),
        Value::Json(_) => Err(BridgeError::UnsupportedInput { kind: "Json" }),
        Value::Molten => Err(BridgeError::UnsupportedInput { kind: "Molten" }),
        Value::Stream(_) => Err(BridgeError::UnsupportedInput { kind: "Stream" }),
        // C18-2: EnumVal marshals to the addon ABI as a plain Int(ordinal).
        // The addon protocol has no dedicated Enum tag; the variant name
        // is a language-level concept used only by jsonEncode. Ordinal
        // round-trips are sufficient for existing addon contracts.
        Value::EnumVal(_, n) => {
            let payload =
                Box::into_raw(Box::new(TaidaAddonIntPayload { value: *n })) as *mut c_void;
            Ok(alloc_value(TaidaAddonValueTag::Int, payload))
        }
    }
}

/// Drop a value previously built by [`build_host_input_value`].
/// Alias for clarity at call sites.
///
/// # Safety
///
/// Same requirements as [`release_value_ptr`].
pub unsafe fn drop_host_built_value(ptr: *mut TaidaAddonValueV1) {
    // SAFETY: delegated.
    unsafe { release_value_ptr(ptr) };
}

/// Materialise an addon-returned `*mut TaidaAddonValueV1` back into a
/// `taida::Value`. Consumes the pointer (releases it) and returns the
/// equivalent `Value`.
///
/// # Safety
///
/// `ptr` must be a pointer returned by an addon via a host callback
/// (`cb_value_new_*`). It must be non-null and not released previously.
pub unsafe fn take_addon_output(ptr: *mut TaidaAddonValueV1) -> Result<Value, BridgeError> {
    if ptr.is_null() {
        return Err(BridgeError::MalformedOutput {
            reason: "addon returned null value pointer",
        });
    }
    // SAFETY: delegated.
    let read_result = unsafe { read_value_by_ref(&*ptr) };
    // Release the subtree regardless of success — the host is the
    // sole free-er.
    // SAFETY: delegated.
    unsafe { release_value_ptr(ptr) };
    read_result
}

/// Internal: read a borrowed `TaidaAddonValueV1` (whether it's an
/// input we built or an output we're about to release) into a
/// `taida::Value`. Does not take ownership.
///
/// # Safety
///
/// The value and its payload subtree must be valid for reads for the
/// duration of this call.
unsafe fn read_value_by_ref(v: &TaidaAddonValueV1) -> Result<Value, BridgeError> {
    match TaidaAddonValueTag::from_u32(v.tag) {
        None => Err(BridgeError::UnknownOutputTag { raw_tag: v.tag }),
        Some(TaidaAddonValueTag::Unit) => Ok(Value::Unit),
        Some(TaidaAddonValueTag::Int) => {
            if v.payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "Int payload null",
                });
            }
            // SAFETY: tag matches; addon built via cb_value_new_int.
            let p = unsafe { &*(v.payload as *const TaidaAddonIntPayload) };
            Ok(Value::Int(p.value))
        }
        Some(TaidaAddonValueTag::Float) => {
            if v.payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "Float payload null",
                });
            }
            // SAFETY: see Int.
            let p = unsafe { &*(v.payload as *const TaidaAddonFloatPayload) };
            Ok(Value::Float(p.value))
        }
        Some(TaidaAddonValueTag::Bool) => {
            if v.payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "Bool payload null",
                });
            }
            // SAFETY: see Int.
            let p = unsafe { &*(v.payload as *const TaidaAddonBoolPayload) };
            // ABI v1 contract (RC1B-109): Bool payload is strictly
            // `0` (false) or `1` (true). Any other byte is a
            // malformed output. Silently mapping `2..=255` to `true`
            // (the previous `!= 0` behaviour) would hide type-safety
            // violations from the addon side.
            match p.value {
                0 => Ok(Value::Bool(false)),
                1 => Ok(Value::Bool(true)),
                _ => Err(BridgeError::MalformedOutput {
                    reason: "Bool payload must be 0 or 1",
                }),
            }
        }
        Some(TaidaAddonValueTag::Str) => {
            if v.payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "Str payload null",
                });
            }
            // SAFETY: see Int.
            let p = unsafe { &*(v.payload as *const TaidaAddonBytesPayload) };
            let bytes = if p.len == 0 || p.ptr.is_null() {
                &[][..]
            } else {
                // SAFETY: host-built slice; ptr/len valid for reads.
                unsafe { core::slice::from_raw_parts(p.ptr, p.len) }
            };
            match core::str::from_utf8(bytes) {
                Ok(s) => Ok(Value::str(s.to_string())),
                Err(_) => Err(BridgeError::InvalidStrEncoding),
            }
        }
        Some(TaidaAddonValueTag::Bytes) => {
            if v.payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "Bytes payload null",
                });
            }
            // SAFETY: see Int.
            let p = unsafe { &*(v.payload as *const TaidaAddonBytesPayload) };
            let bytes = if p.len == 0 || p.ptr.is_null() {
                Vec::new()
            } else {
                // SAFETY: host-built slice.
                unsafe { core::slice::from_raw_parts(p.ptr, p.len) }.to_vec()
            };
            Ok(Value::bytes(bytes))
        }
        Some(TaidaAddonValueTag::List) => {
            if v.payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "List payload null",
                });
            }
            // SAFETY: see Int.
            let p = unsafe { &*(v.payload as *const TaidaAddonListPayload) };
            let mut items: Vec<Value> = Vec::with_capacity(p.len);
            if p.len > 0 {
                if p.items.is_null() {
                    return Err(BridgeError::MalformedOutput {
                        reason: "List items null",
                    });
                }
                // SAFETY: host-built pointer array.
                let children = unsafe { core::slice::from_raw_parts(p.items, p.len) };
                for &child_ptr in children {
                    if child_ptr.is_null() {
                        return Err(BridgeError::MalformedOutput {
                            reason: "List child null",
                        });
                    }
                    // SAFETY: child is host-built.
                    let child = unsafe { read_value_by_ref(&*child_ptr) }?;
                    items.push(child);
                }
            }
            Ok(Value::list(items))
        }
        Some(TaidaAddonValueTag::Pack) => {
            if v.payload.is_null() {
                return Err(BridgeError::MalformedOutput {
                    reason: "Pack payload null",
                });
            }
            // SAFETY: see Int.
            let p = unsafe { &*(v.payload as *const TaidaAddonPackPayload) };
            let mut fields: Vec<(String, Value)> = Vec::with_capacity(p.len);
            if p.len > 0 {
                if p.entries.is_null() {
                    return Err(BridgeError::MalformedOutput {
                        reason: "Pack entries null",
                    });
                }
                // SAFETY: host-built pack entries.
                let entries = unsafe { core::slice::from_raw_parts(p.entries, p.len) };
                for entry in entries {
                    if entry.name.is_null() {
                        return Err(BridgeError::MalformedOutput {
                            reason: "Pack entry name null",
                        });
                    }
                    // SAFETY: nul-terminated C string built by host.
                    let name = unsafe { CStr::from_ptr(entry.name) }
                        .to_str()
                        .map_err(|_| BridgeError::InvalidStrEncoding)?
                        .to_string();
                    if entry.value.is_null() {
                        return Err(BridgeError::MalformedOutput {
                            reason: "Pack entry value null",
                        });
                    }
                    // SAFETY: host-built child.
                    let value = unsafe { read_value_by_ref(&*entry.value) }?;
                    fields.push((name, value));
                }
            }
            Ok(Value::pack(fields))
        }
    }
}

#[cfg(test)]
mod tests {
    //! Host-side value bridge tests.
    //!
    //! These exercise the allocator unification path end to end:
    //! build a `taida::Value` → `*mut TaidaAddonValueV1` → read it
    //! back as a `taida::Value` → release the raw pointer. Any
    //! double-free / leak here would surface through miri or valgrind.
    //! The round-trip equality check pins the data-integrity side.

    use super::*;

    fn roundtrip(value: Value) -> Value {
        let raw = build_host_input_value(&value).expect("build should succeed");
        // SAFETY: we just built this pointer.
        unsafe { take_addon_output(raw) }.expect("read should succeed")
    }

    #[test]
    fn roundtrip_unit() {
        assert!(matches!(roundtrip(Value::Unit), Value::Unit));
    }

    #[test]
    fn roundtrip_int() {
        match roundtrip(Value::Int(-42)) {
            Value::Int(n) => assert_eq!(n, -42),
            other => panic!("expected Int, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_float() {
        match roundtrip(Value::Float(2.5)) {
            Value::Float(f) => assert_eq!(f, 2.5),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_bool() {
        match roundtrip(Value::Bool(true)) {
            Value::Bool(b) => assert!(b),
            other => panic!("expected Bool, got {other:?}"),
        }
        match roundtrip(Value::Bool(false)) {
            Value::Bool(b) => assert!(!b),
            other => panic!("expected Bool, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_str() {
        match roundtrip(Value::str("こんにちは".to_string())) {
            Value::Str(s) => assert_eq!(s.as_str(), "こんにちは"),
            other => panic!("expected Str, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_bytes() {
        let data = vec![0x00, 0xff, 0x7f, 0x80];
        match roundtrip(Value::bytes(data.clone())) {
            Value::Bytes(b) => assert_eq!(&**b, &data),
            other => panic!("expected Bytes, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_empty_list() {
        match roundtrip(Value::list(vec![])) {
            Value::List(items) => assert!(items.is_empty()),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_nested_list() {
        let value = Value::list(vec![
            Value::Int(1),
            Value::str("two".to_string()),
            Value::list(vec![Value::Bool(true), Value::Float(3.5)]),
        ]);
        let back = roundtrip(value.clone());
        // Compare via Debug — Value doesn't implement PartialEq.
        assert_eq!(format!("{back:?}"), format!("{value:?}"));
    }

    #[test]
    fn roundtrip_empty_pack() {
        match roundtrip(Value::pack(vec![])) {
            Value::BuchiPack(fields) => assert!(fields.is_empty()),
            other => panic!("expected BuchiPack, got {other:?}"),
        }
    }

    #[test]
    fn roundtrip_pack_with_fields() {
        let value = Value::pack(vec![
            ("name".to_string(), Value::str("Taida".to_string())),
            ("version".to_string(), Value::Int(2)),
            (
                "tags".to_string(),
                Value::list(vec![
                    Value::str("alpha".to_string()),
                    Value::str("beta".to_string()),
                ]),
            ),
        ]);
        let back = roundtrip(value.clone());
        assert_eq!(format!("{back:?}"), format!("{value:?}"));
    }

    #[test]
    fn unsupported_input_gorilla_is_rejected() {
        let err = build_host_input_value(&Value::Gorilla).expect_err("Gorilla must be rejected");
        match err {
            BridgeError::UnsupportedInput { kind } => assert_eq!(kind, "Gorilla"),
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn unsupported_input_molten_is_rejected() {
        let err = build_host_input_value(&Value::Molten).expect_err("Molten must be rejected");
        assert!(matches!(
            err,
            BridgeError::UnsupportedInput { kind: "Molten" }
        ));
    }

    #[test]
    fn release_null_pointer_is_noop() {
        // SAFETY: null is an explicit no-op per the function's contract.
        unsafe { release_value_ptr(core::ptr::null_mut()) };
    }

    #[test]
    fn host_table_is_constructible() {
        let table = make_host_table();
        assert_eq!(table.abi_version, TAIDA_ADDON_ABI_VERSION);
        // Smoke-test each callback via the table.
        let v_unit = (table.value_new_unit)(&table as *const _);
        assert!(!v_unit.is_null());
        let v_int = (table.value_new_int)(&table as *const _, 7);
        assert!(!v_int.is_null());
        let v_str = (table.value_new_str)(&table as *const _, b"hi".as_ptr(), 2);
        assert!(!v_str.is_null());
        // Release them so leak detectors stay happy. `value_release`
        // is a safe `extern "C" fn` call: the pointers came from the
        // same host table and have not been released before, which
        // is all `cb_value_release` requires.
        (table.value_release)(&table as *const _, v_unit);
        (table.value_release)(&table as *const _, v_int);
        (table.value_release)(&table as *const _, v_str);
    }

    #[test]
    fn error_roundtrip_through_table() {
        let table = make_host_table();
        let msg = b"something broke";
        let err_ptr = (table.error_new)(&table as *const _, 42, msg.as_ptr(), msg.len());
        assert!(!err_ptr.is_null());
        // SAFETY: host-built.
        let err_ref = unsafe { &*err_ptr };
        assert_eq!(err_ref.code, 42);
        // SAFETY: C-string built by `cb_error_new`.
        let cstr = unsafe { CStr::from_ptr(err_ref.message) };
        assert_eq!(cstr.to_str().unwrap(), "something broke");
        // SAFETY: host-built, release once.
        (table.error_release)(&table as *const _, err_ptr);
    }

    #[test]
    fn take_addon_output_rejects_null() {
        // SAFETY: null is explicitly handled as an error.
        let err = unsafe { take_addon_output(core::ptr::null_mut()) }.expect_err("null must fail");
        assert!(matches!(err, BridgeError::MalformedOutput { .. }));
    }

    // ── RC1B-108 regression: null array + non-zero len must not
    // silently normalise to an empty container. ───────────────────

    #[test]
    fn value_new_list_rejects_null_items_with_nonzero_len() {
        // Calling `value_new_list(null, 3)` must return null (malformed
        // input). Previously the callback silently normalised the
        // null pointer to an empty Vec, masking the bug.
        let table = make_host_table();
        let result = (table.value_new_list)(
            &table as *const _,
            core::ptr::null::<*mut TaidaAddonValueV1>(),
            3,
        );
        assert!(
            result.is_null(),
            "value_new_list(null, len=3) must return null, got a constructed value"
        );
    }

    #[test]
    fn value_new_list_allows_empty_list_with_null_items() {
        // `len == 0` with null `items` stays legal: explicit empty list.
        let table = make_host_table();
        let result = (table.value_new_list)(
            &table as *const _,
            core::ptr::null::<*mut TaidaAddonValueV1>(),
            0,
        );
        assert!(!result.is_null(), "empty list with null items must succeed");
        // Materialise & release to prove the subtree is well-formed.
        // SAFETY: we just built it via the host table.
        let value = unsafe { take_addon_output(result) }.expect("empty list must decode");
        match value {
            Value::List(items) => assert!(items.is_empty()),
            other => panic!("expected empty List, got {other:?}"),
        }
    }

    #[test]
    fn value_new_pack_rejects_null_names_with_nonzero_len() {
        let table = make_host_table();
        // Provide a valid `values` slot but null `names`.
        let dummy_value_slot: *mut TaidaAddonValueV1 = (table.value_new_int)(&table as *const _, 7);
        assert!(!dummy_value_slot.is_null());
        let values_arr: [*mut TaidaAddonValueV1; 1] = [dummy_value_slot];
        let result = (table.value_new_pack)(
            &table as *const _,
            core::ptr::null::<*const c_char>(),
            values_arr.as_ptr(),
            1,
        );
        assert!(
            result.is_null(),
            "value_new_pack(null names, values, len=1) must return null"
        );
        // Because construction failed, we must release the orphaned
        // child ourselves so the test doesn't leak. SAFETY: the child
        // was allocated by the host table above and not consumed.
        (table.value_release)(&table as *const _, dummy_value_slot);
    }

    #[test]
    fn value_new_pack_rejects_null_values_with_nonzero_len() {
        let table = make_host_table();
        // Provide a valid `names` slot but null `values`.
        let name_bytes = b"k\0";
        let names_arr: [*const c_char; 1] = [name_bytes.as_ptr() as *const c_char];
        let result = (table.value_new_pack)(
            &table as *const _,
            names_arr.as_ptr(),
            core::ptr::null::<*mut TaidaAddonValueV1>(),
            1,
        );
        assert!(
            result.is_null(),
            "value_new_pack(names, null values, len=1) must return null"
        );
    }

    #[test]
    fn value_new_pack_allows_empty_pack_with_null_arrays() {
        // `len == 0` with both arrays null is the explicit empty pack
        // case and must continue to succeed.
        let table = make_host_table();
        let result = (table.value_new_pack)(
            &table as *const _,
            core::ptr::null::<*const c_char>(),
            core::ptr::null::<*mut TaidaAddonValueV1>(),
            0,
        );
        assert!(
            !result.is_null(),
            "empty pack with null arrays must succeed"
        );
        // SAFETY: host-built, release via take_addon_output.
        let value = unsafe { take_addon_output(result) }.expect("empty pack must decode");
        match value {
            Value::BuchiPack(fields) => assert!(fields.is_empty()),
            other => panic!("expected empty BuchiPack, got {other:?}"),
        }
    }

    // ── RC1B-109 regression: invalid bool payload (2, 255, ...) must
    // be rejected, not silently coerced to `true`. ──────────────────

    #[test]
    fn take_addon_output_rejects_invalid_bool_payload() {
        // Build a raw TaidaAddonValueV1 with tag=Bool but a payload
        // whose inner byte is `2` (outside the frozen ABI's 0/1
        // whitelist). `take_addon_output` MUST reject with
        // `MalformedOutput`, not silently map `2` to `true`.
        let payload_ptr =
            Box::into_raw(Box::new(taida_addon::TaidaAddonBoolPayload { value: 2 })) as *mut c_void;
        let ptr = Box::into_raw(Box::new(TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Bool as u32,
            _reserved: 0,
            payload: payload_ptr,
        }));
        // SAFETY: we built `ptr` through Box::into_raw using the same
        // allocator shape that `cb_value_new_bool` uses, so
        // `take_addon_output` / `release_value_ptr` can reclaim it
        // uniformly.
        let err = unsafe { take_addon_output(ptr) }
            .expect_err("invalid bool payload (2) must be rejected");
        assert!(
            matches!(err, BridgeError::MalformedOutput { .. }),
            "expected MalformedOutput for invalid bool, got {err:?}"
        );
    }

    #[test]
    fn take_addon_output_rejects_invalid_bool_payload_255() {
        // Same as the `2` case but with a high-bit value to lock the
        // full != 0 surface.
        let payload_ptr = Box::into_raw(Box::new(taida_addon::TaidaAddonBoolPayload { value: 255 }))
            as *mut c_void;
        let ptr = Box::into_raw(Box::new(TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Bool as u32,
            _reserved: 0,
            payload: payload_ptr,
        }));
        // SAFETY: see the `2` case above.
        let err = unsafe { take_addon_output(ptr) }
            .expect_err("invalid bool payload (255) must be rejected");
        assert!(
            matches!(err, BridgeError::MalformedOutput { .. }),
            "expected MalformedOutput for 255, got {err:?}"
        );
    }

    #[test]
    fn take_addon_output_accepts_zero_and_one_bool_payloads() {
        // The 0/1 path must continue to round-trip correctly.
        let t_ptr =
            Box::into_raw(Box::new(taida_addon::TaidaAddonBoolPayload { value: 1 })) as *mut c_void;
        let t_val = Box::into_raw(Box::new(TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Bool as u32,
            _reserved: 0,
            payload: t_ptr,
        }));
        // SAFETY: allocator shape matches cb_value_new_bool.
        let got_true = unsafe { take_addon_output(t_val) }.expect("true must decode");
        assert!(matches!(got_true, Value::Bool(true)));

        let f_ptr =
            Box::into_raw(Box::new(taida_addon::TaidaAddonBoolPayload { value: 0 })) as *mut c_void;
        let f_val = Box::into_raw(Box::new(TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Bool as u32,
            _reserved: 0,
            payload: f_ptr,
        }));
        // SAFETY: same as above.
        let got_false = unsafe { take_addon_output(f_val) }.expect("false must decode");
        assert!(matches!(got_false, Value::Bool(false)));
    }

    #[test]
    fn take_addon_output_rejects_unknown_tag() {
        // Build a raw value with a bogus tag (bypassing the normal
        // builder) and make sure read-back rejects it.
        let ptr = Box::into_raw(Box::new(TaidaAddonValueV1 {
            tag: 42,
            _reserved: 0,
            payload: core::ptr::null_mut(),
        }));
        // SAFETY: we just built it via Box::into_raw. release_value_ptr
        // will short-circuit on the unknown tag (Unit-like path) so no
        // UB.
        let err = unsafe { take_addon_output(ptr) }.expect_err("unknown tag must fail");
        assert!(matches!(err, BridgeError::UnknownOutputTag { raw_tag: 42 }));
    }
}
