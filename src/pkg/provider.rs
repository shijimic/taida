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
//   readBytes(path)                      -- read file as Bytes (64MB limit)
//   readBytesAt(path, offset, len)       -- chunked read (C26B-020 柱 1)
//   writeBytes(path, content)            -- write Bytes payload to file
//
// Side-effect APIs (functions -> Result):
//   writeFile(path, content)    -- write file (create or overwrite)
//   appendFile(path, content)   -- append to file
//   remove(path)                -- remove file/directory
//   createDir(path)             -- mkdir -p
//   rename(from, to)            -- move/rename (atomic)
//
// Process APIs (functions -> Gorillax):
//   run(program, args)          -- direct exec (safe, no shell) — captures stdout/stderr
//   execShell(command)          -- shell exec (pipes, redirects) — captures stdout/stderr
//     WARNING: Shell injection risk. Prefer run() for safety.
//
// Interactive process APIs (functions -> Gorillax[@(code: Int)], C19):
//   runInteractive(program, args)  -- TTY passthrough, child inherits parent's stdin/stdout/stderr
//   execShellInteractive(command)  -- TTY passthrough via `sh -c` (POSIX) / `cmd /C` (Windows)
//     NOTE: stdout / stderr are NOT captured — only the exit code is observable.
//     Intended for TUI apps (nvim, less, fzf, git commit). If you need to
//     inspect output programmatically, use the captured `run` / `execShell`.
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

<<< @(Read, ListDir, Stat, Exists, readBytes, readBytesAt, writeFile, writeBytes, appendFile, remove, createDir, rename, run, execShell, runInteractive, execShellInteractive, EnvVar, allEnv, argv, ReadAsync, HttpGet, HttpPost, HttpRequest, tcpConnect, tcpListen, tcpAccept, socketSend, socketSendAll, socketRecv, socketSendBytes, socketRecvBytes, socketRecvExact, udpBind, udpSendTo, udpRecvFrom, socketClose, listenerClose, udpClose)
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
// HTTP server/runtime surface only.
// Low-level socket / DNS APIs live in taida-lang/os.
//
// HTTP surface:
//   httpServe, httpParseRequestHead, httpEncodeResponse, readBody
//   startResponse, writeChunk, endResponse, sseEvent
//   readBodyChunk, readBodyAll
//   wsUpgrade, wsSend, wsReceive, wsClose, wsCloseCode
//   HttpProtocol
//
// TI-21 contract notes:
//   TLS verification on Http* uses backend default trust store (no insecure -k path)
//   Protocol/runtime details remain behind httpServe contract
//   Legacy tcp*/udp*/dnsResolve re-exports were removed after HTTP/3 package freeze

Enum => HttpProtocol = :H1 :H2 :H3

<<< @(httpServe, httpParseRequestHead, httpEncodeResponse, readBody, startResponse, writeChunk, endResponse, sseEvent, readBodyChunk, readBodyAll, wsUpgrade, wsSend, wsReceive, wsClose, wsCloseCode, HttpProtocol)
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
        let source = match pkg_name {
            "os" => Self::os_package_source(),
            "js" => Self::js_package_source(),
            "crypto" => Self::crypto_package_source(),
            "net" => Self::net_package_source(),
            "pool" => Self::pool_package_source(),
            _ => "// Unknown core-bundled package\n",
        };
        let needs_write = match std::fs::read_to_string(&main_td) {
            Ok(existing) => existing != source,
            Err(_) => true,
        };
        if needs_write {
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
///
/// C17-2: consults a stale-detection decision table before reusing a
/// cached entry. See `store::classify_stale` and
/// `.dev/C17_IMPL_SPEC.md` Phase 2.
pub struct StoreProvider {
    store: GlobalStore,
    /// When true, bypass local cache for generation resolution (used by `taida update`).
    force_remote: bool,
    /// C17-2 / C17-4: when true, invalidate any cached entry and re-extract
    /// unconditionally. Skips the decision table.
    force_refresh: bool,
    /// C17-2: when true, skip the remote HEAD lookup entirely -- the
    /// decision table is evaluated with `remote_sha = None` so sidecar
    /// presence alone governs skip/warn. Mutually exclusive with
    /// `force_refresh`; the CLI rejects the combination up front.
    no_remote_check: bool,
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
            force_refresh: false,
            no_remote_check: false,
        }
    }

    /// Create a StoreProvider that bypasses local cache for generation resolution.
    /// Used by `taida update` to always query GitHub for the latest version.
    pub fn new_force_remote() -> Self {
        StoreProvider {
            store: GlobalStore::new(),
            force_remote: true,
            force_refresh: false,
            no_remote_check: false,
        }
    }

    /// C17-2 / C17-4: configure the store-side refresh behaviour.
    ///
    /// Panics if both `force_refresh` and `no_remote_check` are set --
    /// the CLI should reject the combination at argument parsing time.
    pub fn with_refresh_flags(mut self, force_refresh: bool, no_remote_check: bool) -> Self {
        assert!(
            !(force_refresh && no_remote_check),
            "StoreProvider: --force-refresh and --no-remote-check are mutually exclusive"
        );
        self.force_refresh = force_refresh;
        self.no_remote_check = no_remote_check;
        self
    }

    #[cfg(test)]
    pub fn with_store(store: GlobalStore) -> Self {
        StoreProvider {
            store,
            force_remote: false,
            force_refresh: false,
            no_remote_check: false,
        }
    }

    /// C17-2: consult the decision table for a cached package.
    ///
    /// Returns a `StaleOutcome` telling the caller (`resolve`) what to do:
    /// - `Skip` -> fast-path / offline-warning / strong-warning / no-remote-check
    ///   skip. The caller does not touch the cache.
    /// - `Refresh { sha }` -> the caller must stage-swap the existing
    ///   package directory out of the way, call `fetch_and_cache_with_meta`
    ///   with this SHA, and then commit or rollback the swap depending on
    ///   whether the fetch succeeded. `sha` is `None` when the remote lookup
    ///   failed but a refresh was still requested (e.g. `--force-refresh`
    ///   without online access).
    ///
    /// C17B-001: This function intentionally does NOT call
    /// `invalidate_package` any more. The old contract deleted `pkg_dir`
    /// unconditionally before the fetch, so a failed fetch destroyed the
    /// user's working install. The stage-swap lives in `resolve()`.
    ///
    /// Side effects:
    /// - stderr warning on rows 4 and 5 (`offline, cannot verify
    ///   staleness` / `unknown provenance, use --force-refresh`).
    /// - stderr info on a refresh (`remote moved: ...; refreshing store`).
    ///
    /// Never silent: every non-happy-path outcome emits stderr output so
    /// the user can see what happened.
    fn apply_stale_decision(
        &self,
        org: &str,
        name: &str,
        version: &str,
    ) -> Result<StaleOutcome, String> {
        use super::store::{
            RefreshReason, StaleDecision, classify_stale, refresh_reason_short,
            resolve_version_to_sha,
        };

        // Force-refresh short-circuits the table: stage the existing
        // directory out of the way so the fetch path can re-extract.
        // Phase 4 reuses this branch.
        //
        // C17B-001: `invalidate_package` used to delete `pkg_dir` outright.
        // If the subsequent fetch failed (offline, rate-limited, 429...)
        // the user's working install would be gone. The backup-swap lives
        // in `resolve()` proper: it watches for the `refresh_triggered`
        // signal and performs the rename + rollback there. Here we only
        // resolve the remote SHA so it can be written into the new sidecar.
        //
        // C17B-007: `--force-refresh` and `--no-remote-check` are mutually
        // exclusive at construction (`with_refresh_flags` and the CLI
        // argparser reject the combination). The branch that checked
        // `self.no_remote_check` here was therefore dead; we now call
        // `resolve_version_to_sha` unconditionally. If it fails we fall
        // through to `None` and the sidecar is written with `commit_sha=""`
        // so the next install's row 2b path fills it in.
        if self.force_refresh {
            let sha = resolve_version_to_sha(org, name, version).ok().flatten();
            eprintln!(
                "  refreshing store for {}/{}@{} (--force-refresh)",
                org, name, version
            );
            return Ok(StaleOutcome::Refresh { sha });
        }

        let sidecar = match self.store.read_package_meta(org, name, version) {
            Ok(meta) => meta,
            Err(e) => {
                // Malformed or schema-mismatched sidecar: treat as missing
                // and pessimistically refresh. Warn so the operator sees
                // it.
                //
                // C17B-019: schema mismatches (older taida reading a newer
                // sidecar, or vice versa) are unrecoverable without a
                // version change. Surface a hint so the user knows what to
                // do.
                eprintln!(
                    "  sidecar for {}/{}@{} unreadable ({}); re-extracting",
                    org, name, version, e
                );
                if matches!(e, super::store::StoreError::UnknownMetaSchema { .. }) {
                    eprintln!(
                        "  hint: this sidecar was written by a different taida \
                         version. Upgrade taida or pin {}/{}@{} to a version \
                         published by a compatible taida release.",
                        org, name, version
                    );
                }
                None
            }
        };

        // `--no-remote-check` short-circuit: the user has told us not to
        // reach out. When a sidecar is present the skip is silent (the
        // user asked for this); when it is absent we still print the
        // strong warning so the user knows provenance is unverified.
        if self.no_remote_check {
            if sidecar.is_none() {
                eprintln!(
                    "  unknown provenance: {}/{}@{} (sidecar missing; --no-remote-check). \
                     Re-run with --force-refresh when online.",
                    org, name, version
                );
            }
            return Ok(StaleOutcome::Skip);
        }

        let remote_sha = match resolve_version_to_sha(org, name, version) {
            Ok(sha) => sha,
            Err(e) => {
                // Malformed response (distinct from "no network"):
                // warn but continue as if offline.
                eprintln!(
                    "  warning: could not verify {}/{}@{} staleness: {}",
                    org, name, version, e
                );
                None
            }
        };

        let decision = classify_stale(sidecar.as_ref(), remote_sha.as_deref());

        match decision {
            StaleDecision::SkipFastPath => Ok(StaleOutcome::Skip),

            StaleDecision::SkipWithOfflineWarning => {
                eprintln!(
                    "  offline, cannot verify staleness: {}/{}@{} (using cached entry)",
                    org, name, version
                );
                Ok(StaleOutcome::Skip)
            }

            StaleDecision::SkipUnknownProvenanceStrongWarn => {
                eprintln!(
                    "  unknown provenance: {}/{}@{} (sidecar missing). \
                     Re-run with --force-refresh when online.",
                    org, name, version
                );
                Ok(StaleOutcome::Skip)
            }

            StaleDecision::Refresh(reason) => {
                // C17B-010: The three `RefreshReason` variants all emit the
                // same message template. Format once, print once. The
                // variant-specific wording is produced by
                // `refresh_reason_short`.
                //
                // C17B-016: For `SidecarShaUnknown` (row 2b -- the second
                // install after a Phase 1 sidecar was written without a
                // resolved SHA) we add a parenthetical that explains the
                // refresh is filling in the missing SHA. This demystifies
                // the re-download a new user sees on their second install.
                let extra_hint = match &reason {
                    RefreshReason::SidecarShaUnknown => " (filling in missing sidecar SHA)",
                    _ => "",
                };
                eprintln!(
                    "  {}/{}@{}: {}; refreshing store{}",
                    org,
                    name,
                    version,
                    refresh_reason_short(&reason),
                    extra_hint
                );
                // C17B-001: do NOT call `invalidate_package` here. The
                // caller (`StoreProvider::resolve`) stages a backup via
                // `GlobalStore::stage_invalidation` /
                // `commit_invalidation` / `rollback_invalidation` so a
                // failed fetch rolls back the user's previous working
                // install instead of leaving them with an empty directory.
                // The `Ok(StaleOutcome::Refresh { sha })` return tells the
                // caller to perform the swap.
                Ok(StaleOutcome::Refresh { sha: remote_sha })
            }
        }
    }
}

/// C17B-001: outcome of the Phase 2 stale-detection decision table, as
/// consumed by `StoreProvider::resolve`.
///
/// - `Skip`: the cached entry is trustworthy (fast-path / offline-warned /
///   strong-warned / no-remote-check). The caller does NOT touch the store.
/// - `Refresh { sha }`: the caller must stage-swap the directory, call the
///   fetcher, and commit or rollback depending on the fetch result. `sha`
///   is the remote commit SHA to record in the new sidecar (`None` when
///   the lookup failed but a refresh was still requested, e.g. under
///   `--force-refresh` while offline; the sidecar will carry `commit_sha=""`
///   and the next install will upgrade it via row 2b).
#[derive(Debug)]
enum StaleOutcome {
    Skip,
    Refresh { sha: Option<String> },
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

                // C17B-009: take a per-package advisory lock so two
                // concurrent `taida install` processes do not clobber each
                // other's extract. The lock is held for the duration of the
                // decision table + fetch + commit.
                let _lock_guard = match self.store.acquire_install_lock(org, name, &exact_version) {
                    Ok(g) => g,
                    Err(e) => return ProviderResult::Error(e),
                };

                // C17-2: stale-detection decision table.
                //
                // Only engaged when the package is already cached -- an
                // uncached install always falls through to
                // `fetch_and_cache_with_meta` below, which is the natural
                // "first install" path and is unchanged from C17-1.
                //
                // C17B-001: when the outcome is `Refresh`, we stage the
                // existing directory aside (rename to `<dir>.refresh-staging`)
                // BEFORE calling the fetcher. If the fetch fails we
                // restore the backup so the user's working install is
                // preserved. On success we drop the backup.
                let (remote_sha_for_sidecar, stash) =
                    if self.store.is_cached(org, name, &exact_version) {
                        match self.apply_stale_decision(org, name, &exact_version) {
                            Err(msg) => return ProviderResult::Error(msg),
                            Ok(StaleOutcome::Skip) => (None, None),
                            Ok(StaleOutcome::Refresh { sha }) => {
                                match self.store.stage_invalidation(org, name, &exact_version) {
                                    Ok(stash) => (sha, stash),
                                    Err(e) => return ProviderResult::Error(e),
                                }
                            }
                        }
                    } else {
                        // Uncached: do not do an extra remote round-trip here.
                        // `fetch_and_cache_with_meta` will record the SHA we
                        // pass in, but for the first install we leave it
                        // `None` -- the next `taida install` will detect the
                        // missing SHA and do a pessimistic refresh that fills
                        // it in. This keeps the first-install UX unchanged.
                        (None, None)
                    };

                let fetch_result = self.store.fetch_and_cache_with_meta(
                    org,
                    name,
                    &exact_version,
                    remote_sha_for_sidecar.as_deref(),
                );

                match fetch_result {
                    Ok(path) => {
                        // Fetch succeeded; drop the backup (if any).
                        if let Some(stash_path) = stash
                            && let Err(e) = self.store.commit_invalidation(&stash_path)
                        {
                            // Non-fatal: the new install is already in
                            // place, the stash is just leftover disk
                            // space. Warn the user once.
                            eprintln!(
                                "  warning: could not remove refresh backup '{}': {}",
                                stash_path.display(),
                                e
                            );
                        }
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
                    Err(e) => {
                        // C17B-001: fetch failed. Roll back the stash so
                        // the user's previous install reappears instead of
                        // being silently lost.
                        if let Some(stash_path) = stash {
                            let pkg_dir = self.store.package_path(org, name, &exact_version);
                            match self.store.rollback_invalidation(&stash_path, &pkg_dir) {
                                Ok(()) => {
                                    eprintln!(
                                        "  refresh of {}/{}@{} failed ({}); \
                                         restored previous install from backup",
                                        org, name, &exact_version, e
                                    );
                                }
                                Err(rb_err) => {
                                    eprintln!(
                                        "  refresh of {}/{}@{} failed ({}); \
                                         rollback also failed ({}). Previous \
                                         install may be in '{}'",
                                        org,
                                        name,
                                        &exact_version,
                                        e,
                                        rb_err,
                                        stash_path.display()
                                    );
                                }
                            }
                        }
                        ProviderResult::Error(e)
                    }
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
            // C17B-002 + HOLD M2 fix (2026-04-17): skip taida-managed metadata
            // **files** that would otherwise make the directory hash churn
            // between installs even though the package content is unchanged.
            //
            // Skipped files:
            //   - `_meta.toml`        -- C17 sidecar (contains `fetched_at`)
            //   - `.taida_installed`  -- completion marker
            //   - `.*.tmp`            -- temp files from atomic sidecar writes
            //                           (e.g. `._meta.toml.tmp` after crash)
            //
            // These files are Taida's provenance/management metadata, not
            // part of the package's own content. Including them would cause
            // `.taida/taida.lock` integrity hashes to drift every time the
            // sidecar is rewritten, breaking lockfile reproducibility.
            //
            // The filter is applied **inside** the `is_file()` branch so a
            // directory whose name happens to match (the `_meta.toml` edge
            // case) still has its contents traversed. Pre-HOLD the filter
            // ran before the is_file/is_dir split, which would have skipped
            // entire subtrees whose directory name collided with these
            // sentinel filenames.
            if path.is_file() {
                if let Some(fname) = path.file_name().and_then(|n| n.to_str())
                    && (fname == crate::pkg::store::STORE_META_FILENAME
                        || fname == ".taida_installed"
                        || (fname.starts_with('.') && fname.ends_with(".tmp")))
                {
                    continue;
                }
                files.push(path);
            } else if path.is_dir() {
                collect_files_recursive(&path, files)?;
            } else {
                // C18B-009 fix (carry from C17B-022): anything that is
                // neither a regular file nor a directory — broken
                // symlink, socket, FIFO, device node, etc. — used to
                // be silently dropped from the integrity-hash walk.
                // That silently diverged the hash from the actual
                // on-disk state and prevented downstream cache
                // invalidation. Emit a `warning:` line to stderr so
                // the drop is visible; the hash still excludes the
                // entry because there is no stable content to read.
                eprintln!(
                    "warning: skipping non-regular entry during integrity walk: {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // C18B-009 regression: `collect_files_recursive` must still
    // enumerate real files when broken symlinks are present in the
    // traversal root. The symlink target is deliberately missing so
    // the entry's `is_file()` is false, forcing the non-file branch.
    //
    // Platform note: `std::os::unix::fs::symlink` is only available
    // on Unix. On non-Unix targets this test is compiled out; the
    // regression surface (package tarballs produced on Unix CI) is
    // fully covered on those targets.
    #[cfg(unix)]
    #[test]
    fn test_collect_files_recursive_skips_broken_symlink_but_reports() {
        use std::os::unix::fs::symlink;

        let dir = PathBuf::from("/tmp/taida_test_c18b_009_broken_symlink");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Real regular file that MUST be enumerated.
        std::fs::write(dir.join("real.txt"), b"hello").unwrap();
        // Broken symlink pointing at a non-existent target. Before
        // the C18B-009 fix this entry was silently skipped; after the
        // fix it is still skipped (there is no content to hash) but
        // the skip emits a warning. We don't assert on stderr here
        // because `eprintln!` is swallowed by cargo's default test
        // harness — the behavioural pin is "real files still get
        // enumerated, traversal doesn't error out".
        symlink(dir.join("__does_not_exist__"), dir.join("dangling.lnk")).unwrap();

        let mut files = Vec::new();
        let result = collect_files_recursive(&dir, &mut files);
        assert!(
            result.is_ok(),
            "broken-symlink traversal must not return Err"
        );
        // The regular file must be present. The broken symlink must
        // not poison the Vec with a bogus path that read_to_string
        // would later fail on.
        let names: Vec<String> = files
            .iter()
            .filter_map(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert!(
            names.contains(&"real.txt".to_string()),
            "real file must be enumerated; got: {:?}",
            names
        );
        assert!(
            !names.contains(&"dangling.lnk".to_string()),
            "broken symlink must not be enumerated (no stable content); got: {:?}",
            names
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

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
            exports: Vec::new(),
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
            exports: Vec::new(),
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
            exports: Vec::new(),
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

    // ==========================================================================
    // C17-2 / C17-4: StoreProvider refresh flag integration
    // ==========================================================================

    #[test]
    fn test_store_provider_with_refresh_flags_mutual_exclusion_panics() {
        let result =
            std::panic::catch_unwind(|| StoreProvider::new().with_refresh_flags(true, true));
        assert!(
            result.is_err(),
            "force_refresh + no_remote_check must panic at construction"
        );
    }

    #[test]
    fn test_store_provider_force_refresh_returns_refresh_outcome() {
        // C17B-001 / C17B-007 contract: with force_refresh=true,
        // apply_stale_decision must return `StaleOutcome::Refresh` so
        // `resolve()` can perform the stage-swap. It must NOT call
        // invalidate_package itself -- that would regress the
        // "fetch fails -> user loses install" data-loss bug.
        //
        // We point the GitHub API at a closed port to exercise the offline
        // path: the remote SHA lookup fails, so the returned
        // `StaleOutcome::Refresh.sha` is `None`. The cached directory is
        // still present (no invalidation here); the caller owns the swap.
        let _guard = crate::util::env_test_lock().lock().unwrap();
        let dir = PathBuf::from("/tmp/taida_test_force_refresh_outcome");
        let _ = std::fs::remove_dir_all(&dir);
        let pkg_dir = dir.join("alice").join("http").join("b.12");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join(".taida_installed"), "").unwrap();
        let sidecar = super::super::store::StoreMeta {
            schema_version: super::super::store::STORE_META_SCHEMA_VERSION,
            commit_sha: "oldsha".to_string(),
            tarball_sha256: "abc".to_string(),
            tarball_etag: None,
            fetched_at: "2026-04-16T00:00:00Z".to_string(),
            source: "github:alice/http".to_string(),
            version: "b.12".to_string(),
        };
        super::super::store::write_meta_atomic(
            &super::super::store::meta_path_for(&pkg_dir),
            &sidecar,
        )
        .unwrap();

        let store = super::super::store::GlobalStore::with_root(dir.clone());
        let provider = StoreProvider::with_store(store).with_refresh_flags(true, false);

        let prev = std::env::var("TAIDA_GITHUB_API_URL").ok();
        unsafe {
            std::env::set_var("TAIDA_GITHUB_API_URL", "http://127.0.0.1:1");
        }

        let outcome = provider
            .apply_stale_decision("alice", "http", "b.12")
            .expect("apply_stale_decision returns Ok under force_refresh");
        match outcome {
            StaleOutcome::Refresh { sha } => {
                // Remote SHA lookup failed (closed port) -> None.
                assert!(
                    sha.is_none(),
                    "offline force-refresh: sha must be None (got {:?})",
                    sha
                );
            }
            StaleOutcome::Skip => panic!("force_refresh must return Refresh, not Skip"),
        }

        // C17B-001 verification: the cached sidecar / marker must still
        // exist. Invalidation is the caller's job (in `resolve()`), not
        // `apply_stale_decision`'s. This is the anti-data-loss contract.
        let meta_path = super::super::store::meta_path_for(&pkg_dir);
        assert!(
            meta_path.exists(),
            "apply_stale_decision must not delete the old sidecar under force_refresh"
        );
        let m = super::super::store::read_meta(&meta_path).unwrap().unwrap();
        assert_eq!(
            m.commit_sha, "oldsha",
            "old sidecar content must be untouched by apply_stale_decision"
        );

        unsafe {
            match prev {
                Some(v) => std::env::set_var("TAIDA_GITHUB_API_URL", v),
                None => std::env::remove_var("TAIDA_GITHUB_API_URL"),
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_store_provider_force_refresh_rollback_on_fetch_failure() {
        // C17B-001: the full `resolve()` path must restore the previous
        // working install when the fetch fails. We simulate that by
        // pointing BOTH the archive base URL and the API URL at closed
        // ports, so the fetch errors out. The user's existing extracted
        // directory (.taida_installed marker + sidecar) must reappear.
        let _guard = crate::util::env_test_lock().lock().unwrap();
        let dir = PathBuf::from("/tmp/taida_test_force_refresh_rollback");
        let _ = std::fs::remove_dir_all(&dir);
        let pkg_dir = dir.join("alice").join("http").join("b.12");
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(pkg_dir.join("main.td"), "// original content").unwrap();
        std::fs::write(pkg_dir.join(".taida_installed"), "").unwrap();
        let sidecar = super::super::store::StoreMeta {
            schema_version: super::super::store::STORE_META_SCHEMA_VERSION,
            commit_sha: "oldsha".to_string(),
            tarball_sha256: "abc".to_string(),
            tarball_etag: None,
            fetched_at: "2026-04-16T00:00:00Z".to_string(),
            source: "github:alice/http".to_string(),
            version: "b.12".to_string(),
        };
        super::super::store::write_meta_atomic(
            &super::super::store::meta_path_for(&pkg_dir),
            &sidecar,
        )
        .unwrap();

        let store = super::super::store::GlobalStore::with_root(dir.clone());
        let provider = StoreProvider::with_store(store).with_refresh_flags(true, false);

        let prev_api = std::env::var("TAIDA_GITHUB_API_URL").ok();
        let prev_base = std::env::var("TAIDA_GITHUB_BASE_URL").ok();
        unsafe {
            std::env::set_var("TAIDA_GITHUB_API_URL", "http://127.0.0.1:1");
            std::env::set_var("TAIDA_GITHUB_BASE_URL", "http://127.0.0.1:1");
        }

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: PathBuf::from("/tmp"),
            exports: Vec::new(),
        };
        let dep = Dependency::Registry {
            org: "alice".to_string(),
            name: "http".to_string(),
            version: "b.12".to_string(),
        };
        let result = provider.resolve("http", &dep, &manifest);
        unsafe {
            match prev_api {
                Some(v) => std::env::set_var("TAIDA_GITHUB_API_URL", v),
                None => std::env::remove_var("TAIDA_GITHUB_API_URL"),
            }
            match prev_base {
                Some(v) => std::env::set_var("TAIDA_GITHUB_BASE_URL", v),
                None => std::env::remove_var("TAIDA_GITHUB_BASE_URL"),
            }
        }

        // The fetch must fail (offline mock).
        assert!(
            matches!(result, ProviderResult::Error(_)),
            "offline fetch must error, got: {:?}",
            match result {
                ProviderResult::Error(_) => "Error",
                ProviderResult::Resolved(_) => "Resolved",
                ProviderResult::NotApplicable => "NotApplicable",
            }
        );

        // C17B-001: the user's previous install must be restored.
        assert!(
            pkg_dir.join(".taida_installed").exists(),
            "rollback must restore .taida_installed marker"
        );
        assert!(
            pkg_dir.join("main.td").exists(),
            "rollback must restore original file content"
        );
        assert_eq!(
            std::fs::read_to_string(pkg_dir.join("main.td")).unwrap(),
            "// original content",
            "rollback must restore the exact previous content"
        );
        let meta = super::super::store::read_meta(&super::super::store::meta_path_for(&pkg_dir))
            .unwrap()
            .unwrap();
        assert_eq!(
            meta.commit_sha, "oldsha",
            "rollback must restore the old sidecar (commit_sha=oldsha)"
        );

        // The staging dir must also be cleaned up.
        let parent = dir.join("alice").join("http");
        let staging_count = std::fs::read_dir(&parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".refresh-staging"))
            .count();
        assert_eq!(
            staging_count, 0,
            "rollback must remove the staging directory"
        );

        let _ = std::fs::remove_dir_all(&dir);
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
            exports: Vec::new(),
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
            exports: Vec::new(),
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
            exports: Vec::new(),
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

    #[test]
    fn test_compute_dir_hash_ignores_taida_managed_metadata() {
        // C17B-002 regression guard: `_meta.toml`, `.taida_installed`,
        // and `.foo.tmp` files must not contribute to the directory hash.
        // These are Taida-managed sidecar / scratch files that change
        // every install even when package content is stable.
        let dir = PathBuf::from("/tmp/taida_test_hash_c17b_002");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.td"), "stdout(\"hello\")\n").unwrap();
        std::fs::write(dir.join("packages.tdm"), "<<<@a.1 x/y\n").unwrap();

        let baseline = compute_dir_hash(&dir);

        // Add `_meta.toml` with a timestamp -- must not change hash.
        std::fs::write(
            dir.join("_meta.toml"),
            "schema_version = 1\nfetched_at = \"2026-01-01T00:00:00Z\"\n",
        )
        .unwrap();
        let with_meta = compute_dir_hash(&dir);
        assert_eq!(
            baseline, with_meta,
            "_meta.toml must NOT affect integrity hash (C17B-002)"
        );

        // Change sidecar content -> still same hash.
        std::fs::write(
            dir.join("_meta.toml"),
            "schema_version = 1\nfetched_at = \"2026-12-31T23:59:59Z\"\n",
        )
        .unwrap();
        let with_meta2 = compute_dir_hash(&dir);
        assert_eq!(
            baseline, with_meta2,
            "mutating _meta.toml must NOT change integrity (C17B-002)"
        );

        // Add `.taida_installed` marker -> still same.
        std::fs::write(dir.join(".taida_installed"), "").unwrap();
        let with_marker = compute_dir_hash(&dir);
        assert_eq!(
            baseline, with_marker,
            ".taida_installed marker must NOT affect integrity (C17B-002)"
        );

        // Add a `.foo.tmp` scratch file -> still same.
        std::fs::write(dir.join("._meta.toml.tmp"), "stale").unwrap();
        let with_tmp = compute_dir_hash(&dir);
        assert_eq!(
            baseline, with_tmp,
            ".foo.tmp scratch must NOT affect integrity (C17B-002)"
        );

        // Sanity: a *real* content change still flips the hash.
        std::fs::write(dir.join("main.td"), "stdout(\"changed\")\n").unwrap();
        let changed = compute_dir_hash(&dir);
        assert_ne!(
            baseline, changed,
            "actual content change must flip hash even with metadata filters"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_compute_dir_hash_handles_meta_toml_directory_edge_case() {
        // HOLD M2 fix (2026-04-17) regression guard: the metadata filter
        // must only apply to regular files. A **directory** named
        // `_meta.toml` (or `.taida_installed` / `._meta.toml.tmp`) is an
        // edge case that does not trigger any of the filter predicates;
        // its contents must still be hashed. Pre-fix the filter ran
        // before the is_file/is_dir split and would have skipped the
        // entire subtree.
        let dir = PathBuf::from("/tmp/taida_test_hash_m2_edgecase");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.td"), "stdout(\"hi\")\n").unwrap();

        // Put a real content file inside a directory whose name collides
        // with the sidecar filename.
        let quirky = dir.join("_meta.toml");
        std::fs::create_dir_all(&quirky).unwrap();
        std::fs::write(quirky.join("inner.td"), "x = 1\n").unwrap();

        let h1 = compute_dir_hash(&dir);

        // Mutating a file inside the `_meta.toml/` directory MUST flip
        // the hash, proving the subtree was traversed.
        std::fs::write(quirky.join("inner.td"), "x = 2\n").unwrap();
        let h2 = compute_dir_hash(&dir);
        assert_ne!(
            h1, h2,
            "content inside a dir named _meta.toml must still affect hash (HOLD M2)"
        );

        // Adding a sibling inside the quirky dir also flips.
        std::fs::write(quirky.join("extra.td"), "y = 3\n").unwrap();
        let h3 = compute_dir_hash(&dir);
        assert_ne!(
            h2, h3,
            "new file inside dir named _meta.toml must flip hash (HOLD M2)"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
