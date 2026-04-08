//! Process-wide addon registry (RC1 Phase 4 -- `RC1-4b` / `RC1-4c`).
//!
//! `.dev/RC1_DESIGN.md` Phase 4 Lock §Runtime registry pins three
//! contracts that this module enforces:
//!
//! 1. **Single load per (project_root, package_id)**: an addon-backed
//!    package is `dlopen`-ed at most once per process. Subsequent
//!    imports return the same `Arc<LoadedAddon>` so the value bridge
//!    allocator stays unified across calls.
//! 2. **No unload until process exit**: `libloading::Library` is held
//!    for the registry's entire lifetime. The registry is `'static`
//!    and never drops, so addon function pointers remain valid for as
//!    long as the host process is running.
//! 3. **Resolution order is single source**: the cdylib search order
//!    documented in the design lock lives entirely inside this module.
//!    Both the interpreter import path and the (future) Cranelift /
//!    JS / WASM compile-time error path call into the same lookup
//!    helper, so they can never drift.
//!
//! # Why a global registry?
//!
//! `LoadedAddon` cannot be cloned (it owns a `libloading::Library`
//! handle), and copying the host capability table would mean two
//! independent allocators -- exactly the problem the Phase 3 ownership
//! lock fixed. The cleanest fix is to keep `LoadedAddon` in an `Arc`
//! and hand `Arc` clones to every consumer. The natural place for an
//! `Arc<LoadedAddon>` to live is a `'static` registry keyed by
//! `(project_root, package_id)`.
//!
//! # Concurrency
//!
//! The registry uses a single `Mutex` around a `HashMap`. Addon
//! loading is rare (once per package per process) and the critical
//! section is microscopic, so contention is not a concern. The
//! `LoadedAddon` itself is `Send + Sync` (Phase 2 `unsafe impl`), so
//! handing out `Arc` clones across threads is safe.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::addon::call::AddonCallError;
use crate::addon::loader::{AddonLoadError, LoadedAddon, load_addon};
use crate::addon::manifest::{AddonManifest, AddonManifestError, parse_addon_manifest};

/// Errors raised when wiring an addon-backed package into the import
/// path. These are the deterministic failure modes the import resolver
/// surfaces to Taida user code.
///
/// Each variant carries enough context (`package`, `path`) for the
/// diagnostic to point at the offending package directory.
#[derive(Debug)]
#[non_exhaustive]
pub enum AddonImportError {
    /// `parse_addon_manifest` failed (syntax / validation error).
    Manifest(AddonManifestError),
    /// `addon.toml` validated, but the cdylib was not found in any of
    /// the documented search locations. The `searched` field lists
    /// every path the resolver tried so users can fix the layout.
    LibraryNotFound {
        package: String,
        library: String,
        searched: Vec<PathBuf>,
    },
    /// `dlopen` / handshake failed.
    Loader(AddonLoadError),
    /// The addon registry `Mutex` was found to be poisoned (a previous
    /// holder panicked). This is a process-level failure, but we
    /// surface it as a recoverable error rather than propagating the
    /// panic to unrelated import paths.
    RegistryPoisoned { package: String },
    /// Manifest declared a function that the loaded cdylib does not
    /// export. This is the over-export protection mandated by the
    /// design lock: the manifest is the source of truth and any drift
    /// between manifest and binary is a hard error.
    FunctionNotInBinary {
        package: String,
        function: String,
        cdylib: PathBuf,
    },
    /// Manifest declared a function with arity X but the cdylib
    /// declared arity Y. Stops the import to prevent runtime arity
    /// confusion.
    ArityMismatch {
        package: String,
        function: String,
        manifest_arity: u32,
        binary_arity: u32,
    },
    /// The `package` field inside `native/addon.toml` does not match
    /// the package id the import resolver was looking up. RC1B-110
    /// contract: the two identifiers must be identical; a drift here
    /// would desynchronise the registry key, the sentinel binding,
    /// and the manifest-reported diagnostics.
    PackageMismatch {
        /// The package id from the `>>>` import statement.
        expected: String,
        /// The `package` value from `native/addon.toml`.
        actual: String,
        /// Path of the `addon.toml` that declared the wrong id.
        manifest_path: PathBuf,
    },
}

impl std::fmt::Display for AddonImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Manifest(e) => write!(f, "{e}"),
            Self::LibraryNotFound {
                package,
                library,
                searched,
            } => {
                write!(
                    f,
                    "addon import failed: cdylib for package '{}' (library stem '{}') not found. Searched: ",
                    package, library
                )?;
                for (i, p) in searched.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p.display())?;
                }
                Ok(())
            }
            Self::Loader(e) => write!(f, "{e}"),
            Self::RegistryPoisoned { package } => write!(
                f,
                "addon import failed: registry mutex poisoned while loading package '{}'",
                package
            ),
            Self::FunctionNotInBinary {
                package,
                function,
                cdylib,
            } => write!(
                f,
                "addon import failed: package '{}' declares function '{}' in addon.toml but it is not exported by '{}'",
                package,
                function,
                cdylib.display()
            ),
            Self::ArityMismatch {
                package,
                function,
                manifest_arity,
                binary_arity,
            } => write!(
                f,
                "addon import failed: package '{}' function '{}' arity mismatch (manifest declares {}, binary declares {})",
                package, function, manifest_arity, binary_arity
            ),
            Self::PackageMismatch {
                expected,
                actual,
                manifest_path,
            } => write!(
                f,
                "addon import failed: package id mismatch in '{}' (import resolver expected '{}', manifest declares '{}')",
                manifest_path.display(),
                expected,
                actual
            ),
        }
    }
}

impl std::error::Error for AddonImportError {}

impl From<AddonManifestError> for AddonImportError {
    fn from(e: AddonManifestError) -> Self {
        Self::Manifest(e)
    }
}

impl From<AddonLoadError> for AddonImportError {
    fn from(e: AddonLoadError) -> Self {
        Self::Loader(e)
    }
}

/// A fully resolved addon-backed package, ready to be bound into the
/// interpreter env.
///
/// Carries the canonical package id, an `Arc` to the loaded addon
/// (process-wide unique), and the validated manifest. Cloning is cheap
/// because the addon handle is reference-counted.
#[derive(Clone, Debug)]
pub struct ResolvedAddon {
    /// Canonical package id (e.g. `"taida-lang/addon-rs-sample"`).
    pub package_id: String,
    /// `Arc` so multiple imports of the same package share the handle.
    pub addon: Arc<LoadedAddon>,
    /// Validated manifest. Held alongside the addon so the import
    /// resolver can decide which functions to expose to Taida.
    pub manifest: AddonManifest,
}

impl ResolvedAddon {
    /// Convenience pass-through to `LoadedAddon::call_function`.
    /// Same safety contract: `&[Value] -> Result<Value, AddonCallError>`,
    /// no raw pointers.
    pub fn call_function(
        &self,
        function: &str,
        args: &[crate::interpreter::value::Value],
    ) -> Result<crate::interpreter::value::Value, AddonCallError> {
        self.addon.call_function(function, args)
    }
}

/// Process-wide registry of loaded addons.
///
/// Use [`AddonRegistry::global`] to obtain the singleton. The registry
/// is created lazily on first use.
pub struct AddonRegistry {
    inner: Mutex<HashMap<RegistryKey, Arc<ResolvedAddon>>>,
}

/// `(canonical_project_root, canonical_package_id)` keying for the
/// process-wide registry. We canonicalize project roots so that two
/// imports from different `.taida` cwd spellings still hit the same
/// entry, but we keep the package id as-declared in `addon.toml`.
type RegistryKey = (PathBuf, String);

impl AddonRegistry {
    /// Singleton accessor.
    pub fn global() -> &'static AddonRegistry {
        static REGISTRY: OnceLock<AddonRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| AddonRegistry {
            inner: Mutex::new(HashMap::new()),
        })
    }

    /// Look up a previously-loaded addon.
    ///
    /// Returns `None` if the package has not been loaded yet (or if
    /// the project_root / package_id pair has not been registered).
    /// Used by the interpreter dispatch path which expects the import
    /// resolver to have already populated the registry.
    pub fn lookup(&self, project_root: &Path, package_id: &str) -> Option<Arc<ResolvedAddon>> {
        let canonical_root = canonical_or_owned(project_root);
        let key = (canonical_root, package_id.to_string());
        self.inner.lock().ok()?.get(&key).cloned()
    }

    /// Idempotently load an addon-backed package.
    ///
    /// On first call for a given `(project_root, package_id)`, this
    /// performs the full chain: parse manifest -> resolve cdylib path
    /// -> `load_addon` -> cross-check function table -> publish
    /// `Arc<ResolvedAddon>` into the registry. Subsequent calls return
    /// the cached `Arc`.
    ///
    /// `pkg_dir` is the resolved package directory (e.g. the result of
    /// `resolve_package_module(_versioned)`). `package_id` is the
    /// canonical package name from the import statement (e.g.
    /// `"taida-lang/addon-rs-sample"`).
    pub fn ensure_loaded(
        &self,
        project_root: &Path,
        package_id: &str,
        pkg_dir: &Path,
    ) -> Result<Arc<ResolvedAddon>, AddonImportError> {
        let canonical_root = canonical_or_owned(project_root);
        let key: RegistryKey = (canonical_root.clone(), package_id.to_string());

        {
            let map = self
                .inner
                .lock()
                .map_err(|_| AddonImportError::RegistryPoisoned {
                    package: package_id.to_string(),
                })?;
            if let Some(existing) = map.get(&key) {
                return Ok(existing.clone());
            }
        }

        // 1. Parse the addon manifest.
        let manifest_path = pkg_dir.join("native").join("addon.toml");
        let manifest = parse_addon_manifest(&manifest_path)?;

        // 1a. Cross-check the manifest's declared `package` against
        //     the package id the import resolver was looking up
        //     (RC1B-110). A drift here would desynchronise:
        //       - the registry key (`package_id`)
        //       - the sentinel string (`__taida_addon_call::<pkg>::...`)
        //       - manifest-reported diagnostics (`manifest.package`)
        //     so it is a hard import-time error with a dedicated
        //     variant for deterministic classification.
        if manifest.package != package_id {
            return Err(AddonImportError::PackageMismatch {
                expected: package_id.to_string(),
                actual: manifest.package.clone(),
                manifest_path: manifest_path.clone(),
            });
        }

        // 2. Resolve the cdylib path.
        let cdylib = resolve_cdylib_path(pkg_dir, &manifest.library).ok_or_else(|| {
            AddonImportError::LibraryNotFound {
                package: manifest.package.clone(),
                library: manifest.library.clone(),
                searched: cdylib_search_paths(pkg_dir, &manifest.library),
            }
        })?;

        // 3. Load the addon (this performs ABI handshake + init).
        let loaded = load_addon(&cdylib)?;

        // 4. Cross-check declared function table against binary.
        for (declared_name, declared_arity) in &manifest.functions {
            match loaded.find_function(declared_name) {
                Some(actual) => {
                    if actual.arity() != *declared_arity {
                        return Err(AddonImportError::ArityMismatch {
                            package: manifest.package.clone(),
                            function: declared_name.clone(),
                            manifest_arity: *declared_arity,
                            binary_arity: actual.arity(),
                        });
                    }
                }
                None => {
                    return Err(AddonImportError::FunctionNotInBinary {
                        package: manifest.package.clone(),
                        function: declared_name.clone(),
                        cdylib: cdylib.clone(),
                    });
                }
            }
        }

        // 5. Publish into the registry. We re-check that another
        //    thread has not raced ahead while we were loading.
        let resolved = Arc::new(ResolvedAddon {
            package_id: package_id.to_string(),
            addon: Arc::new(loaded),
            manifest,
        });

        let mut map = self
            .inner
            .lock()
            .map_err(|_| AddonImportError::RegistryPoisoned {
                package: package_id.to_string(),
            })?;
        if let Some(existing) = map.get(&key) {
            // Another thread raced; prefer the older entry to keep
            // pointer identity stable. The `loaded` we just built will
            // drop here, which unloads the second copy of the library.
            // That is fine -- both copies have the same descriptor and
            // there is no shared state outside the libloading handle.
            return Ok(existing.clone());
        }
        map.insert(key, resolved.clone());
        Ok(resolved)
    }
}

/// Canonicalize `path` if possible, falling back to the path as-given.
/// Used for registry keying so two spellings of the same project root
/// hash to the same entry.
fn canonical_or_owned(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Compute the platform-specific cdylib filename for `library_stem`.
///
/// Returns the filename **inside** the conventional package layout
/// (i.e. `lib<stem>.so` on Linux, `lib<stem>.dylib` on macOS,
/// `<stem>.dll` on Windows).
fn platform_cdylib_filename(library_stem: &str) -> String {
    if cfg!(target_os = "linux") {
        format!("lib{}.so", library_stem)
    } else if cfg!(target_os = "macos") {
        format!("lib{}.dylib", library_stem)
    } else if cfg!(target_os = "windows") {
        format!("{}.dll", library_stem)
    } else {
        // Unknown OS: fall back to Linux-style naming. The actual load
        // attempt will fail with a deterministic LibraryNotFound, so
        // there is no UB risk here.
        format!("lib{}.so", library_stem)
    }
}

/// Returns every cdylib search path the resolver will try, in order.
///
/// Used both by the resolver itself and by the
/// `AddonImportError::LibraryNotFound` diagnostic so the user sees
/// exactly which paths were searched.
pub fn cdylib_search_paths(pkg_dir: &Path, library_stem: &str) -> Vec<PathBuf> {
    let filename = platform_cdylib_filename(library_stem);
    let mut paths = vec![pkg_dir.join("native").join(&filename)];

    let target_root = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            // When running inside the workspace, fall back to
            // `<workspace>/target/`. We approximate "workspace" by
            // walking up from `pkg_dir` until we find a Cargo.toml,
            // because the package directory may live anywhere on disk.
            //
            // For RC1 dev addons (taida-addon-sample), pkg_dir lives
            // *outside* the cargo workspace (under .taida/deps), so the
            // walk-up does not find anything. We instead use the
            // CARGO_MANIFEST_DIR env var when present (set by cargo
            // test) so the search hits the workspace target dir.
            std::env::var_os("CARGO_MANIFEST_DIR").map(|d| PathBuf::from(d).join("target"))
        });

    if let Some(root) = target_root {
        paths.push(root.join("debug").join(&filename));
        paths.push(root.join("release").join(&filename));
        paths.push(root.join("debug").join("deps").join(&filename));
        paths.push(root.join("release").join("deps").join(&filename));
    }

    paths
}

/// Walk the cdylib search order and return the first existing path.
///
/// `RC1_DESIGN.md` Phase 4 Lock §Resolution order Step 5:
/// 1. `<pkg_dir>/native/lib<stem>.{so,dylib,dll}`
/// 2. `${CARGO_TARGET_DIR}/debug/...`
/// 3. `${CARGO_TARGET_DIR}/release/...`
fn resolve_cdylib_path(pkg_dir: &Path, library_stem: &str) -> Option<PathBuf> {
    cdylib_search_paths(pkg_dir, library_stem)
        .into_iter()
        .find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_singleton_returns_same_instance() {
        let a = AddonRegistry::global() as *const _;
        let b = AddonRegistry::global() as *const _;
        assert_eq!(a, b);
    }

    #[test]
    fn lookup_on_empty_registry_returns_none() {
        // Use a unique key so this test does not interfere with other
        // tests that may have populated the global registry.
        let project = std::env::temp_dir().join("rc1_phase4_lookup_empty");
        let _ = std::fs::create_dir_all(&project);
        let result = AddonRegistry::global().lookup(&project, "nonexistent/package");
        assert!(result.is_none());
    }

    #[test]
    fn ensure_loaded_propagates_manifest_error() {
        // Build a temp pkg_dir with a malformed addon.toml.
        let pkg = std::env::temp_dir().join("rc1_phase4_bad_manifest_pkg");
        let _ = std::fs::remove_dir_all(&pkg);
        std::fs::create_dir_all(pkg.join("native")).unwrap();
        std::fs::write(pkg.join("native").join("addon.toml"), "abi = 99\n").unwrap();

        let project = pkg.parent().unwrap().to_path_buf();
        let result = AddonRegistry::global().ensure_loaded(&project, "test/bad-manifest", &pkg);
        match result {
            Err(AddonImportError::Manifest(_)) => {}
            other => panic!("expected Manifest error, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn library_not_found_lists_searched_paths() {
        let pkg = std::env::temp_dir().join("rc1_phase4_no_cdylib_pkg");
        let _ = std::fs::remove_dir_all(&pkg);
        std::fs::create_dir_all(pkg.join("native")).unwrap();
        std::fs::write(
            pkg.join("native").join("addon.toml"),
            r#"
abi = 1
entry = "taida_addon_get_v1"
package = "test/no-cdylib"
library = "definitely_not_built_anywhere_42"

[functions]
noop = 0
"#,
        )
        .unwrap();

        let project = pkg.parent().unwrap().to_path_buf();
        let result = AddonRegistry::global().ensure_loaded(&project, "test/no-cdylib", &pkg);
        match result {
            Err(AddonImportError::LibraryNotFound {
                package,
                library,
                searched,
            }) => {
                assert_eq!(package, "test/no-cdylib");
                assert_eq!(library, "definitely_not_built_anywhere_42");
                assert!(!searched.is_empty(), "searched paths must be reported");
            }
            other => panic!("expected LibraryNotFound, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn cdylib_search_paths_includes_native_subdir_first() {
        let pkg = PathBuf::from("/tmp/test_pkg");
        let paths = cdylib_search_paths(&pkg, "mylib");
        assert!(!paths.is_empty());
        // First entry must be inside the package's native/ subdir, not
        // the workspace target. This pins resolution order from the
        // design lock: published addons live in native/.
        let first = &paths[0];
        assert!(
            first.starts_with(pkg.join("native")),
            "first search path should be inside <pkg>/native, got {}",
            first.display()
        );
    }

    #[test]
    fn library_not_found_display_is_classifiable() {
        let err = AddonImportError::LibraryNotFound {
            package: "x/y".to_string(),
            library: "stem".to_string(),
            searched: vec![PathBuf::from("/a"), PathBuf::from("/b")],
        };
        let msg = err.to_string();
        assert!(msg.contains("addon import failed"));
        assert!(msg.contains("'x/y'"));
        assert!(msg.contains("stem"));
        assert!(msg.contains("/a"));
        assert!(msg.contains("/b"));
    }

    #[test]
    fn arity_mismatch_display_pins_both_arities() {
        let err = AddonImportError::ArityMismatch {
            package: "x/y".to_string(),
            function: "f".to_string(),
            manifest_arity: 2,
            binary_arity: 1,
        };
        let msg = err.to_string();
        assert!(msg.contains("manifest declares 2"));
        assert!(msg.contains("binary declares 1"));
    }

    // RC1B-110 regression: manifest `package` id must match the
    // import resolver's package id. Silent acceptance would
    // desynchronise the registry key and the sentinel binding.
    #[test]
    fn ensure_loaded_rejects_package_id_mismatch() {
        let pkg = std::env::temp_dir().join("rc1b110_pkg_mismatch_pkg");
        let _ = std::fs::remove_dir_all(&pkg);
        std::fs::create_dir_all(pkg.join("native")).unwrap();
        // Manifest declares "evil/wrong-package" but the import
        // resolver is looking up "test/original-package".
        std::fs::write(
            pkg.join("native").join("addon.toml"),
            r#"
abi = 1
entry = "taida_addon_get_v1"
package = "evil/wrong-package"
library = "taida_addon_sample"

[functions]
echo = 1
"#,
        )
        .unwrap();

        let project = pkg.parent().unwrap().to_path_buf();
        let result = AddonRegistry::global().ensure_loaded(&project, "test/original-package", &pkg);
        match result {
            Err(AddonImportError::PackageMismatch {
                expected,
                actual,
                manifest_path,
            }) => {
                assert_eq!(expected, "test/original-package");
                assert_eq!(actual, "evil/wrong-package");
                assert!(
                    manifest_path.ends_with("native/addon.toml"),
                    "manifest_path must point at the offending addon.toml, got {}",
                    manifest_path.display()
                );
            }
            other => panic!("expected PackageMismatch, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&pkg);
    }

    #[test]
    fn package_mismatch_display_pins_both_ids_and_path() {
        let err = AddonImportError::PackageMismatch {
            expected: "org/alpha".to_string(),
            actual: "org/beta".to_string(),
            manifest_path: PathBuf::from("/tmp/pkg/native/addon.toml"),
        };
        let msg = err.to_string();
        assert!(msg.contains("addon import failed"));
        assert!(msg.contains("'org/alpha'"));
        assert!(msg.contains("'org/beta'"));
        assert!(msg.contains("/tmp/pkg/native/addon.toml"));
    }

    #[test]
    fn function_not_in_binary_display_includes_path() {
        let err = AddonImportError::FunctionNotInBinary {
            package: "x/y".to_string(),
            function: "ghost".to_string(),
            cdylib: PathBuf::from("/tmp/libghost.so"),
        };
        let msg = err.to_string();
        assert!(msg.contains("'ghost'"));
        assert!(msg.contains("/tmp/libghost.so"));
    }
}
