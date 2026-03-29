/// Package provider trait and implementations for Taida's Common Package Resolver.
///
/// Three providers resolve dependencies from different sources:
/// - **WorkspaceProvider**: Local path dependencies (`Dependency::Path`)
/// - **CoreBundledProvider**: Core packages bundled with Taida (`taida-lang/*`)
/// - **StoreProvider**: External registry packages (GitHub tarball download)
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::manifest::{Dependency, Manifest};
use super::store::GlobalStore;

/// The source from which a package was resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum PackageSource {
    /// Local path dependency.
    Path(String),
    /// Core package bundled with Taida runtime.
    CoreBundled,
    /// External package from the global store.
    Store { org: String, name: String },
}

/// A successfully resolved package.
#[derive(Debug, Clone)]
pub struct ResolvedPackage {
    /// Package name.
    pub name: String,
    /// Resolved version string.
    pub version: String,
    /// How this package was resolved.
    pub source: PackageSource,
    /// Absolute path to the package contents.
    pub path: PathBuf,
    /// Integrity hash (sha256 of directory contents, hex-encoded).
    pub integrity: String,
}

/// Result of resolving a single dependency.
pub enum ProviderResult {
    /// Successfully resolved.
    Resolved(ResolvedPackage),
    /// This provider cannot handle this dependency type.
    NotApplicable,
    /// Resolution failed with an error message.
    Error(String),
}

/// Trait for package providers.
///
/// Each provider handles a specific category of dependencies.
/// The resolver tries providers in order until one returns `Resolved` or `Error`.
pub trait PackageProvider {
    /// Human-readable name of this provider.
    fn name(&self) -> &str;

    /// Check if this provider can attempt to resolve the given dependency.
    fn can_resolve(&self, dep: &Dependency) -> bool;

    /// Attempt to resolve the dependency.
    fn resolve(&self, name: &str, dep: &Dependency, manifest: &Manifest) -> ProviderResult;

    /// Install a resolved package to the destination directory.
    /// Creates a symlink or copies files as appropriate.
    fn install(&self, resolved: &ResolvedPackage, dest: &Path) -> Result<(), String>;
}

// ── WorkspaceProvider ──────────────────────────────────────

/// Resolves local path dependencies.
pub struct WorkspaceProvider;

impl PackageProvider for WorkspaceProvider {
    fn name(&self) -> &str {
        "workspace"
    }

    fn can_resolve(&self, dep: &Dependency) -> bool {
        matches!(dep, Dependency::Path { .. })
    }

    fn resolve(&self, name: &str, dep: &Dependency, manifest: &Manifest) -> ProviderResult {
        let path = match dep {
            Dependency::Path { path } => path,
            _ => return ProviderResult::NotApplicable,
        };

        let abs_path = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            manifest.root_dir.join(path)
        };

        match abs_path.canonicalize() {
            Ok(canonical) => {
                if canonical.is_dir() {
                    let integrity = compute_dir_hash(&canonical);
                    ProviderResult::Resolved(ResolvedPackage {
                        name: name.to_string(),
                        version: "0.0.0-local".to_string(),
                        source: PackageSource::Path(path.clone()),
                        path: canonical,
                        integrity,
                    })
                } else {
                    ProviderResult::Error(format!(
                        "Dependency '{}': path '{}' is not a directory",
                        name,
                        abs_path.display()
                    ))
                }
            }
            Err(e) => ProviderResult::Error(format!(
                "Dependency '{}': cannot resolve path '{}': {}",
                name,
                abs_path.display(),
                e
            )),
        }
    }

    fn install(&self, resolved: &ResolvedPackage, dest: &Path) -> Result<(), String> {
        let link_path = dest.join(&resolved.name);
        if let Some(parent) = link_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create parent dir for '{}': {}", resolved.name, e))?;
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&resolved.path, &link_path)
                .map_err(|e| format!("Cannot create symlink for '{}': {}", resolved.name, e))?;
        }

        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(&resolved.path, &link_path)
                .map_err(|e| format!("Cannot create symlink for '{}': {}", resolved.name, e))?;
        }

        Ok(())
    }
}

// ── CoreBundledProvider ────────────────────────────────────

/// Core packages bundled with the Taida runtime.
///
/// Currently supports:
/// - `taida-lang/os`: File I/O, environment variables, process execution
/// - `taida-lang/js`: JS interop mold (`JSNew`)
/// - `taida-lang/crypto`: cryptographic primitives (`sha256`)
/// - `taida-lang/net`: network APIs (socket/TCP/UDP contract surface)
/// - `taida-lang/pool`: pool contract surface (official upper package)
///
/// Core-bundled packages are resolved to `~/.taida/bundled/<name>/` (global directory)
/// that is created on demand with stub Taida source files.
pub struct CoreBundledProvider {
    /// Known core-bundled packages: (org, name) -> version
    known: BTreeMap<(String, String), String>,
    /// Override for the bundled root directory (used in tests).
    bundled_root_override: Option<PathBuf>,
}

impl Default for CoreBundledProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreBundledProvider {
    pub fn new() -> Self {
        let mut known = BTreeMap::new();
        known.insert(
            ("taida-lang".to_string(), "os".to_string()),
            "a.1".to_string(),
        );
        known.insert(
            ("taida-lang".to_string(), "js".to_string()),
            "a.1".to_string(),
        );
        known.insert(
            ("taida-lang".to_string(), "crypto".to_string()),
            "a.1".to_string(),
        );
        known.insert(
            ("taida-lang".to_string(), "net".to_string()),
            "a.1".to_string(),
        );
        known.insert(
            ("taida-lang".to_string(), "pool".to_string()),
            "a.1".to_string(),
        );
        CoreBundledProvider {
            known,
            bundled_root_override: None,
        }
    }

    /// Create a CoreBundledProvider with a custom bundled root (for testing).
    #[cfg(test)]
    pub fn with_bundled_root(root: PathBuf) -> Self {
        let mut provider = Self::new();
        provider.bundled_root_override = Some(root);
        provider
    }

    /// Check if a package is a known core-bundled package.
    pub fn is_core_bundled(org: &str, name: &str) -> bool {
        org == "taida-lang" && matches!(name, "os" | "js" | "crypto" | "net" | "pool")
    }

    /// Generate the os package stub source.
    fn os_package_source() -> &'static str {
        r#"// taida-lang/os — Core bundled package
//
// Input APIs (molds -> Lax/Bool):
//   Read[path]()       -- read file contents (64MB limit)
//   ListDir[path]()    -- list directory entries
//   Stat[path]()       -- file metadata (size, modified, isDir)
//   Exists[path]()     -- existence check (returns Bool)
//   EnvVar[name]()     -- environment variable (read-only)
//
// Binary file APIs:
//   readBytes(path)            -- read file as Bytes (64MB limit)
//   writeBytes(path, content)  -- write Bytes payload to file
//
// Side-effect APIs (functions -> Result):
//   writeFile(path, content)    -- write file (create or overwrite)
//   appendFile(path, content)   -- append to file
//   remove(path)                -- remove file/directory
//   createDir(path)             -- mkdir -p
//   rename(from, to)            -- move/rename (atomic)
//
// Process APIs (functions -> Gorillax):
//   run(program, args)          -- direct exec (safe, no shell)
//   execShell(command)          -- shell exec (pipes, redirects)
//     WARNING: Shell injection risk. Prefer run() for safety.
//
// Query APIs:
//   allEnv()                    -- all env vars as HashMap[Str, Str]
//   argv()                      -- CLI user args as @[Str]
//
// Async input APIs (molds -> Async[Lax[T]]):
//   ReadAsync[path]()           -- async file read
//   HttpGet[url]()              -- HTTP GET
//   HttpPost[url, body]()       -- HTTP POST
//   HttpRequest[method, url](...) -- generic HTTP request
//
// Async socket APIs (functions -> Async[Result/Lax]):
//   tcpConnect(host, port[, timeoutMs])
//   tcpListen(port[, timeoutMs])
//   tcpAccept(listener[, timeoutMs])
//   socketSend(socket, data[, timeoutMs])
//   socketSendAll(socket, data[, timeoutMs])
//   socketRecv(socket[, timeoutMs])
//   socketSendBytes(socket, data[, timeoutMs])
//   socketRecvBytes(socket[, timeoutMs])
//   socketRecvExact(socket, size[, timeoutMs])
//   udpBind(host, port[, timeoutMs])
//   udpSendTo(socket, host, port, data[, timeoutMs])
//   udpRecvFrom(socket[, timeoutMs])
//   socketClose(socket)
//   listenerClose(listener)
//   udpClose(socket)            -- alias of socketClose

<<< @(Read, ListDir, Stat, Exists, readBytes, writeFile, writeBytes, appendFile, remove, createDir, rename, run, execShell, EnvVar, allEnv, argv, ReadAsync, HttpGet, HttpPost, HttpRequest, tcpConnect, tcpListen, tcpAccept, socketSend, socketSendAll, socketRecv, socketSendBytes, socketRecvBytes, socketRecvExact, udpBind, udpSendTo, udpRecvFrom, socketClose, listenerClose, udpClose)
"#
    }

    /// Generate the js package stub source.
    fn js_package_source() -> &'static str {
        r#"// taida-lang/js — JS interop package (core bundled)
// JSNew[T](...) — instantiate a JS class: JSNew[Hono]() → new Hono()
// This mold is JS-backend only. Interpreter/Native will error.

<<< @(JSNew)
"#
    }

    /// Generate the crypto package stub source.
    fn crypto_package_source() -> &'static str {
        r#"// taida-lang/crypto — Core bundled crypto package
// Current surface:
//   sha256(value) -- SHA-256 lower-hex digest
//
// Note:
//   `sha256` is exposed via taida-lang/crypto import path only.
//   Prelude compatibility is intentionally not provided.

<<< @(sha256)
"#
    }

    /// Generate the net package stub source.
    fn net_package_source() -> &'static str {
        r#"// taida-lang/net — Core bundled network package
// Legacy surface (delegates to existing socket runtime path):
//   dnsResolve
//   tcpConnect, tcpListen, tcpAccept
//   socketSend, socketSendAll, socketRecv
//   socketSendBytes, socketRecvBytes, socketRecvExact
//   udpBind, udpSendTo, udpRecvFrom
//   socketClose, listenerClose, udpClose
//
// HTTP v1 surface:
//   httpServe, httpParseRequestHead, httpEncodeResponse, readBody
//
// TI-21 contract notes:
//   TLS verification on Http* uses backend default trust store (no insecure -k path)
//   IPv6 outbound resolution/connect is supported via resolver path
//   Unix domain sockets are not provided yet (explicit non-support)

<<< @(dnsResolve, tcpConnect, tcpListen, tcpAccept, socketSend, socketSendAll, socketRecv, socketSendBytes, socketRecvBytes, socketRecvExact, udpBind, udpSendTo, udpRecvFrom, socketClose, listenerClose, udpClose, httpServe, httpParseRequestHead, httpEncodeResponse, readBody, startResponse, writeChunk, endResponse, sseEvent, readBodyChunk, readBodyAll, wsUpgrade, wsSend, wsReceive, wsClose)
"#
    }

    /// Generate the pool package stub source.
    fn pool_package_source() -> &'static str {
        r#"// taida-lang/pool — Core bundled pool package (contract stub)
// TI-22 minimal contract (official upper package):
//   poolCreate(config) -> Result[@(pool)]
//   poolAcquire(pool[, timeoutMs]) -> Async[Result[@(resource, token), _]]
//   poolRelease(pool, token, resource) -> Result[@(ok, reused)]
//   poolClose(pool) -> Async[Result[@(ok), _]]
//   poolHealth(pool) -> @(open, idle, inUse, waiting)
//
// Implementation note:
//   Minimal in-memory pool runtime is provided by core backends.
//   Driver-level connect/validate policy is delegated to upper libraries.

<<< @(poolCreate, poolAcquire, poolRelease, poolClose, poolHealth)
"#
    }

    /// Get the global bundled directory (`~/.taida/bundled/`).
    fn global_bundled_root() -> PathBuf {
        let home = crate::util::taida_home_dir().unwrap_or_else(|_| std::env::temp_dir());
        home.join(".taida").join("bundled")
    }

    /// Ensure the bundled package directory exists with source files.
    fn ensure_bundled_dir(&self, _manifest: &Manifest, pkg_name: &str) -> Result<PathBuf, String> {
        let bundled_root = self
            .bundled_root_override
            .clone()
            .unwrap_or_else(Self::global_bundled_root);
        let bundled_dir = bundled_root.join(pkg_name);
        std::fs::create_dir_all(&bundled_dir)
            .map_err(|e| format!("Cannot create bundled dir for '{}': {}", pkg_name, e))?;

        // Write the package source file
        let main_td = bundled_dir.join("main.td");
        if !main_td.exists() {
            let source = match pkg_name {
                "os" => Self::os_package_source(),
                "js" => Self::js_package_source(),
                "crypto" => Self::crypto_package_source(),
                "net" => Self::net_package_source(),
                "pool" => Self::pool_package_source(),
                _ => "// Unknown core-bundled package\n",
            };
            std::fs::write(&main_td, source)
                .map_err(|e| format!("Cannot write bundled source for '{}': {}", pkg_name, e))?;
        }

        Ok(bundled_dir)
    }
}

impl PackageProvider for CoreBundledProvider {
    fn name(&self) -> &str {
        "core-bundled"
    }

    fn can_resolve(&self, dep: &Dependency) -> bool {
        match dep {
            Dependency::Registry { org, name, .. } => {
                self.known.contains_key(&(org.clone(), name.clone()))
            }
            _ => false,
        }
    }

    fn resolve(&self, dep_name: &str, dep: &Dependency, manifest: &Manifest) -> ProviderResult {
        let (org, pkg_name, version) = match dep {
            Dependency::Registry { org, name, version } => (org, name, version),
            _ => return ProviderResult::NotApplicable,
        };

        let key = (org.clone(), pkg_name.clone());
        let bundled_version = match self.known.get(&key) {
            Some(v) => v,
            None => return ProviderResult::NotApplicable,
        };

        // Version compatibility check: requested version must match bundled version
        if version != bundled_version {
            return ProviderResult::Error(format!(
                "Dependency '{}': requested version {} but bundled version is {}. \
                 Core-bundled packages have a fixed version.",
                dep_name, version, bundled_version
            ));
        }

        match self.ensure_bundled_dir(manifest, pkg_name) {
            Ok(bundled_dir) => {
                let integrity = compute_dir_hash(&bundled_dir);
                ProviderResult::Resolved(ResolvedPackage {
                    name: dep_name.to_string(),
                    version: bundled_version.clone(),
                    source: PackageSource::CoreBundled,
                    path: bundled_dir,
                    integrity,
                })
            }
            Err(e) => ProviderResult::Error(e),
        }
    }

    fn install(&self, resolved: &ResolvedPackage, dest: &Path) -> Result<(), String> {
        let link_path = dest.join(&resolved.name);
        // Ensure parent dir exists (for org/name structure)
        if let Some(parent) = link_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create parent dir for '{}': {}", resolved.name, e))?;
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&resolved.path, &link_path)
                .map_err(|e| format!("Cannot create symlink for '{}': {}", resolved.name, e))?;
        }

        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(&resolved.path, &link_path)
                .map_err(|e| format!("Cannot create symlink for '{}': {}", resolved.name, e))?;
        }

        Ok(())
    }
}

// ── StoreProvider ───────────────────────────────────────

/// External package store provider.
///
/// Downloads packages from GitHub repositories and caches them
/// in the global store (`~/.taida/store/`).
pub struct StoreProvider {
    store: GlobalStore,
    /// When true, bypass local cache for generation resolution (used by `taida update`).
    force_remote: bool,
}

impl Default for StoreProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl StoreProvider {
    pub fn new() -> Self {
        StoreProvider {
            store: GlobalStore::new(),
            force_remote: false,
        }
    }

    /// Create a StoreProvider that bypasses local cache for generation resolution.
    /// Used by `taida update` to always query GitHub for the latest version.
    pub fn new_force_remote() -> Self {
        StoreProvider {
            store: GlobalStore::new(),
            force_remote: true,
        }
    }

    #[cfg(test)]
    pub fn with_store(store: GlobalStore) -> Self {
        StoreProvider {
            store,
            force_remote: false,
        }
    }
}

impl PackageProvider for StoreProvider {
    fn name(&self) -> &str {
        "store"
    }

    fn can_resolve(&self, dep: &Dependency) -> bool {
        matches!(dep, Dependency::Registry { .. })
    }

    fn resolve(&self, dep_name: &str, dep: &Dependency, _manifest: &Manifest) -> ProviderResult {
        match dep {
            Dependency::Registry { org, name, version } => {
                // Determine if this is a gen-only or exact version
                let exact_version = if version.contains('.') {
                    if self.force_remote {
                        // For update: even exact versions need re-fetching if not cached
                        version.clone()
                    } else {
                        // exact: "a.3"
                        version.clone()
                    }
                } else {
                    // gen-only: "a" → resolve to latest in generation
                    let resolve_result = if self.force_remote {
                        self.store.resolve_generation_remote(org, name, version)
                    } else {
                        self.store.resolve_generation(org, name, version)
                    };
                    match resolve_result {
                        Ok(v) => v,
                        Err(e) => return ProviderResult::Error(e),
                    }
                };

                match self.store.fetch_and_cache(org, name, &exact_version) {
                    Ok(path) => {
                        let integrity = compute_dir_hash(&path);
                        ProviderResult::Resolved(ResolvedPackage {
                            name: dep_name.to_string(),
                            version: exact_version,
                            source: PackageSource::Store {
                                org: org.clone(),
                                name: name.clone(),
                            },
                            path,
                            integrity,
                        })
                    }
                    Err(e) => ProviderResult::Error(e),
                }
            }
            _ => ProviderResult::NotApplicable,
        }
    }

    fn install(&self, resolved: &ResolvedPackage, dest: &Path) -> Result<(), String> {
        let link_path = dest.join(&resolved.name);
        if let Some(parent) = link_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Cannot create parent dir for '{}': {}", resolved.name, e))?;
        }

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&resolved.path, &link_path)
                .map_err(|e| format!("Cannot create symlink for '{}': {}", resolved.name, e))?;
        }

        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(&resolved.path, &link_path)
                .map_err(|e| format!("Cannot create symlink for '{}': {}", resolved.name, e))?;
        }

        Ok(())
    }
}

// ── Utilities ──────────────────────────────────────────────

/// Compute a content-based hash of a directory for integrity checking.
///
/// Uses a deterministic traversal (sorted file names) and hashes file paths
/// and file contents using FNV-1a. This ensures that any change to file
/// content is detected, not just changes to file size or structure.
pub fn compute_dir_hash(dir: &Path) -> String {
    let mut hasher_state: u64 = 0xcbf29ce484222325; // FNV-1a offset basis

    if let Ok(entries) = collect_files_sorted(dir) {
        for entry in &entries {
            // Hash the relative path
            if let Ok(rel) = entry.strip_prefix(dir) {
                for byte in rel.to_string_lossy().as_bytes() {
                    hasher_state ^= *byte as u64;
                    hasher_state = hasher_state.wrapping_mul(0x100000001b3); // FNV prime
                }
            }
            // Hash the file contents
            if let Ok(content) = std::fs::read(entry) {
                for byte in &content {
                    hasher_state ^= *byte as u64;
                    hasher_state = hasher_state.wrapping_mul(0x100000001b3);
                }
            }
        }
    }

    format!("fnv1a:{:016x}", hasher_state)
}

/// Collect all files in a directory tree, sorted by relative path.
fn collect_files_sorted(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut files = Vec::new();
    collect_files_recursive(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files_recursive(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                collect_files_recursive(&path, files)?;
            } else {
                files.push(path);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_provider_resolves_path_dep() {
        let dir = PathBuf::from("/tmp/taida_test_ws_provider");
        let dep_dir = dir.join("mylib");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dep_dir).unwrap();
        std::fs::write(dep_dir.join("main.td"), "// lib").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: dir.clone(),
        };

        let dep = Dependency::Path {
            path: "./mylib".to_string(),
        };
        let provider = WorkspaceProvider;

        assert!(provider.can_resolve(&dep));
        match provider.resolve("mylib", &dep, &manifest) {
            ProviderResult::Resolved(pkg) => {
                assert_eq!(pkg.name, "mylib");
                assert_eq!(pkg.version, "0.0.0-local");
                assert!(matches!(pkg.source, PackageSource::Path(_)));
                assert!(pkg.path.exists());
                assert!(!pkg.integrity.is_empty());
            }
            _ => panic!("Expected Resolved"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_workspace_provider_not_applicable_for_registry() {
        let dep = Dependency::Registry {
            org: "taida-lang".to_string(),
            name: "os".to_string(),
            version: "a.1".to_string(),
        };
        let provider = WorkspaceProvider;
        assert!(!provider.can_resolve(&dep));
    }

    #[test]
    fn test_core_bundled_provider_resolves_os() {
        let dir = PathBuf::from("/tmp/taida_test_bundled_provider");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: dir.clone(),
        };

        let dep = Dependency::Registry {
            org: "taida-lang".to_string(),
            name: "os".to_string(),
            version: "a.1".to_string(),
        };
        let provider = CoreBundledProvider::with_bundled_root(dir.join("bundled"));

        assert!(provider.can_resolve(&dep));
        match provider.resolve("os", &dep, &manifest) {
            ProviderResult::Resolved(pkg) => {
                assert_eq!(pkg.name, "os");
                assert_eq!(pkg.version, "a.1");
                assert_eq!(pkg.source, PackageSource::CoreBundled);
                assert!(pkg.path.exists());
                assert!(pkg.path.join("main.td").exists());
                let source = std::fs::read_to_string(pkg.path.join("main.td")).unwrap();
                assert!(
                    source.contains("ReadAsync"),
                    "os package should export ReadAsync"
                );
                assert!(
                    source.contains("HttpRequest"),
                    "os package should export HttpRequest"
                );
                assert!(
                    source.contains("udpBind"),
                    "os package should export udpBind"
                );
                assert!(
                    source.contains("udpRecvFrom"),
                    "os package should export udpRecvFrom"
                );
                assert!(
                    source.contains("listenerClose"),
                    "os package should export listenerClose"
                );
                assert!(
                    source.contains("tcpAccept"),
                    "os package should export tcpAccept"
                );
                assert!(
                    source.contains("socketSendAll"),
                    "os package should export socketSendAll"
                );
                assert!(
                    source.contains("readBytes"),
                    "os package should export readBytes"
                );
                assert!(
                    source.contains("socketRecvBytes"),
                    "os package should export socketRecvBytes"
                );
                assert!(
                    source.contains("socketRecvExact"),
                    "os package should export socketRecvExact"
                );
                assert!(source.contains("argv"), "os package should export argv");
            }
            other => panic!(
                "Expected Resolved, got: {:?}",
                match other {
                    ProviderResult::Error(e) => e,
                    _ => "NotApplicable".to_string(),
                }
            ),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_core_bundled_provider_version_mismatch() {
        let dir = PathBuf::from("/tmp/taida_test_bundled_version");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: dir.clone(),
        };

        let dep = Dependency::Registry {
            org: "taida-lang".to_string(),
            name: "os".to_string(),
            version: "b.1".to_string(),
        };
        let provider = CoreBundledProvider::with_bundled_root(dir.join("bundled"));

        match provider.resolve("os", &dep, &manifest) {
            ProviderResult::Error(msg) => {
                assert!(msg.contains("b.1"));
                assert!(msg.contains("a.1"));
            }
            _ => panic!("Expected Error for version mismatch"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_store_provider_can_resolve_registry() {
        let dep = Dependency::Registry {
            org: "taida-community".to_string(),
            name: "http".to_string(),
            version: "a.3".to_string(),
        };
        let provider = StoreProvider::new();
        assert!(provider.can_resolve(&dep));
    }

    #[test]
    fn test_store_provider_resolves_cached_package() {
        let dir = PathBuf::from("/tmp/taida_test_store_provider");
        let _ = std::fs::remove_dir_all(&dir);
        let pkg_dir = dir.join("alice").join("http").join("b.12");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("main.td"), "// http lib").unwrap();
        std::fs::write(pkg_dir.join(".taida_installed"), "").unwrap();

        let store = super::super::store::GlobalStore::with_root(dir.clone());
        let provider = StoreProvider::with_store(store);

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: PathBuf::from("/tmp"),
        };

        let dep = Dependency::Registry {
            org: "alice".to_string(),
            name: "http".to_string(),
            version: "b.12".to_string(),
        };

        match provider.resolve("http", &dep, &manifest) {
            ProviderResult::Resolved(pkg) => {
                assert_eq!(pkg.name, "http");
                assert_eq!(pkg.version, "b.12");
                assert!(matches!(pkg.source, PackageSource::Store { .. }));
                assert!(pkg.path.exists());
            }
            ProviderResult::Error(e) => panic!("Expected Resolved, got Error: {}", e),
            _ => panic!("Expected Resolved"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_is_core_bundled() {
        assert!(CoreBundledProvider::is_core_bundled("taida-lang", "os"));
        assert!(CoreBundledProvider::is_core_bundled("taida-lang", "js"));
        assert!(CoreBundledProvider::is_core_bundled("taida-lang", "crypto"));
        assert!(CoreBundledProvider::is_core_bundled("taida-lang", "net"));
        assert!(CoreBundledProvider::is_core_bundled("taida-lang", "pool"));
        assert!(!CoreBundledProvider::is_core_bundled(
            "taida-community",
            "os"
        ));
        assert!(!CoreBundledProvider::is_core_bundled("taida-lang", "http"));
    }

    #[test]
    fn test_core_bundled_provider_resolves_js() {
        let dir = PathBuf::from("/tmp/taida_test_bundled_js");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: dir.clone(),
        };

        let dep = Dependency::Registry {
            org: "taida-lang".to_string(),
            name: "js".to_string(),
            version: "a.1".to_string(),
        };
        let provider = CoreBundledProvider::with_bundled_root(dir.join("bundled"));

        assert!(provider.can_resolve(&dep));
        match provider.resolve("js", &dep, &manifest) {
            ProviderResult::Resolved(pkg) => {
                assert_eq!(pkg.name, "js");
                assert_eq!(pkg.version, "a.1");
                assert_eq!(pkg.source, PackageSource::CoreBundled);
                assert!(pkg.path.exists());
                assert!(pkg.path.join("main.td").exists());
                // Verify the source contains JSNew export
                let source = std::fs::read_to_string(pkg.path.join("main.td")).unwrap();
                assert!(source.contains("JSNew"), "js package should export JSNew");
            }
            other => panic!(
                "Expected Resolved, got: {:?}",
                match other {
                    ProviderResult::Error(e) => e,
                    _ => "NotApplicable".to_string(),
                }
            ),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_core_bundled_provider_resolves_crypto_net_pool() {
        let dir = PathBuf::from("/tmp/taida_test_bundled_more");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: dir.clone(),
        };

        let provider = CoreBundledProvider::with_bundled_root(dir.join("bundled"));
        for (pkg, expected_token) in [
            ("crypto", "sha256"),
            ("net", "dnsResolve"),
            ("pool", "poolAcquire"),
        ] {
            let dep = Dependency::Registry {
                org: "taida-lang".to_string(),
                name: pkg.to_string(),
                version: "a.1".to_string(),
            };

            assert!(
                provider.can_resolve(&dep),
                "provider should resolve {}",
                pkg
            );
            match provider.resolve(pkg, &dep, &manifest) {
                ProviderResult::Resolved(resolved) => {
                    let source = std::fs::read_to_string(resolved.path.join("main.td")).unwrap();
                    assert!(
                        source.contains(expected_token),
                        "{} package should contain {}",
                        pkg,
                        expected_token
                    );
                }
                ProviderResult::Error(e) => {
                    panic!("expected {} to resolve, got error: {}", pkg, e);
                }
                ProviderResult::NotApplicable => {
                    panic!("expected {} to resolve, got NotApplicable", pkg);
                }
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compute_dir_hash_deterministic() {
        let dir = PathBuf::from("/tmp/taida_test_hash");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        std::fs::write(dir.join("b.txt"), "world").unwrap();

        let hash1 = compute_dir_hash(&dir);
        let hash2 = compute_dir_hash(&dir);
        assert_eq!(hash1, hash2, "Hash should be deterministic");
        assert!(hash1.starts_with("fnv1a:"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compute_dir_hash_changes_with_content() {
        let dir = PathBuf::from("/tmp/taida_test_hash_change");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();

        let hash1 = compute_dir_hash(&dir);

        // Add a file -> hash should change
        std::fs::write(dir.join("b.txt"), "world").unwrap();
        let hash2 = compute_dir_hash(&dir);

        assert_ne!(hash1, hash2, "Hash should change when files are added");

        // Modify file content (same size) -> hash should change
        std::fs::write(dir.join("a.txt"), "hullo").unwrap();
        let hash3 = compute_dir_hash(&dir);
        assert_ne!(hash2, hash3, "Hash should change when file content changes");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
