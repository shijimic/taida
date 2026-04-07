//! `taida-addon-sample` — minimal sample addon for RC1.
//!
//! This is the **smallest possible addon** that exercises the frozen
//! ABI v1 contract. It exists for two reasons:
//!
//! 1. To prove that `taida-addon`'s `declare_addon!` macro produces a
//!    well-formed `taida_addon_get_v1` symbol.
//! 2. To give RC1 Phase 2 / Phase 3 (Native loader + value bridge) a
//!    real `cdylib` to dlopen.
//!
//! Surface (deliberately tiny):
//!
//! - `noop`: arity 0, returns `Ok` and does nothing.
//! - `echo`: arity 1, round-trips its single argument through the
//!   Phase 3 value bridge: the addon reads the borrowed input via
//!   [`taida_addon::bridge`], rebuilds an equivalent host-owned value
//!   via the host capability table, and writes it into `*out_value`.
//!
//! The crate intentionally avoids depending on anything beyond
//! `taida-addon` and `core::ffi`.
//!
//! ## Init callback
//!
//! The `echo` implementation needs the `TaidaHostV1` callback table to
//! build return values, but the C ABI call signature doesn't pass the
//! host pointer per-call (RC1 locked the Phase 1 shape and we don't
//! want to reshuffle it for Phase 3). Instead, the addon captures the
//! host pointer in a `static AtomicPtr` during its `init` callback.
//! RC1 guarantees `init` is called exactly once before any function
//! call (`.dev/RC1_DESIGN.md` Section C).

use core::ffi::c_char;
use core::sync::atomic::{AtomicPtr, Ordering};

use taida_addon::bridge::{BorrowedValue, HostValueBuilder};
use taida_addon::{
    TaidaAddonErrorV1, TaidaAddonFunctionV1, TaidaAddonStatus, TaidaAddonValueTag,
    TaidaAddonValueV1, TaidaHostV1,
};

/// Captured host callback table. Populated by [`sample_init`] and read
/// by per-call entry points. RC1 Phase 3 ownership contract: the host
/// table outlives the addon, so storing a raw pointer here is sound as
/// long as the loader keeps the library loaded while calls are in
/// flight (which it always does — calls happen through a
/// `LoadedAddon` borrow).
static HOST_PTR: AtomicPtr<TaidaHostV1> = AtomicPtr::new(core::ptr::null_mut());

/// One-shot init callback. The host calls this exactly once after a
/// successful ABI handshake. We capture the `TaidaHostV1` pointer so
/// per-call entry points can build host-owned return values.
extern "C" fn sample_init(host: *const TaidaHostV1) -> TaidaAddonStatus {
    if host.is_null() {
        // Defensive: a null host table would make `echo` unable to
        // build return values. Fail fast.
        return TaidaAddonStatus::NullPointer;
    }
    // SAFETY: we only store the pointer; we read it back with matching
    // `unsafe` at call time. The host promises the table is valid for
    // the addon's lifetime (see RC1 design lock).
    HOST_PTR.store(host as *mut _, Ordering::Release);
    TaidaAddonStatus::Ok
}

extern "C" fn noop(
    _args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    _out_value: *mut *mut TaidaAddonValueV1,
    _out_error: *mut *mut TaidaAddonErrorV1,
) -> TaidaAddonStatus {
    if args_len != 0 {
        return TaidaAddonStatus::ArityMismatch;
    }
    TaidaAddonStatus::Ok
}

/// Rebuild a host-owned copy of `src` using the provided builder. This
/// is a deliberately simple identity mapping — it's the minimum
/// round-trip we need to prove the Phase 3 value bridge works end to
/// end.
///
/// Returns `None` when the input value has an unsupported kind (caller
/// should surface `TaidaAddonStatus::UnsupportedValue`).
fn rebuild(
    builder: &HostValueBuilder<'_>,
    src: BorrowedValue<'_>,
) -> Option<*mut TaidaAddonValueV1> {
    match src.tag()? {
        TaidaAddonValueTag::Unit => Some(builder.unit()),
        TaidaAddonValueTag::Int => Some(builder.int(src.as_int()?)),
        TaidaAddonValueTag::Float => Some(builder.float(src.as_float()?)),
        TaidaAddonValueTag::Bool => Some(builder.bool(src.as_bool()?)),
        TaidaAddonValueTag::Str => Some(builder.str(src.as_str()?)),
        TaidaAddonValueTag::Bytes => Some(builder.bytes(src.as_bytes()?)),
        TaidaAddonValueTag::List => {
            let list = src.as_list()?;
            // Recursively rebuild every child. On any failure, release
            // the partial build so we don't leak host-owned values.
            let mut built: Vec<*mut TaidaAddonValueV1> = Vec::with_capacity(list.len());
            for child in list.iter() {
                match rebuild(builder, child) {
                    Some(ptr) => built.push(ptr),
                    None => {
                        // Roll back: release every host-owned value we
                        // already constructed. Ownership rollback is the
                        // only place an addon calls `release` directly.
                        for partial in built {
                            // SAFETY: we just built these via the host
                            // callbacks above.
                            unsafe { builder.release(partial) };
                        }
                        return None;
                    }
                }
            }
            Some(builder.list(&built))
        }
        TaidaAddonValueTag::Pack => {
            let pack = src.as_pack()?;
            // We need nul-terminated C strings for the host's pack
            // constructor. Build owned CStrings so they live until the
            // host has copied the bytes in `value_new_pack`.
            let mut owned_names: Vec<std::ffi::CString> = Vec::with_capacity(pack.len());
            let mut name_ptrs: Vec<*const c_char> = Vec::with_capacity(pack.len());
            let mut built_values: Vec<*mut TaidaAddonValueV1> = Vec::with_capacity(pack.len());
            for (name, value) in pack.iter() {
                match std::ffi::CString::new(name) {
                    Ok(c) => {
                        name_ptrs.push(c.as_ptr());
                        owned_names.push(c);
                    }
                    Err(_) => {
                        // Name had an interior nul; roll back.
                        for partial in built_values {
                            // SAFETY: just built above.
                            unsafe { builder.release(partial) };
                        }
                        return None;
                    }
                }
                match rebuild(builder, value) {
                    Some(ptr) => built_values.push(ptr),
                    None => {
                        for partial in built_values {
                            // SAFETY: just built above.
                            unsafe { builder.release(partial) };
                        }
                        return None;
                    }
                }
            }
            Some(builder.pack(&name_ptrs, &built_values))
        }
    }
}

extern "C" fn echo(
    args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    out_value: *mut *mut TaidaAddonValueV1,
    _out_error: *mut *mut TaidaAddonErrorV1,
) -> TaidaAddonStatus {
    if args_len != 1 {
        return TaidaAddonStatus::ArityMismatch;
    }
    if args_ptr.is_null() {
        return TaidaAddonStatus::NullPointer;
    }
    if out_value.is_null() {
        return TaidaAddonStatus::NullPointer;
    }

    // Pull the host table captured during init. RC1 guarantees init
    // runs first, so this should be non-null by the time any function
    // call reaches us.
    let host_raw = HOST_PTR.load(Ordering::Acquire);
    if host_raw.is_null() {
        return TaidaAddonStatus::InvalidState;
    }
    // SAFETY: host_raw came from the host's own callback table, which
    // outlives the addon. See the module-level comment.
    let builder = match unsafe { HostValueBuilder::from_raw(host_raw) } {
        Some(b) => b,
        None => return TaidaAddonStatus::NullPointer,
    };

    // SAFETY: host contract — args_ptr points to `args_len` valid
    // TaidaAddonValueV1 entries for the duration of this call.
    let arg = match unsafe { taida_addon::bridge::borrow_arg(args_ptr, args_len, 0) } {
        Some(v) => v,
        None => return TaidaAddonStatus::NullPointer,
    };

    match rebuild(&builder, arg) {
        Some(built) => {
            // SAFETY: out_value was null-checked above; the host passes
            // a valid `*mut *mut TaidaAddonValueV1` slot per the ABI
            // call signature.
            unsafe { *out_value = built };
            TaidaAddonStatus::Ok
        }
        None => TaidaAddonStatus::UnsupportedValue,
    }
}

/// Function table for the sample addon.
///
/// `pub` so the workspace's rlib consumers (loader integration tests)
/// can introspect the same table the cdylib exposes.
pub static SAMPLE_FUNCTIONS: &[TaidaAddonFunctionV1] = &[
    TaidaAddonFunctionV1 {
        name: c"noop".as_ptr() as *const c_char,
        arity: 0,
        call: noop,
    },
    TaidaAddonFunctionV1 {
        name: c"echo".as_ptr() as *const c_char,
        arity: 1,
        call: echo,
    },
];

// Wires up `taida_addon_get_v1` for the cdylib build.
//
// The macro emits a `#[no_mangle] pub extern "C" fn taida_addon_get_v1`
// at the crate root. The Native loader (RC1 Phase 2) resolves this
// exact symbol from the produced shared object.
taida_addon::declare_addon! {
    name: "taida-lang/addon-rs-sample",
    functions: SAMPLE_FUNCTIONS,
    init: sample_init,
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::ffi::CStr;
    use taida_addon::{TAIDA_ADDON_ABI_VERSION, TaidaAddonDescriptorV1};

    // Pull the symbol generated by `declare_addon!` back through Rust
    // so we can exercise it without a real dlopen.
    unsafe extern "C" {
        fn taida_addon_get_v1() -> *const TaidaAddonDescriptorV1;
    }

    #[test]
    fn entry_symbol_returns_a_descriptor() {
        // SAFETY: declare_addon! emitted this symbol in this same crate.
        let ptr = unsafe { taida_addon_get_v1() };
        assert!(!ptr.is_null());
        let d = unsafe { &*ptr };

        // ABI handshake — the host loader does this exact check first.
        assert_eq!(d.abi_version, TAIDA_ADDON_ABI_VERSION);
    }

    #[test]
    fn descriptor_advertises_two_functions() {
        let ptr = unsafe { taida_addon_get_v1() };
        let d = unsafe { &*ptr };
        assert_eq!(d.function_count as usize, SAMPLE_FUNCTIONS.len());
        assert_eq!(d.function_count, 2);
    }

    #[test]
    fn descriptor_addon_name_is_namespaced() {
        let ptr = unsafe { taida_addon_get_v1() };
        let d = unsafe { &*ptr };
        let name = unsafe { CStr::from_ptr(d.addon_name) };
        assert_eq!(name.to_str().unwrap(), "taida-lang/addon-rs-sample");
    }

    #[test]
    fn descriptor_init_is_wired_up() {
        // Phase 3: the sample addon now has a real `init` callback
        // (Phase 1 had `init: None`). This pins the wire-up so future
        // refactors can't silently drop it.
        let ptr = unsafe { taida_addon_get_v1() };
        let d = unsafe { &*ptr };
        assert!(d.init.is_some());
    }

    #[test]
    fn function_table_names_match_static() {
        let ptr = unsafe { taida_addon_get_v1() };
        let d = unsafe { &*ptr };

        // Read every function entry by raw pointer arithmetic — that's
        // exactly what the host loader will do.
        let mut seen = Vec::new();
        for i in 0..d.function_count as isize {
            let f = unsafe { &*d.functions.offset(i) };
            let name = unsafe { CStr::from_ptr(f.name) }.to_str().unwrap();
            seen.push((name.to_string(), f.arity));
        }
        assert_eq!(
            seen,
            vec![("noop".to_string(), 0u32), ("echo".to_string(), 1u32)]
        );
    }

    #[test]
    fn noop_call_runs_through_ok() {
        let f = &SAMPLE_FUNCTIONS[0];
        let status = (f.call)(
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::Ok);
    }

    #[test]
    fn noop_with_extra_args_is_arity_mismatch() {
        let f = &SAMPLE_FUNCTIONS[0];
        // Even though we don't construct a real arg vector, the entry
        // point must reject non-zero arity *before* dereferencing.
        let status = (f.call)(
            core::ptr::null(),
            3,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::ArityMismatch);
    }

    #[test]
    fn echo_with_zero_args_is_arity_mismatch() {
        let f = &SAMPLE_FUNCTIONS[1];
        let status = (f.call)(
            core::ptr::null(),
            0,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::ArityMismatch);
    }

    #[test]
    fn echo_with_null_args_ptr_is_null_pointer() {
        let f = &SAMPLE_FUNCTIONS[1];
        let status = (f.call)(
            core::ptr::null(),
            1,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::NullPointer);
    }

    #[test]
    fn echo_without_out_value_slot_is_null_pointer() {
        // `out_value` is how the addon hands back the round-tripped
        // value; passing null for it when we *did* send an input
        // vector must be rejected before the addon tries to write.
        use taida_addon::TaidaAddonIntPayload;
        let p = TaidaAddonIntPayload { value: 1 };
        let v = TaidaAddonValueV1 {
            tag: TaidaAddonValueTag::Int as u32,
            _reserved: 0,
            payload: &p as *const _ as *mut _,
        };
        let f = &SAMPLE_FUNCTIONS[1];
        let status = (f.call)(
            &v as *const _,
            1,
            core::ptr::null_mut(), // out_value intentionally null
            core::ptr::null_mut(),
        );
        assert_eq!(status, TaidaAddonStatus::NullPointer);
    }
}
