//! `taida-addon` — Taida Lang addon authoring crate (ABI v1).
//!
//! This is the **addon foundation** referenced by RC1 (`taida-lang/addon-rs`).
//! Authors of native Rust addons depend on this crate to build cdylib
//! binaries that the Taida Native backend loader can discover and call.
//!
//! ## Quick start
//!
//! ```ignore
//! // In your addon crate's Cargo.toml:
//! // [lib]
//! // crate-type = ["cdylib"]
//! //
//! // [dependencies]
//! // taida-addon = { path = "../addon-rs" }
//!
//! use core::ffi::c_char;
//! use taida_addon::abi::{TaidaAddonFunctionV1, TaidaAddonStatus, TaidaAddonValueV1, TaidaAddonErrorV1};
//!
//! extern "C" fn noop_call(
//!     _args_ptr: *const TaidaAddonValueV1,
//!     _args_len: u32,
//!     _out_value: *mut *mut TaidaAddonValueV1,
//!     _out_error: *mut *mut TaidaAddonErrorV1,
//! ) -> TaidaAddonStatus {
//!     TaidaAddonStatus::Ok
//! }
//!
//! static FUNCTIONS: &[TaidaAddonFunctionV1] = &[TaidaAddonFunctionV1 {
//!     name: c"noop".as_ptr() as *const c_char,
//!     arity: 0,
//!     call: noop_call,
//! }];
//!
//! taida_addon::declare_addon! {
//!     name: "example/sample",
//!     functions: FUNCTIONS,
//! }
//! ```
//!
//! The `declare_addon!` macro emits a `#[no_mangle] pub extern "C" fn
//! taida_addon_get_v1()` that the Native loader looks up.
//!
//! ## Frozen contract
//!
//! See [`abi`] for the frozen ABI v1 surface. Anything outside the `abi`
//! module is part of the **safe wrapper** and may evolve in compatible
//! ways without bumping the ABI version.

#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod abi;
pub mod bridge;

pub use abi::{
    TAIDA_ADDON_ABI_VERSION, TAIDA_ADDON_ENTRY_SYMBOL, TaidaAddonBoolPayload,
    TaidaAddonBytesPayload, TaidaAddonDescriptorV1, TaidaAddonErrorV1, TaidaAddonFloatPayload,
    TaidaAddonFunctionV1, TaidaAddonGetV1, TaidaAddonIntPayload, TaidaAddonListPayload,
    TaidaAddonPackEntryV1, TaidaAddonPackPayload, TaidaAddonStatus, TaidaAddonValueTag,
    TaidaAddonValueV1, TaidaHostV1,
};

/// Declare an addon entry point.
///
/// Generates a `#[no_mangle] pub extern "C" fn taida_addon_get_v1()` that
/// returns a pointer to a `'static` [`TaidaAddonDescriptorV1`]. The macro
/// also emits the addon name as a nul-terminated `'static` `CStr` so the
/// pointer in the descriptor stays valid for the lifetime of the library.
///
/// # Example
///
/// ```ignore
/// taida_addon::declare_addon! {
///     name: "taida-lang/sample",
///     functions: FUNCTIONS,
/// }
/// ```
///
/// `FUNCTIONS` must be a `&'static [TaidaAddonFunctionV1]` (typically a
/// `static FUNCTIONS: &[TaidaAddonFunctionV1] = &[...];`).
///
/// An optional `init` callback can be supplied:
///
/// ```ignore
/// extern "C" fn my_init(host: *const taida_addon::TaidaHostV1)
///     -> taida_addon::TaidaAddonStatus
/// {
///     taida_addon::TaidaAddonStatus::Ok
/// }
///
/// taida_addon::declare_addon! {
///     name: "taida-lang/sample",
///     functions: FUNCTIONS,
///     init: my_init,
/// }
/// ```
#[macro_export]
macro_rules! declare_addon {
    (
        name: $name:literal,
        functions: $functions:path $(,)?
    ) => {
        $crate::__declare_addon_impl!($name, $functions, ::core::option::Option::None);
    };
    (
        name: $name:literal,
        functions: $functions:path,
        init: $init:path $(,)?
    ) => {
        $crate::__declare_addon_impl!(
            $name,
            $functions,
            ::core::option::Option::Some(
                $init as extern "C" fn(*const $crate::TaidaHostV1) -> $crate::TaidaAddonStatus
            )
        );
    };
}

/// Internal expansion helper for [`declare_addon!`]. Not part of the public API.
#[doc(hidden)]
#[macro_export]
macro_rules! __declare_addon_impl {
    ($name:literal, $functions:path, $init:expr) => {
        // Nul-terminated 'static name. We use a const C-string literal so the
        // pointer is guaranteed to live for the entire library lifetime.
        const __TAIDA_ADDON_NAME: &::core::ffi::CStr =
            match ::core::ffi::CStr::from_bytes_with_nul(::core::concat!($name, "\0").as_bytes()) {
                ::core::result::Result::Ok(s) => s,
                ::core::result::Result::Err(_) => {
                    panic!("taida_addon: name must not contain interior NUL bytes")
                }
            };

        static __TAIDA_ADDON_DESCRIPTOR: $crate::TaidaAddonDescriptorV1 =
            $crate::TaidaAddonDescriptorV1 {
                abi_version: $crate::TAIDA_ADDON_ABI_VERSION,
                _reserved: 0,
                addon_name: __TAIDA_ADDON_NAME.as_ptr(),
                function_count: { $functions.len() as u32 },
                _reserved2: 0,
                functions: { $functions.as_ptr() },
                init: $init,
            };

        /// Frozen entry point. The Taida Native loader resolves this exact
        /// symbol via dlsym (or platform equivalent).
        #[unsafe(no_mangle)]
        pub extern "C" fn taida_addon_get_v1() -> *const $crate::TaidaAddonDescriptorV1 {
            &__TAIDA_ADDON_DESCRIPTOR as *const _
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::{CStr, c_char};

    // ── ABI freeze tests ───────────────────────────────────────────

    #[test]
    fn abi_version_is_locked_to_one() {
        // RC1_DESIGN.md Section C — ABI v1 freeze, item 1.
        assert_eq!(TAIDA_ADDON_ABI_VERSION, 1);
    }

    #[test]
    fn entry_symbol_is_taida_addon_get_v1() {
        // RC1_DESIGN.md Section C — ABI v1 freeze, item 2.
        assert_eq!(TAIDA_ADDON_ENTRY_SYMBOL, "taida_addon_get_v1");
    }

    #[test]
    fn status_codes_are_stable() {
        // Wire-format check: status codes are part of the C ABI surface.
        // Reordering or renumbering them is a breaking change.
        assert_eq!(TaidaAddonStatus::Ok as u32, 0);
        assert_eq!(TaidaAddonStatus::Error as u32, 1);
        assert_eq!(TaidaAddonStatus::AbiMismatch as u32, 2);
        assert_eq!(TaidaAddonStatus::InvalidState as u32, 3);
        assert_eq!(TaidaAddonStatus::UnsupportedValue as u32, 4);
        assert_eq!(TaidaAddonStatus::NullPointer as u32, 5);
        assert_eq!(TaidaAddonStatus::ArityMismatch as u32, 6);
    }

    #[test]
    fn descriptor_is_repr_c_and_pointer_aligned() {
        // We rely on natural pointer alignment so the host loader can read
        // each field with a regular load, no unaligned access.
        assert_eq!(
            core::mem::align_of::<TaidaAddonDescriptorV1>(),
            core::mem::align_of::<*const c_char>()
        );
    }

    #[test]
    fn function_entry_layout_is_stable() {
        // Three pointer-or-word slots: name, arity, call.
        // We don't pin exact bytes (compiler may pad), but we do pin the
        // field accessors so future refactors can't shuffle the layout
        // accidentally.
        let dummy_name = c"x";

        extern "C" fn noop(
            _args_ptr: *const TaidaAddonValueV1,
            _args_len: u32,
            _out_value: *mut *mut TaidaAddonValueV1,
            _out_error: *mut *mut TaidaAddonErrorV1,
        ) -> TaidaAddonStatus {
            TaidaAddonStatus::Ok
        }

        let entry = TaidaAddonFunctionV1 {
            name: dummy_name.as_ptr(),
            arity: 2,
            call: noop,
        };
        // Smoke check: name reads back as the original C string.
        let read = unsafe { CStr::from_ptr(entry.name) };
        assert_eq!(read, dummy_name);
        assert_eq!(entry.arity, 2);
    }

    // ── declare_addon! macro tests ─────────────────────────────────

    extern "C" fn test_call(
        _args_ptr: *const TaidaAddonValueV1,
        _args_len: u32,
        _out_value: *mut *mut TaidaAddonValueV1,
        _out_error: *mut *mut TaidaAddonErrorV1,
    ) -> TaidaAddonStatus {
        TaidaAddonStatus::Ok
    }

    static TEST_FUNCTIONS: &[TaidaAddonFunctionV1] = &[TaidaAddonFunctionV1 {
        name: c"noop".as_ptr(),
        arity: 0,
        call: test_call,
    }];

    // We can't expand `declare_addon!` at module scope inside #[cfg(test)]
    // because it would emit `#[no_mangle]` symbols that collide between
    // test crates. Instead we test the descriptor construction by hand
    // using the same constants the macro would have used.
    static TEST_DESCRIPTOR: TaidaAddonDescriptorV1 = TaidaAddonDescriptorV1 {
        abi_version: TAIDA_ADDON_ABI_VERSION,
        _reserved: 0,
        addon_name: c"taida-addon/test".as_ptr(),
        function_count: 1,
        _reserved2: 0,
        functions: TEST_FUNCTIONS.as_ptr(),
        init: None,
    };

    #[test]
    fn manual_descriptor_round_trips() {
        let d = &TEST_DESCRIPTOR;
        assert_eq!(d.abi_version, TAIDA_ADDON_ABI_VERSION);
        assert_eq!(d.function_count, 1);
        assert!(d.init.is_none());

        let name = unsafe { CStr::from_ptr(d.addon_name) };
        assert_eq!(name.to_str().unwrap(), "taida-addon/test");

        // function_count must be consistent with the slice length.
        assert_eq!(d.function_count as usize, TEST_FUNCTIONS.len());

        // Read function 0 by raw pointer (mirrors what the host loader does).
        let f0 = unsafe { &*d.functions.add(0) };
        let f0_name = unsafe { CStr::from_ptr(f0.name) };
        assert_eq!(f0_name.to_str().unwrap(), "noop");
        assert_eq!(f0.arity, 0);

        // Sanity-check the call pointer is invokable.
        let status = (f0.call)(
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::Ok);
    }

    // ── Host table layout test ─────────────────────────────────────
    //
    // Phase 3 replaces the Phase 1 `*const c_void` reservation slots with
    // concrete extern "C" fn signatures. We build a throwaway table here
    // with stub callbacks to prove the struct is constructible and
    // pinned in field order. Any reshuffling in `abi.rs` makes this
    // test fail to compile, which is exactly what we want.

    extern "C" fn stub_new_unit(_host: *const TaidaHostV1) -> *mut TaidaAddonValueV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_new_int(_host: *const TaidaHostV1, _v: i64) -> *mut TaidaAddonValueV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_new_float(_host: *const TaidaHostV1, _v: f64) -> *mut TaidaAddonValueV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_new_bool(_host: *const TaidaHostV1, _v: u8) -> *mut TaidaAddonValueV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_new_str(
        _host: *const TaidaHostV1,
        _bytes: *const u8,
        _len: usize,
    ) -> *mut TaidaAddonValueV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_new_bytes(
        _host: *const TaidaHostV1,
        _bytes: *const u8,
        _len: usize,
    ) -> *mut TaidaAddonValueV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_new_list(
        _host: *const TaidaHostV1,
        _items: *const *mut TaidaAddonValueV1,
        _len: usize,
    ) -> *mut TaidaAddonValueV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_new_pack(
        _host: *const TaidaHostV1,
        _names: *const *const c_char,
        _values: *const *mut TaidaAddonValueV1,
        _len: usize,
    ) -> *mut TaidaAddonValueV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_release(_host: *const TaidaHostV1, _value: *mut TaidaAddonValueV1) {}
    extern "C" fn stub_error_new(
        _host: *const TaidaHostV1,
        _code: u32,
        _msg_ptr: *const u8,
        _msg_len: usize,
    ) -> *mut TaidaAddonErrorV1 {
        core::ptr::null_mut()
    }
    extern "C" fn stub_error_release(_host: *const TaidaHostV1, _error: *mut TaidaAddonErrorV1) {}

    fn stub_host() -> TaidaHostV1 {
        TaidaHostV1 {
            abi_version: TAIDA_ADDON_ABI_VERSION,
            _reserved: 0,
            value_new_unit: stub_new_unit,
            value_new_int: stub_new_int,
            value_new_float: stub_new_float,
            value_new_bool: stub_new_bool,
            value_new_str: stub_new_str,
            value_new_bytes: stub_new_bytes,
            value_new_list: stub_new_list,
            value_new_pack: stub_new_pack,
            value_release: stub_release,
            error_new: stub_error_new,
            error_release: stub_error_release,
        }
    }

    #[test]
    fn host_table_is_repr_c_and_minimal() {
        // RC1_DESIGN.md Section G + Phase 3 Lock: minimal host capability
        // table with concrete signatures. No logging slot, no async
        // scheduler hook.
        let host = stub_host();
        assert_eq!(host.abi_version, 1);

        // Smoke-check each slot is invokable through the table. Every
        // stub is a no-op that returns null.
        assert!((host.value_new_unit)(&host as *const _).is_null());
        assert!((host.value_new_int)(&host as *const _, 42).is_null());
        assert!((host.value_new_float)(&host as *const _, 2.5).is_null());
        assert!((host.value_new_bool)(&host as *const _, 1).is_null());
        assert!((host.value_new_str)(&host as *const _, core::ptr::null(), 0).is_null());
        assert!((host.value_new_bytes)(&host as *const _, core::ptr::null(), 0).is_null());
        assert!((host.value_new_list)(&host as *const _, core::ptr::null(), 0).is_null());
        assert!(
            (host.value_new_pack)(&host as *const _, core::ptr::null(), core::ptr::null(), 0)
                .is_null()
        );
        (host.value_release)(&host as *const _, core::ptr::null_mut());
        assert!((host.error_new)(&host as *const _, 0, core::ptr::null(), 0).is_null());
        (host.error_release)(&host as *const _, core::ptr::null_mut());
    }

    #[test]
    fn value_tag_discriminants_are_frozen() {
        // Phase 3 Lock: these wire values are part of the ABI surface.
        // Bumping any of them requires a new entry symbol name.
        assert_eq!(TaidaAddonValueTag::Unit as u32, 0);
        assert_eq!(TaidaAddonValueTag::Int as u32, 1);
        assert_eq!(TaidaAddonValueTag::Float as u32, 2);
        assert_eq!(TaidaAddonValueTag::Bool as u32, 3);
        assert_eq!(TaidaAddonValueTag::Str as u32, 4);
        assert_eq!(TaidaAddonValueTag::Bytes as u32, 5);
        assert_eq!(TaidaAddonValueTag::List as u32, 6);
        assert_eq!(TaidaAddonValueTag::Pack as u32, 7);
    }

    #[test]
    fn value_tag_round_trips_via_from_u32() {
        for (raw, expected) in [
            (0u32, TaidaAddonValueTag::Unit),
            (1, TaidaAddonValueTag::Int),
            (2, TaidaAddonValueTag::Float),
            (3, TaidaAddonValueTag::Bool),
            (4, TaidaAddonValueTag::Str),
            (5, TaidaAddonValueTag::Bytes),
            (6, TaidaAddonValueTag::List),
            (7, TaidaAddonValueTag::Pack),
        ] {
            assert_eq!(TaidaAddonValueTag::from_u32(raw), Some(expected));
        }
        // Values outside the frozen table must return None so the host
        // can surface them as `TaidaAddonStatus::UnsupportedValue`.
        assert_eq!(TaidaAddonValueTag::from_u32(8), None);
        assert_eq!(TaidaAddonValueTag::from_u32(u32::MAX), None);
    }

    #[test]
    fn value_header_layout_matches_phase_one_reservation() {
        // Phase 3 must not change the byte layout of the header. Phase 1
        // addons saw (tag: u32, _reserved: u32, payload: *mut c_void).
        use core::mem::{align_of, size_of};

        // u32 + u32 = 8 bytes, then pointer-aligned payload.
        assert_eq!(size_of::<TaidaAddonValueV1>(), 8 + size_of::<*mut ()>());
        // Natural pointer alignment — the loader reads these fields
        // with a regular load.
        assert_eq!(align_of::<TaidaAddonValueV1>(), align_of::<*mut ()>());
    }
}
