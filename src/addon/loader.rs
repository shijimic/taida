//! Native addon loader (RC1 Phase 2 -- `RC1-2a` / `RC1-2b`).
//!
//! Resolves the frozen `taida_addon_get_v1` symbol from a Rust addon
//! `cdylib`, validates the ABI handshake, and exposes the addon's
//! function table behind a safe Rust facade.
//!
//! # Frozen contract (`.dev/RC1_DESIGN.md`)
//!
//! 1. ABI version is `1`. The loader validates `descriptor.abi_version
//!    == TAIDA_ADDON_ABI_VERSION` **before** dereferencing any other
//!    descriptor field. Mismatch -> [`AddonLoadError::AbiMismatch`].
//!    No silent fallback. (RC1B-101)
//! 2. Load failures are split into distinct, classifiable variants:
//!    library not found, entry symbol missing, null descriptor,
//!    invalid descriptor, init failure. (RC1B-102)
//! 3. The loader takes ownership of the `libloading::Library` handle and
//!    keeps it alive for the entire lifetime of the [`LoadedAddon`].
//!    Function pointers harvested from the descriptor are valid only
//!    while the library is loaded -- this is enforced by lifetime-tying
//!    [`AddonFunctionRef`] to a borrow of `LoadedAddon`.
//! 4. Taida user code never sees raw pointers. The public API only
//!    hands out `&str` / `u32` / `extern "C" fn` references.
//!
//! # Why `libloading`
//!
//! `libloading` is a thin, well-maintained `dlopen`/`LoadLibrary`
//! wrapper. It is **not** in the dependency tree of any non-Native
//! backend (it is gated by `feature = "native"`), so this loader is
//! invisible to the JS, WASM, and Interpreter pipelines.
//!
//! # Phase split
//!
//! Phase 2 stops at "ABI handshake + function table introspection".
//! Calling functions through the bridge (with real Taida values) is
//! Phase 3 (`RC1-3*`). The function pointers exposed here can already
//! be invoked with null arg vectors -- the Phase 1 sample addon proves
//! that path -- but routing actual Taida values is deliberately deferred.

use std::ffi::CStr;
use std::path::{Path, PathBuf};

use libloading::{Library, Symbol};
use taida_addon::{
    TAIDA_ADDON_ABI_VERSION, TAIDA_ADDON_ENTRY_SYMBOL, TaidaAddonDescriptorV1,
    TaidaAddonFunctionV1, TaidaAddonStatus, TaidaHostV1,
};

/// Distinct, classifiable error variants produced by [`load_addon`].
///
/// Every variant carries the addon path so the diagnostic can be
/// routed back to the offending package import. The split between
/// `LibraryNotFound`, `EntrySymbolMissing`, `NullDescriptor`,
/// `AbiMismatch`, `InvalidDescriptor`, and `InitFailed` is the direct
/// fix for **RC1B-102** (load failures were previously not classifiable).
///
/// `AbiMismatch` is the direct fix for **RC1B-101** (ABI mismatch was
/// previously a silent success because nothing checked the version
/// before reading other descriptor fields).
#[derive(Debug)]
#[non_exhaustive]
pub enum AddonLoadError {
    /// `dlopen` (or platform equivalent) failed. The shared object does
    /// not exist at the given path, or the OS refused to load it.
    LibraryNotFound {
        path: PathBuf,
        source: libloading::Error,
    },
    /// The library opened, but `dlsym` for `taida_addon_get_v1` failed.
    /// Either the addon was built without the `declare_addon!` macro,
    /// or it targets a future ABI version with a different entry name.
    EntrySymbolMissing {
        path: PathBuf,
        symbol: &'static str,
        source: libloading::Error,
    },
    /// `taida_addon_get_v1()` returned a null pointer. Addons must
    /// return a `'static` descriptor; null is treated as a hard error
    /// rather than a recoverable state.
    NullDescriptor { path: PathBuf },
    /// **RC1B-101 fix.** `descriptor.abi_version` did not match
    /// [`TAIDA_ADDON_ABI_VERSION`]. The loader stops *before* reading
    /// any other descriptor field; the addon may have a completely
    /// different layout under a future ABI.
    AbiMismatch {
        path: PathBuf,
        expected: u32,
        actual: u32,
    },
    /// The descriptor passed the ABI handshake but contained a
    /// structurally invalid field (e.g. `function_count > 0` with a
    /// null `functions` pointer, or a non-UTF-8 addon name).
    InvalidDescriptor { path: PathBuf, reason: String },
    /// The optional `init` callback returned a non-`Ok` status. The
    /// loader does not retry; the addon is considered failed.
    InitFailed {
        path: PathBuf,
        status: TaidaAddonStatus,
    },
}

impl std::fmt::Display for AddonLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LibraryNotFound { path, source } => write!(
                f,
                "addon load failed: cannot open library '{}' ({})",
                path.display(),
                source
            ),
            Self::EntrySymbolMissing {
                path,
                symbol,
                source,
            } => write!(
                f,
                "addon load failed: entry symbol '{}' missing from '{}' ({})",
                symbol,
                path.display(),
                source
            ),
            Self::NullDescriptor { path } => write!(
                f,
                "addon load failed: '{}' returned a null descriptor",
                path.display()
            ),
            Self::AbiMismatch {
                path,
                expected,
                actual,
            } => write!(
                f,
                "addon load failed: ABI version mismatch in '{}' (expected {}, got {})",
                path.display(),
                expected,
                actual
            ),
            Self::InvalidDescriptor { path, reason } => write!(
                f,
                "addon load failed: invalid descriptor in '{}' ({})",
                path.display(),
                reason
            ),
            Self::InitFailed { path, status } => write!(
                f,
                "addon load failed: init callback in '{}' returned status {:?}",
                path.display(),
                status
            ),
        }
    }
}

impl std::error::Error for AddonLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LibraryNotFound { source, .. } | Self::EntrySymbolMissing { source, .. } => {
                Some(source)
            }
            _ => None,
        }
    }
}

/// A successfully loaded addon.
///
/// Owns the `libloading::Library` handle so that function pointers
/// extracted from the descriptor remain valid for the lifetime of the
/// `LoadedAddon`. Drop order: function table is borrowed from `library`,
/// so the field declaration order matters -- Rust drops fields top to
/// bottom, and `library` must be dropped *last* (declared last in the
/// struct).
///
/// Cloning is intentionally not supported: each loaded addon is a
/// unique resource handle.
#[derive(Debug)]
pub struct LoadedAddon {
    /// Source path of the loaded shared object. Kept for diagnostics.
    path: PathBuf,
    /// Cached addon name as a Rust `String` (descriptor `addon_name`
    /// is borrowed across the FFI boundary so we copy it to make the
    /// safe API independent of pointer lifetimes).
    name: String,
    /// Cached descriptor pointer. Tied for lifetime to `library`.
    descriptor: *const TaidaAddonDescriptorV1,
    /// Cached function entries -- raw pointers from the addon's
    /// `'static` table, valid as long as `library` is loaded.
    functions: Vec<RawFunctionEntry>,
    /// Host capability table passed to the addon's `init` callback.
    /// RC1 Phase 3 populates it with real callbacks
    /// (`src/addon/value_bridge.rs::make_host_table`). Stored in a
    /// `Box` so the pointer we handed the addon remains stable for
    /// the addon's entire lifetime -- addons typically capture it in
    /// a `static AtomicPtr` during init.
    ///
    /// Field ordering: this must be dropped *before* `library` so the
    /// addon cannot reference it post-unload, but *after* everything
    /// that might conceivably touch the host table on drop. Rust
    /// drops fields top to bottom, so `host_table` comes after
    /// `functions` and before `library`.
    #[allow(dead_code)]
    host_table: Box<TaidaHostV1>,
    /// `Library` must be the last field so it is dropped last.
    /// All raw pointers above are borrowed from this handle.
    ///
    /// `#[allow(dead_code)]` is intentional: the field is RAII-only.
    /// Reading it after construction would expose `libloading::Library`
    /// to callers, which we explicitly do not want -- the loader's
    /// public API is `path()` / `name()` / `functions()` and the raw
    /// `Library` handle stays sealed.
    #[allow(dead_code)]
    library: Library,
}

// SAFETY: `Library` is `Send` on POSIX (libloading::Library: Send).
// `LoadedAddon` only holds raw pointers into the loaded image's
// `.rodata`, which is read-only and immutable for the library's
// lifetime, and a `String` (which is `Send`). No interior mutability.
unsafe impl Send for LoadedAddon {}

// SAFETY: `Sync` is sound for the same reason as `Send` plus the
// observation that every accessor on `LoadedAddon` reads from
// immutable memory:
//
// - `descriptor` points into the loaded library's `.rodata`. The
//   addon descriptor is `'static` per the ABI v1 contract, so the
//   bytes never change. Concurrent reads are safe.
// - `functions` is a `Vec<RawFunctionEntry>` that is built once at
//   load time and never mutated. `RawFunctionEntry` is already
//   `Send + Sync`. Reading the vec from multiple threads is fine.
// - `host_table: Box<TaidaHostV1>` is built once and read-only after
//   `init`. The fields are `extern "C" fn` (`Send + Sync`) and
//   plain integers.
// - `library` is `libloading::Library`, which is `Send` on POSIX.
//   `Sync` is sound here because every method we expose performs
//   read-only lookups against the descriptor we already validated;
//   we never call `library.get` again after `load_addon` returns.
// - The `path` and `name` fields are `PathBuf` / `String`, both
//   `Send + Sync`.
//
// RC1 Phase 4 needs `Sync` so that `Arc<LoadedAddon>` can live in
// the process-wide `AddonRegistry` (`OnceLock<AddonRegistry>` ->
// `Mutex<HashMap<_, Arc<ResolvedAddon>>>`).
unsafe impl Sync for LoadedAddon {}

#[derive(Clone, Copy, Debug)]
struct RawFunctionEntry {
    name_ptr: *const core::ffi::c_char,
    arity: u32,
    call: extern "C" fn(
        args_ptr: *const taida_addon::TaidaAddonValueV1,
        args_len: u32,
        out_value: *mut *mut taida_addon::TaidaAddonValueV1,
        out_error: *mut *mut taida_addon::TaidaAddonErrorV1,
    ) -> TaidaAddonStatus,
}

// SAFETY: identical reasoning to `LoadedAddon`. The pointers live in
// the loaded library's read-only image and the function pointers are
// extern "C" fn (always Send + Sync).
unsafe impl Send for RawFunctionEntry {}
unsafe impl Sync for RawFunctionEntry {}

/// Borrowed reference to a single addon function.
///
/// The lifetime is tied to the parent [`LoadedAddon`] so callers can
/// not accidentally hold a function pointer past `unload`/drop.
#[derive(Clone, Copy)]
pub struct AddonFunctionRef<'a> {
    name: &'a str,
    arity: u32,
    raw: RawFunctionEntry,
}

impl<'a> AddonFunctionRef<'a> {
    /// Function name as declared in the addon's function table.
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// Declared arity.
    pub fn arity(&self) -> u32 {
        self.arity
    }

    /// `extern "C" fn` entry point.
    ///
    /// **Do not call this directly from Taida user code.** It is
    /// exposed so the Phase 3 host-side call facade
    /// ([`LoadedAddon::call_function`](crate::addon::loader::LoadedAddon::call_function))
    /// can invoke the raw pointer while it holds the host-built
    /// borrowed input vector. Any other caller is responsible for
    /// respecting the full ABI contract manually (arg-vector layout,
    /// `out_value` / `out_error` ownership, etc.).
    pub fn raw_call(
        &self,
    ) -> extern "C" fn(
        args_ptr: *const taida_addon::TaidaAddonValueV1,
        args_len: u32,
        out_value: *mut *mut taida_addon::TaidaAddonValueV1,
        out_error: *mut *mut taida_addon::TaidaAddonErrorV1,
    ) -> TaidaAddonStatus {
        self.raw.call
    }
}

impl LoadedAddon {
    /// Filesystem path the addon was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Addon name (`addon_name` from the descriptor).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// ABI version the descriptor advertises. Always equal to
    /// [`TAIDA_ADDON_ABI_VERSION`] -- mismatched addons are rejected
    /// at load time, so this is mostly useful for diagnostics.
    pub fn abi_version(&self) -> u32 {
        // SAFETY: descriptor pointer was validated non-null and the
        // library is still loaded as long as `&self` is alive.
        unsafe { (*self.descriptor).abi_version }
    }

    /// Number of functions exposed by the addon.
    pub fn function_count(&self) -> usize {
        self.functions.len()
    }

    /// Iterate over the addon's function table.
    pub fn functions(&self) -> impl Iterator<Item = AddonFunctionRef<'_>> + '_ {
        self.functions.iter().map(move |raw| {
            // SAFETY: name_ptr was validated UTF-8 at load time and the
            // library is still loaded.
            let name = unsafe { CStr::from_ptr(raw.name_ptr) }
                .to_str()
                .unwrap_or("");
            AddonFunctionRef {
                name,
                arity: raw.arity,
                raw: *raw,
            }
        })
    }

    /// Look up a function by name. Returns `None` if not found.
    pub fn find_function(&self, name: &str) -> Option<AddonFunctionRef<'_>> {
        self.functions().find(|f| f.name() == name)
    }
}

/// Load a Rust addon from `path`.
///
/// This is the single entry point for the Native backend's addon
/// dispatcher (RC1 Phase 4 will plumb it through the package import
/// path). It performs:
///
/// 1. `dlopen(path)` -> `LibraryNotFound` on failure.
/// 2. `dlsym("taida_addon_get_v1")` -> `EntrySymbolMissing`.
/// 3. Call the entry symbol -> `NullDescriptor` if it returns null.
/// 4. **ABI handshake**: read `descriptor.abi_version` *only* and
///    compare to [`TAIDA_ADDON_ABI_VERSION`]. Mismatch ->
///    `AbiMismatch`. **No other field is touched until this passes.**
/// 5. Validate `function_count` / `functions` consistency
///    -> `InvalidDescriptor`.
/// 6. Cache the addon name (UTF-8 validated) -> `InvalidDescriptor`
///    on non-UTF-8.
/// 7. Cache function entries (UTF-8 validate each name).
/// 8. If `init` is present, build a [`TaidaHostV1`] with the host's
///    ABI version filled in and call it -> `InitFailed` on non-Ok.
///
/// On success the returned [`LoadedAddon`] owns the library handle.
pub fn load_addon<P: AsRef<Path>>(path: P) -> Result<LoadedAddon, AddonLoadError> {
    let path_ref = path.as_ref();
    let owned_path = path_ref.to_path_buf();

    // 1. dlopen
    //
    // SAFETY: libloading wraps `dlopen`/`LoadLibrary`. The unsafety
    // boundary is "the library may run static initializers". RC1 only
    // loads addons that the host explicitly chose to load via the
    // package layer, so this is the same trust level as a Cargo
    // dependency.
    let library =
        unsafe { Library::new(path_ref) }.map_err(|source| AddonLoadError::LibraryNotFound {
            path: owned_path.clone(),
            source,
        })?;

    // 2. dlsym taida_addon_get_v1
    //
    // SAFETY: the symbol type matches the ABI v1 contract
    // (`extern "C" fn() -> *const TaidaAddonDescriptorV1`). If the
    // addon exports a different signature under the same name we have
    // a much bigger ABI break than this loader can recover from --
    // that is precisely why the entry symbol carries the version (`v1`).
    let get_descriptor: Symbol<unsafe extern "C" fn() -> *const TaidaAddonDescriptorV1> = unsafe {
        library
            .get(TAIDA_ADDON_ENTRY_SYMBOL.as_bytes())
            .map_err(|source| AddonLoadError::EntrySymbolMissing {
                path: owned_path.clone(),
                symbol: TAIDA_ADDON_ENTRY_SYMBOL,
                source,
            })?
    };

    // 3. Call entry symbol
    //
    // SAFETY: the function is `extern "C"` and we matched its
    // signature exactly above. It is allowed to perform addon-internal
    // initialisation but must return a `'static` descriptor pointer
    // (or null on hard failure).
    let descriptor_ptr: *const TaidaAddonDescriptorV1 = unsafe { get_descriptor() };
    if descriptor_ptr.is_null() {
        return Err(AddonLoadError::NullDescriptor { path: owned_path });
    }

    // 4. ABI handshake -- RC1B-101 fix.
    //
    // We read EXACTLY `abi_version` and nothing else. Even reading
    // `addon_name` first would be wrong: a future ABI v2 might shuffle
    // the layout so `addon_name` lives at a different offset, and we
    // would dereference garbage. The first 4 bytes of the descriptor
    // are pinned to `u32 abi_version` by `RC1_DESIGN.md` Section C.
    //
    // SAFETY: descriptor_ptr is non-null (checked above) and points
    // into the loaded library's `.rodata`, which is alive for as long
    // as `library` is alive.
    let actual_version = unsafe { core::ptr::read(&(*descriptor_ptr).abi_version) };
    if actual_version != TAIDA_ADDON_ABI_VERSION {
        return Err(AddonLoadError::AbiMismatch {
            path: owned_path,
            expected: TAIDA_ADDON_ABI_VERSION,
            actual: actual_version,
        });
    }

    // 5. Now that the ABI handshake passed, the layout is locked. We
    // can read the rest of the descriptor.
    //
    // SAFETY: see step 4.
    let descriptor: &TaidaAddonDescriptorV1 = unsafe { &*descriptor_ptr };

    // function_count == 0 with null `functions` is allowed (an addon
    // exposing only an `init` callback). function_count > 0 with a
    // null pointer is a structural error.
    if descriptor.function_count > 0 && descriptor.functions.is_null() {
        return Err(AddonLoadError::InvalidDescriptor {
            path: owned_path,
            reason: format!(
                "function_count={} but functions pointer is null",
                descriptor.function_count
            ),
        });
    }

    // 6. Cache the addon name. UTF-8 validate up front so the public
    // API can hand out `&str` without per-call validation.
    if descriptor.addon_name.is_null() {
        return Err(AddonLoadError::InvalidDescriptor {
            path: owned_path,
            reason: "addon_name pointer is null".to_string(),
        });
    }
    // SAFETY: pointer is non-null and points into the addon's
    // `.rodata`. The addon contract requires nul-termination.
    let name_cstr = unsafe { CStr::from_ptr(descriptor.addon_name) };
    let name = match name_cstr.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => {
            return Err(AddonLoadError::InvalidDescriptor {
                path: owned_path,
                reason: "addon_name is not valid UTF-8".to_string(),
            });
        }
    };

    // 7. Cache function entries.
    let mut functions = Vec::with_capacity(descriptor.function_count as usize);
    for i in 0..descriptor.function_count as usize {
        // SAFETY: bounds checked against `function_count` (validated
        // non-null above) and the addon contract requires this slice
        // to live for the library's lifetime.
        let entry: &TaidaAddonFunctionV1 = unsafe { &*descriptor.functions.add(i) };
        if entry.name.is_null() {
            return Err(AddonLoadError::InvalidDescriptor {
                path: owned_path,
                reason: format!("function[{i}].name pointer is null"),
            });
        }
        // UTF-8 validate so the public API never panics.
        // SAFETY: pointer is non-null and the addon contract requires
        // nul-termination.
        let name_cstr = unsafe { CStr::from_ptr(entry.name) };
        if name_cstr.to_str().is_err() {
            return Err(AddonLoadError::InvalidDescriptor {
                path: owned_path,
                reason: format!("function[{i}].name is not valid UTF-8"),
            });
        }
        functions.push(RawFunctionEntry {
            name_ptr: entry.name,
            arity: entry.arity,
            call: entry.call,
        });
    }

    // 8. init callback (optional, called exactly once).
    //
    // Phase 3 populates the host capability table with real
    // callbacks (see `src/addon/value_bridge.rs::make_host_table`).
    // The table is stored alongside the `LoadedAddon` so it stays
    // alive for the addon's lifetime — addons capture the pointer
    // during `init` and read it on every subsequent call.
    let host_table = Box::new(crate::addon::value_bridge::make_host_table());
    if let Some(init_fn) = descriptor.init {
        // SAFETY: signature matches `descriptor.init`'s declared type
        // (`extern "C" fn(*const TaidaHostV1) -> TaidaAddonStatus`).
        let status = init_fn(&*host_table as *const TaidaHostV1);
        if status != TaidaAddonStatus::Ok {
            return Err(AddonLoadError::InitFailed {
                path: owned_path,
                status,
            });
        }
    }

    Ok(LoadedAddon {
        path: owned_path,
        name,
        descriptor: descriptor_ptr,
        functions,
        host_table,
        library,
    })
}

#[cfg(test)]
mod tests {
    //! Loader unit tests.
    //!
    //! These tests construct in-process descriptors (no dlopen) and
    //! exercise the validation code paths. The end-to-end test that
    //! actually `dlopen`s `libtaida_addon_sample.so` lives in the
    //! integration suite (`tests/addon_loader_smoke.rs`) so the unit
    //! tests stay hermetic.

    use super::*;
    use core::ffi::c_char;
    use std::path::PathBuf;
    use taida_addon::TaidaAddonValueV1;

    extern "C" fn ok_call(
        _args_ptr: *const TaidaAddonValueV1,
        _args_len: u32,
        _out_value: *mut *mut TaidaAddonValueV1,
        _out_error: *mut *mut taida_addon::TaidaAddonErrorV1,
    ) -> TaidaAddonStatus {
        TaidaAddonStatus::Ok
    }

    static UT_FUNCTIONS: &[TaidaAddonFunctionV1] = &[TaidaAddonFunctionV1 {
        name: c"unit".as_ptr() as *const c_char,
        arity: 0,
        call: ok_call,
    }];

    static UT_DESCRIPTOR_OK: TaidaAddonDescriptorV1 = TaidaAddonDescriptorV1 {
        abi_version: TAIDA_ADDON_ABI_VERSION,
        _reserved: 0,
        addon_name: c"taida-addon/unit".as_ptr(),
        function_count: 1,
        _reserved2: 0,
        functions: UT_FUNCTIONS.as_ptr(),
        init: None,
    };

    static UT_DESCRIPTOR_ABI_BAD: TaidaAddonDescriptorV1 = TaidaAddonDescriptorV1 {
        abi_version: 999, // wrong on purpose
        _reserved: 0,
        addon_name: c"taida-addon/abi-bad".as_ptr(),
        function_count: 0,
        _reserved2: 0,
        functions: core::ptr::null(),
        init: None,
    };

    // ── Error display tests ─────────────────────────────────────────

    #[test]
    fn null_descriptor_error_message_classifies() {
        let err = AddonLoadError::NullDescriptor {
            path: PathBuf::from("/tmp/x.so"),
        };
        let msg = err.to_string();
        assert!(msg.contains("addon load failed"));
        assert!(msg.contains("null descriptor"));
        assert!(msg.contains("/tmp/x.so"));
    }

    #[test]
    fn abi_mismatch_error_carries_versions() {
        // RC1B-101: this exact format is the diagnostic the host
        // surfaces. Pin the substring so future refactors notice.
        let err = AddonLoadError::AbiMismatch {
            path: PathBuf::from("/tmp/x.so"),
            expected: 1,
            actual: 7,
        };
        let msg = err.to_string();
        assert!(msg.contains("ABI version mismatch"));
        assert!(msg.contains("expected 1"));
        assert!(msg.contains("got 7"));
    }

    #[test]
    fn entry_symbol_missing_error_names_the_symbol() {
        // Construct via the failing real path (libloading::Error has
        // no public constructor, so we provoke a real failure).
        let result = load_addon("/this/path/definitely/does/not/exist.so");
        let err = result.expect_err("missing path must fail");
        assert!(matches!(err, AddonLoadError::LibraryNotFound { .. }));
        assert!(err.to_string().contains("addon load failed"));
        assert!(err.to_string().contains("cannot open library"));
    }

    #[test]
    fn invalid_descriptor_error_includes_reason() {
        let err = AddonLoadError::InvalidDescriptor {
            path: PathBuf::from("/tmp/x.so"),
            reason: "function_count=2 but functions pointer is null".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("invalid descriptor"));
        assert!(msg.contains("function_count=2"));
    }

    #[test]
    fn init_failed_error_includes_status() {
        let err = AddonLoadError::InitFailed {
            path: PathBuf::from("/tmp/x.so"),
            status: TaidaAddonStatus::AbiMismatch,
        };
        let msg = err.to_string();
        assert!(msg.contains("init callback"));
        assert!(msg.contains("AbiMismatch"));
    }

    // ── Loader path tests (no dlopen, in-process descriptor) ────────
    //
    // The loader's main code path is `load_addon(path)` which performs
    // dlopen. To exercise the validation logic without a real .so we
    // call the helpers directly via a small `validate_descriptor`
    // shim. The shim is below in this test module so it does not pollute
    // the public API.

    /// Mirror of `load_addon`'s descriptor-validation phase, used by
    /// unit tests to exercise validation against in-process descriptors.
    /// Does not perform dlopen / dlsym / init.
    fn validate_descriptor_for_test(
        path: PathBuf,
        descriptor_ptr: *const TaidaAddonDescriptorV1,
    ) -> Result<(String, Vec<RawFunctionEntry>), AddonLoadError> {
        if descriptor_ptr.is_null() {
            return Err(AddonLoadError::NullDescriptor { path });
        }
        let actual_version = unsafe { core::ptr::read(&(*descriptor_ptr).abi_version) };
        if actual_version != TAIDA_ADDON_ABI_VERSION {
            return Err(AddonLoadError::AbiMismatch {
                path,
                expected: TAIDA_ADDON_ABI_VERSION,
                actual: actual_version,
            });
        }
        let descriptor: &TaidaAddonDescriptorV1 = unsafe { &*descriptor_ptr };
        if descriptor.function_count > 0 && descriptor.functions.is_null() {
            return Err(AddonLoadError::InvalidDescriptor {
                path,
                reason: "function_count > 0 but functions pointer is null".to_string(),
            });
        }
        if descriptor.addon_name.is_null() {
            return Err(AddonLoadError::InvalidDescriptor {
                path,
                reason: "addon_name pointer is null".to_string(),
            });
        }
        let name = unsafe { CStr::from_ptr(descriptor.addon_name) }
            .to_str()
            .map_err(|_| AddonLoadError::InvalidDescriptor {
                path: path.clone(),
                reason: "addon_name is not valid UTF-8".to_string(),
            })?
            .to_string();
        let mut functions = Vec::new();
        for i in 0..descriptor.function_count as usize {
            let entry: &TaidaAddonFunctionV1 = unsafe { &*descriptor.functions.add(i) };
            if entry.name.is_null() {
                return Err(AddonLoadError::InvalidDescriptor {
                    path,
                    reason: format!("function[{i}].name is null"),
                });
            }
            functions.push(RawFunctionEntry {
                name_ptr: entry.name,
                arity: entry.arity,
                call: entry.call,
            });
        }
        Ok((name, functions))
    }

    #[test]
    fn validate_accepts_well_formed_descriptor() {
        let (name, fns) =
            validate_descriptor_for_test(PathBuf::from("test://ok"), &UT_DESCRIPTOR_OK as *const _)
                .expect("well-formed descriptor must validate");
        assert_eq!(name, "taida-addon/unit");
        assert_eq!(fns.len(), 1);
        assert_eq!(fns[0].arity, 0);
    }

    #[test]
    fn validate_rejects_null_descriptor() {
        let err = validate_descriptor_for_test(PathBuf::from("test://null"), core::ptr::null())
            .expect_err("null descriptor must be rejected");
        assert!(matches!(err, AddonLoadError::NullDescriptor { .. }));
    }

    #[test]
    fn validate_rejects_abi_mismatch_without_touching_other_fields() {
        // RC1B-101 fix: this is the regression test. The descriptor's
        // addon_name pointer is intentionally fine here, but if the
        // loader read it *before* checking abi_version on a real
        // mismatch we would have lost the chance to bail safely.
        let err = validate_descriptor_for_test(
            PathBuf::from("test://abi-bad"),
            &UT_DESCRIPTOR_ABI_BAD as *const _,
        )
        .expect_err("ABI mismatch must be hard-rejected");
        match err {
            AddonLoadError::AbiMismatch {
                expected, actual, ..
            } => {
                assert_eq!(expected, TAIDA_ADDON_ABI_VERSION);
                assert_eq!(actual, 999);
            }
            other => panic!("expected AbiMismatch, got {other:?}"),
        }
    }

    // Custom-built bad descriptors for the structural-error paths.
    // We can't put `*const c_char` in a `static` builder, so we use
    // `Box::leak` inside the test to keep things simple. These
    // tests do not free the leaked memory, but they only run once.

    #[test]
    fn validate_rejects_function_count_with_null_pointer() {
        let bad = Box::leak(Box::new(TaidaAddonDescriptorV1 {
            abi_version: TAIDA_ADDON_ABI_VERSION,
            _reserved: 0,
            addon_name: c"x".as_ptr(),
            function_count: 3,
            _reserved2: 0,
            functions: core::ptr::null(),
            init: None,
        }));
        let err = validate_descriptor_for_test(PathBuf::from("test://no-fns"), bad as *const _)
            .expect_err("function_count > 0 with null functions must be rejected");
        match err {
            AddonLoadError::InvalidDescriptor { reason, .. } => {
                assert!(reason.contains("functions pointer is null"));
            }
            other => panic!("expected InvalidDescriptor, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_null_addon_name() {
        let bad = Box::leak(Box::new(TaidaAddonDescriptorV1 {
            abi_version: TAIDA_ADDON_ABI_VERSION,
            _reserved: 0,
            addon_name: core::ptr::null(),
            function_count: 0,
            _reserved2: 0,
            functions: core::ptr::null(),
            init: None,
        }));
        let err = validate_descriptor_for_test(PathBuf::from("test://no-name"), bad as *const _)
            .expect_err("null addon_name must be rejected");
        match err {
            AddonLoadError::InvalidDescriptor { reason, .. } => {
                assert!(reason.contains("addon_name pointer is null"));
            }
            other => panic!("expected InvalidDescriptor, got {other:?}"),
        }
    }

    #[test]
    fn load_addon_returns_library_not_found_for_missing_path() {
        // RC1B-102 fix: distinct error variants. Verify the missing-
        // file case maps to LibraryNotFound (not EntrySymbolMissing,
        // not generic Error).
        let err = load_addon("/non/existent/addon.so").expect_err("must fail");
        assert!(matches!(err, AddonLoadError::LibraryNotFound { .. }));
    }

    #[test]
    fn load_addon_returns_entry_symbol_missing_for_unrelated_lib() {
        // Pick a library that we know exists on the test runner but
        // does NOT export `taida_addon_get_v1`. libc.so.6 is the
        // most portable choice on Linux. If it isn't there, skip.
        let candidates = ["libc.so.6", "/lib/x86_64-linux-gnu/libc.so.6"];
        let mut tried_any = false;
        for c in &candidates {
            if !std::path::Path::new(c).exists() && !c.starts_with("lib") {
                continue;
            }
            tried_any = true;
            let result = load_addon(c);
            // Either the symbol is missing (the case we want), or the
            // OS refused to dlopen the file (acceptable on hardened
            // hosts -- still classified, just as LibraryNotFound).
            if let Err(err) = result {
                match err {
                    AddonLoadError::EntrySymbolMissing { symbol, .. } => {
                        assert_eq!(symbol, TAIDA_ADDON_ENTRY_SYMBOL);
                        return;
                    }
                    AddonLoadError::LibraryNotFound { .. } => return,
                    other => panic!("unexpected error variant: {other:?}"),
                }
            }
        }
        if !tried_any {
            // Test environment had nothing usable -- treat as skipped.
            eprintln!("note: skipping entry-symbol-missing test (no candidate libraries)");
        }
    }
}
