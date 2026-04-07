//! Taida Addon ABI v1 — frozen C ABI surface.
//!
//! This module defines the **frozen** ABI v1 contract referenced by
//! `.dev/RC1_DESIGN.md` (Section C — ABI v1 freeze) and
//! `.dev/RC1_IMPL_SPEC.md` (Phase 0 Frozen Contracts).
//!
//! Non-negotiable invariants (do not change without bumping the ABI version):
//!
//! 1. ABI version is `1`.
//! 2. The host looks up the symbol `taida_addon_get_v1`.
//! 3. `taida_addon_get_v1` is `extern "C"` and returns `*const TaidaAddonDescriptorV1`.
//! 4. The host validates `descriptor.abi_version == TAIDA_ADDON_ABI_VERSION` *before*
//!    reading any other field; mismatch is a hard load error
//!    (`AddonAbiMismatch` family) with no fallback.
//! 5. The descriptor pointer must remain valid for the lifetime of the loaded
//!    addon library.
//!
//! Ownership freeze (RC1_DESIGN.md, Section D — Ownership freeze):
//!
//! - Host -> addon: borrowed read-only views.
//! - Addon -> host: addon-constructed owned values.
//! - The addon must not retain borrowed inputs past the call.
//! - Host owns release of returned values.

use core::ffi::{c_char, c_void};

/// The frozen ABI version for Taida addons.
///
/// Bumping this constant requires bumping the entry symbol name as well
/// (e.g. `taida_addon_get_v2`) and is considered a breaking change.
pub const TAIDA_ADDON_ABI_VERSION: u32 = 1;

/// Frozen entry symbol name. The Native loader resolves this exact symbol.
///
/// Kept as a `&str` constant so both the addon-side macro and the host-side
/// loader can reference the same name.
pub const TAIDA_ADDON_ENTRY_SYMBOL: &str = "taida_addon_get_v1";

/// Status code returned by addon entry points and host callbacks.
///
/// Wire format is `u32` (`#[repr(u32)]`), so it is stable across compilers
/// for the C ABI surface.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaidaAddonStatus {
    /// Operation succeeded.
    Ok = 0,
    /// Generic addon-side error. The optional `out_error` slot carries detail.
    Error = 1,
    /// The host or addon ABI version did not match `TAIDA_ADDON_ABI_VERSION`.
    AbiMismatch = 2,
    /// `init` was called twice or call ordering is invalid.
    InvalidState = 3,
    /// A value of an unsupported kind crossed the bridge.
    UnsupportedValue = 4,
    /// One or more pointer arguments were null.
    NullPointer = 5,
    /// `arity` mismatch between declared and actual call.
    ArityMismatch = 6,
}

/// A single function entry in an addon's function table.
///
/// All fields are `#[repr(C)]` for ABI stability. Strings are
/// nul-terminated and owned by the addon (typically `'static`).
#[repr(C)]
pub struct TaidaAddonFunctionV1 {
    /// Nul-terminated UTF-8 function name. Borrowed read-only by the host.
    pub name: *const c_char,
    /// Declared arity. The host enforces this before invoking `call`.
    pub arity: u32,
    /// The C ABI call entry point.
    ///
    /// `args_ptr` and `args_len` describe a borrowed read-only argument vector
    /// (host-owned). `out_value` and `out_error` are addon-allocated and
    /// transferred to host ownership on `Ok` / `Error` respectively. Either
    /// out slot may be left null when not used.
    pub call: extern "C" fn(
        args_ptr: *const TaidaAddonValueV1,
        args_len: u32,
        out_value: *mut *mut TaidaAddonValueV1,
        out_error: *mut *mut TaidaAddonErrorV1,
    ) -> TaidaAddonStatus,
}

// SAFETY: TaidaAddonFunctionV1 is a plain repr(C) descriptor that addon
// authors construct at compile time via `&'static [TaidaAddonFunctionV1]`.
// The raw pointers it carries are nul-terminated `'static` strings and
// `extern "C" fn` pointers, both of which are `Send + Sync` by their nature.
unsafe impl Send for TaidaAddonFunctionV1 {}
unsafe impl Sync for TaidaAddonFunctionV1 {}

/// The descriptor returned by `taida_addon_get_v1`.
///
/// Layout is frozen by RC1 design lock. New fields would require bumping
/// `TAIDA_ADDON_ABI_VERSION` and the entry symbol.
#[repr(C)]
pub struct TaidaAddonDescriptorV1 {
    /// Must equal `TAIDA_ADDON_ABI_VERSION`. The host validates this *first*.
    pub abi_version: u32,
    /// Padding so that following pointer-sized fields are naturally aligned
    /// on 64-bit targets without forcing `#[repr(C, packed)]`.
    pub _reserved: u32,
    /// Nul-terminated UTF-8 addon name (e.g. `"taida-lang/sample"`).
    pub addon_name: *const c_char,
    /// Number of entries in `functions`.
    pub function_count: u32,
    /// Padding to keep `functions` 8-byte aligned.
    pub _reserved2: u32,
    /// Pointer to the function table. Length is `function_count`.
    pub functions: *const TaidaAddonFunctionV1,
    /// Optional one-shot init callback. The host calls it exactly once
    /// after a successful ABI handshake and *before* any function call.
    pub init: Option<extern "C" fn(host: *const TaidaHostV1) -> TaidaAddonStatus>,
}

// SAFETY: see TaidaAddonFunctionV1. The descriptor is intended to live in the
// addon's `.rodata` and be safely shared across threads.
unsafe impl Send for TaidaAddonDescriptorV1 {}
unsafe impl Sync for TaidaAddonDescriptorV1 {}

/// Discriminant for [`TaidaAddonValueV1::tag`].
///
/// Wire format is `u32` and is **frozen in Phase 3** (`.dev/RC1_DESIGN.md`,
/// Phase 3 Lock — Value Bridge Concrete Layout). Do not renumber or
/// reorder these variants without bumping [`TAIDA_ADDON_ABI_VERSION`] and
/// the entry symbol name.
///
/// Tag values 8 and above are reserved for future value kinds. Anything
/// outside this set must be rejected at the boundary with
/// [`TaidaAddonStatus::UnsupportedValue`].
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaidaAddonValueTag {
    /// Default value. `payload` is ignored and typically null.
    Unit = 0,
    /// `payload` is `*mut TaidaAddonIntPayload`.
    Int = 1,
    /// `payload` is `*mut TaidaAddonFloatPayload`.
    Float = 2,
    /// `payload` is `*mut TaidaAddonBoolPayload`.
    Bool = 3,
    /// `payload` is `*mut TaidaAddonBytesPayload`; bytes must be valid UTF-8.
    Str = 4,
    /// `payload` is `*mut TaidaAddonBytesPayload`; arbitrary octets.
    Bytes = 5,
    /// `payload` is `*mut TaidaAddonListPayload`.
    List = 6,
    /// `payload` is `*mut TaidaAddonPackPayload`.
    Pack = 7,
}

impl TaidaAddonValueTag {
    /// Narrow a raw `u32` tag back to the strongly-typed enum. Returns
    /// `None` for unknown values (the host surfaces these as
    /// `TaidaAddonStatus::UnsupportedValue`).
    pub fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Unit),
            1 => Some(Self::Int),
            2 => Some(Self::Float),
            3 => Some(Self::Bool),
            4 => Some(Self::Str),
            5 => Some(Self::Bytes),
            6 => Some(Self::List),
            7 => Some(Self::Pack),
            _ => None,
        }
    }
}

/// Integer payload for [`TaidaAddonValueTag::Int`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TaidaAddonIntPayload {
    pub value: i64,
}

/// Floating-point payload for [`TaidaAddonValueTag::Float`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TaidaAddonFloatPayload {
    pub value: f64,
}

/// Boolean payload for [`TaidaAddonValueTag::Bool`]. `0` = false, `1` = true.
/// All other values must be rejected as invalid.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TaidaAddonBoolPayload {
    pub value: u8,
}

/// `Str` / `Bytes` payload for [`TaidaAddonValueTag::Str`] and
/// [`TaidaAddonValueTag::Bytes`].
///
/// - For `Str` the `ptr`/`len` slice is valid UTF-8.
/// - For `Bytes` the slice is arbitrary octets.
///
/// Ownership: the slice is owned by the host allocator (the host
/// allocates the struct *and* the bytes). Addons must not call `free`
/// on it directly; they release it by calling `host->value_release`
/// on the owning `TaidaAddonValueV1`.
#[repr(C)]
pub struct TaidaAddonBytesPayload {
    pub ptr: *const u8,
    pub len: usize,
}

// SAFETY: TaidaAddonBytesPayload is an immutable view; raw bytes behind
// the pointer are host-owned read-only memory for the call's lifetime.
unsafe impl Send for TaidaAddonBytesPayload {}
unsafe impl Sync for TaidaAddonBytesPayload {}

/// List payload for [`TaidaAddonValueTag::List`].
///
/// `items` points to a contiguous array of `*mut TaidaAddonValueV1`
/// (length `len`). Ownership of each child pointer belongs to this
/// payload; releasing the parent `TaidaAddonValueV1` recursively
/// releases the children.
#[repr(C)]
pub struct TaidaAddonListPayload {
    pub items: *const *mut TaidaAddonValueV1,
    pub len: usize,
}

// SAFETY: pointer array is host-owned and immutable for the call's lifetime.
unsafe impl Send for TaidaAddonListPayload {}
unsafe impl Sync for TaidaAddonListPayload {}

/// Single named entry in a [`TaidaAddonPackPayload`].
///
/// `name` is a nul-terminated UTF-8 C string owned by the host
/// allocator; `value` is a `*mut TaidaAddonValueV1` owned by the parent
/// pack payload.
#[repr(C)]
pub struct TaidaAddonPackEntryV1 {
    pub name: *const c_char,
    pub value: *mut TaidaAddonValueV1,
}

// SAFETY: Both pointers are host-owned and read-only for the call.
unsafe impl Send for TaidaAddonPackEntryV1 {}
unsafe impl Sync for TaidaAddonPackEntryV1 {}

/// BuchiPack payload for [`TaidaAddonValueTag::Pack`].
#[repr(C)]
pub struct TaidaAddonPackPayload {
    pub entries: *const TaidaAddonPackEntryV1,
    pub len: usize,
}

// SAFETY: entries array is host-owned and immutable for the call's lifetime.
unsafe impl Send for TaidaAddonPackPayload {}
unsafe impl Sync for TaidaAddonPackPayload {}

/// Host capability table v1.
///
/// Per `.dev/RC1_DESIGN.md` Section G and Phase 3 Lock, this is the
/// minimal surface an addon needs to build output values and errors that
/// cross the bridge. **All bridge values are allocated by the host** —
/// addons must not `malloc` / `Box::leak` values directly. This is how
/// RC1B-103 is resolved: allocator unification means one free-er (the
/// host) regardless of which side constructed the value.
///
/// Phase 1 reserved 5 `*const c_void` slots here and documented them as
/// "signature TBD in Phase 3". Phase 3 (this file) fills them with the
/// concrete signatures listed below. The ABI version stays at `1`
/// because the `taida-addon` crate and the host loader are updated in
/// lockstep inside the same workspace.
///
/// Intentionally omitted (not in RC1 scope):
///
/// - no logging hook
/// - no async scheduler hook
/// - no arbitrary user-defined allocator escape hatch
#[repr(C)]
#[derive(Debug)]
pub struct TaidaHostV1 {
    /// Must equal `TAIDA_ADDON_ABI_VERSION`. Set by the host before calling
    /// `init`. Addons must reject mismatched values.
    pub abi_version: u32,
    /// Reserved for future extension. Set to 0 by the host.
    pub _reserved: u32,

    /// Construct a `Unit` value. Returns a host-owned `*mut TaidaAddonValueV1`.
    pub value_new_unit: extern "C" fn(host: *const TaidaHostV1) -> *mut TaidaAddonValueV1,
    /// Construct an `Int` value.
    pub value_new_int: extern "C" fn(host: *const TaidaHostV1, v: i64) -> *mut TaidaAddonValueV1,
    /// Construct a `Float` value.
    pub value_new_float: extern "C" fn(host: *const TaidaHostV1, v: f64) -> *mut TaidaAddonValueV1,
    /// Construct a `Bool` value. `v` must be `0` or `1`.
    pub value_new_bool: extern "C" fn(host: *const TaidaHostV1, v: u8) -> *mut TaidaAddonValueV1,
    /// Construct a `Str` value. The host copies `len` bytes starting at
    /// `bytes`; the input must be valid UTF-8.
    pub value_new_str: extern "C" fn(
        host: *const TaidaHostV1,
        bytes: *const u8,
        len: usize,
    ) -> *mut TaidaAddonValueV1,
    /// Construct a `Bytes` value. The host copies `len` bytes.
    pub value_new_bytes: extern "C" fn(
        host: *const TaidaHostV1,
        bytes: *const u8,
        len: usize,
    ) -> *mut TaidaAddonValueV1,
    /// Construct a `List` value. `items` is an array of `len` already-built
    /// `*mut TaidaAddonValueV1` pointers. **Ownership of each child moves
    /// into the newly built list payload**; the addon must not release
    /// the children afterwards.
    pub value_new_list: extern "C" fn(
        host: *const TaidaHostV1,
        items: *const *mut TaidaAddonValueV1,
        len: usize,
    ) -> *mut TaidaAddonValueV1,
    /// Construct a `Pack` value. `names` / `values` are parallel arrays of
    /// length `len`. Name strings must be UTF-8 nul-terminated; the host
    /// copies them. Ownership of each value moves into the pack payload.
    pub value_new_pack: extern "C" fn(
        host: *const TaidaHostV1,
        names: *const *const c_char,
        values: *const *mut TaidaAddonValueV1,
        len: usize,
    ) -> *mut TaidaAddonValueV1,
    /// Release a host-owned value (recursive for lists / packs). Null is
    /// a no-op. Double-release is undefined and must not happen (the host
    /// tracks this via `AddonCallError::DoubleRelease` in debug builds).
    pub value_release: extern "C" fn(host: *const TaidaHostV1, value: *mut TaidaAddonValueV1),

    /// Construct an error value. `msg` must be valid UTF-8 of length `msg_len`.
    /// The host copies the bytes.
    pub error_new: extern "C" fn(
        host: *const TaidaHostV1,
        code: u32,
        msg_ptr: *const u8,
        msg_len: usize,
    ) -> *mut TaidaAddonErrorV1,
    /// Release a host-owned error. Null is a no-op.
    pub error_release: extern "C" fn(host: *const TaidaHostV1, error: *mut TaidaAddonErrorV1),
}

// SAFETY: the host fills this table before calling `init`; thereafter the
// pointers are immutable for the addon's lifetime.
unsafe impl Send for TaidaHostV1 {}
unsafe impl Sync for TaidaHostV1 {}

/// A value crossing the addon boundary.
///
/// Layout is **frozen in Phase 3**. The header is `(tag: u32, _reserved:
/// u32, payload: *mut c_void)` — identical byte shape to the Phase 1
/// reservation, so Phase 1 addons that treated this as an opaque header
/// keep working.
///
/// Ownership: every `TaidaAddonValueV1` that crosses the bridge is
/// host-allocated and host-freed. Addons obtain them via
/// `TaidaHostV1::value_new_*` callbacks and never call `free` on them
/// directly. See `.dev/RC1_DESIGN.md` Phase 3 Lock — Ownership contract.
#[repr(C)]
pub struct TaidaAddonValueV1 {
    /// Tag identifying the value kind. See [`TaidaAddonValueTag`] for
    /// the frozen discriminant table. Raw `u32` for ABI stability.
    pub tag: u32,
    /// Reserved for future per-value flags. Set to 0 by the host.
    pub _reserved: u32,
    /// Type-erased payload pointer. Layout depends on [`tag`](#structfield.tag).
    pub payload: *mut c_void,
}

/// An error value crossing the addon boundary.
///
/// Phase 3 locks the shape to `(code: u32, _reserved: u32, message:
/// *const c_char)`. `message` is a nul-terminated UTF-8 C string owned
/// by the host allocator.
#[repr(C)]
pub struct TaidaAddonErrorV1 {
    pub code: u32,
    pub _reserved: u32,
    /// Nul-terminated UTF-8 message owned by the host allocator.
    /// Released via `TaidaHostV1::error_release`.
    pub message: *const c_char,
}

// SAFETY: see TaidaAddonValueV1. The pointer and C-string are host-owned
// and immutable for the call's lifetime.
unsafe impl Send for TaidaAddonErrorV1 {}
unsafe impl Sync for TaidaAddonErrorV1 {}

/// Type alias for the entry symbol exported by every addon.
///
/// Addon authors should not construct this directly — use the
/// [`crate::declare_addon!`] macro, which generates a properly named
/// `extern "C"` function returning a `&'static TaidaAddonDescriptorV1`.
pub type TaidaAddonGetV1 = unsafe extern "C" fn() -> *const TaidaAddonDescriptorV1;
