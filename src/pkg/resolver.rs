/// Common Package Resolver for Taida Lang.
///
/// Resolves all dependencies using the Provider pattern:
/// 1. WorkspaceProvider — local path dependencies
/// 2. CoreBundledProvider — core packages bundled with Taida (`taida-lang/*`)
/// 3. StoreProvider — external registry (stub, Phase 3+)
///
/// `taida ingot deps` and `taida ingot install` read `packages.tdm`, resolve all dependencies
/// through the provider chain, and create `.taida/deps/` symlinks.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::lockfile::Lockfile;
use super::manifest::{Dependency, Manifest};
use super::provider::{
    CoreBundledProvider, PackageProvider, PackageSource, ProviderResult, ResolvedPackage,
    StoreProvider, WorkspaceProvider,
};

// RC1.5: addon prebuild integration
use crate::addon::host_target;
use crate::addon::manifest::parse_addon_manifest;
use crate::addon::prebuild_fetcher::{FetchError, fetch_prebuild_with_progress};

/// Result of dependency resolution.
#[derive(Debug)]
pub struct ResolveResult {
    /// Successfully resolved dependencies: name -> absolute path.
    pub resolved: BTreeMap<String, PathBuf>,
    /// All resolved packages (for lockfile generation).
    pub packages: Vec<ResolvedPackage>,
    /// Errors encountered during resolution.
    pub errors: Vec<String>,
}

/// C17-2: refresh-related flags forwarded from the CLI to the StoreProvider.
#[derive(Debug, Clone, Copy, Default)]
pub struct StoreRefreshFlags {
    /// `--force-refresh`: invalidate the store entry and re-extract.
    pub force_refresh: bool,
    /// `--no-remote-check`: skip the remote HEAD lookup; sidecar presence
    /// alone governs skip/warn.
    pub no_remote_check: bool,
}

impl StoreRefreshFlags {
    /// Validate CLI-level mutual exclusion. The CLI is expected to reject
    /// the combination before constructing this struct -- this helper
    /// exists so callers (and tests) can double-check.
    pub fn validate(&self) -> Result<(), String> {
        if self.force_refresh && self.no_remote_check {
            Err("--force-refresh and --no-remote-check cannot be combined \
                 (force-refresh requires the remote lookup to be available \
                 for the sidecar it rewrites)"
                .to_string())
        } else {
            Ok(())
        }
    }
}

/// Resolve all dependencies declared in the manifest using the provider chain.
///
/// Tries each provider in order for each dependency:
/// WorkspaceProvider -> CoreBundledProvider -> StoreProvider
pub fn resolve_deps(manifest: &Manifest) -> ResolveResult {
    resolve_deps_inner(manifest, false, None, StoreRefreshFlags::default())
}

/// Resolve all dependencies using the provider chain, but pin generation-only
/// versions to their locked exact versions from an existing lockfile.
///
/// Used by `taida ingot install` to ensure reproducible installs: if a lockfile
/// records `alice/demo@a.2`, a manifest dependency of `alice/demo@a` will
/// resolve to `a.2` instead of the latest version in generation `a`.
pub fn resolve_deps_locked(manifest: &Manifest, lockfile: &Lockfile) -> ResolveResult {
    resolve_deps_inner(
        manifest,
        false,
        Some(lockfile),
        StoreRefreshFlags::default(),
    )
}

/// C17-2: variant of `resolve_deps_locked` that forwards the C17 refresh
/// flags to the `StoreProvider` (force-refresh / no-remote-check).
pub fn resolve_deps_locked_with_flags(
    manifest: &Manifest,
    lockfile: &Lockfile,
    flags: StoreRefreshFlags,
) -> ResolveResult {
    resolve_deps_inner(manifest, false, Some(lockfile), flags)
}

/// C17-2: variant of `resolve_deps` that forwards the C17 refresh flags
/// to the `StoreProvider` when there is no prior lockfile.
pub fn resolve_deps_with_flags(manifest: &Manifest, flags: StoreRefreshFlags) -> ResolveResult {
    resolve_deps_inner(manifest, false, None, flags)
}

/// Resolve all dependencies, bypassing local cache for generation resolution.
///
/// Used by `taida ingot update` to re-resolve generation-only versions (e.g. "a")
/// to the latest exact version (e.g. "a.47") by querying GitHub API directly.
pub fn resolve_deps_update(manifest: &Manifest) -> ResolveResult {
    resolve_deps_inner(manifest, true, None, StoreRefreshFlags::default())
}

fn dep_decl_identity(dep: &Dependency, declared_from_root: &Path) -> String {
    match dep {
        Dependency::Path { path } => {
            let joined = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                declared_from_root.join(path)
            };
            // N-50: canonicalize normalizes `..` segments and symlinks for
            // identity comparison. Falls back to the joined path when the
            // target does not yet exist (e.g. first install). Path traversal
            // is not a practical risk because package resolution starts from
            // the project root and local path dependencies are author-controlled.
            let canonical = joined.canonicalize().unwrap_or(joined);
            format!("path:{}", canonical.display())
        }
        Dependency::Registry { org, name, version } => {
            format!("registry:{}/{}@{}", org, name, version)
        }
    }
}

fn resolved_identity(pkg: &ResolvedPackage) -> String {
    match &pkg.source {
        PackageSource::Path(path) => format!("path:{}@{}", path, pkg.path.display()),
        PackageSource::CoreBundled => format!("bundled:{}@{}", pkg.name, pkg.version),
        PackageSource::Store { org, name } => format!("store:{}/{}@{}", org, name, pkg.version),
    }
}

/// Try to resolve a dependency by iterating through providers.
///
/// On success, registers the resolved package in all tracking maps and enqueues
/// transitive dependencies. Returns `true` if a provider handled the dependency
/// (either resolved or errored), `false` if no provider could resolve it.
#[allow(clippy::too_many_arguments)]
fn try_resolve_with_providers(
    providers: &[Box<dyn PackageProvider>],
    alias_key: &str,
    dep: &Dependency,
    manifest: &Manifest,
    requested_identity: &str,
    resolved_by_alias: &mut BTreeMap<String, String>,
    resolved: &mut BTreeMap<String, PathBuf>,
    packages: &mut Vec<ResolvedPackage>,
    resolved_by_identity: &mut BTreeMap<String, String>,
    queue: &mut std::collections::VecDeque<(String, super::manifest::Dependency, String, PathBuf)>,
    errors: &mut Vec<String>,
) -> bool {
    for provider in providers {
        if !provider.can_resolve(dep) {
            continue;
        }
        match provider.resolve(alias_key, dep, manifest) {
            ProviderResult::Resolved(pkg) => {
                let pkg_path = pkg.path.clone();
                let pkg_identity = resolved_identity(&pkg);
                resolved_by_alias.insert(alias_key.to_string(), requested_identity.to_string());
                resolved.insert(alias_key.to_string(), pkg.path.clone());
                packages.push(pkg);

                // Only traverse transitive deps once per resolved package identity.
                if let std::collections::btree_map::Entry::Vacant(e) =
                    resolved_by_identity.entry(pkg_identity)
                {
                    e.insert(alias_key.to_string());
                    if let Ok(Some(sub_manifest)) = super::manifest::Manifest::from_dir(&pkg_path) {
                        for (sub_name, sub_dep) in &sub_manifest.deps {
                            queue.push_back((
                                sub_name.clone(),
                                sub_dep.clone(),
                                alias_key.to_string(),
                                sub_manifest.root_dir.clone(),
                            ));
                        }
                    }
                }
                return true;
            }
            ProviderResult::Error(e) => {
                errors.push(e);
                return true;
            }
            ProviderResult::NotApplicable => continue,
        }
    }
    false
}

fn resolve_deps_inner(
    manifest: &Manifest,
    force_remote: bool,
    lockfile: Option<&Lockfile>,
    flags: StoreRefreshFlags,
) -> ResolveResult {
    // Build a lookup table from lockfile: package name -> locked version.
    // HashMap<String, String> means last-writer-wins for duplicate names.
    // In practice, version-qualified names (e.g. "alice/http@b.12") act as
    // distinct keys, so multiple versions of the same package are supported
    // only when stored under version-qualified keys — not by HashMap allowing
    // duplicate keys.
    let locked_versions: std::collections::HashMap<String, String> = lockfile
        .map(|lf| {
            lf.packages
                .iter()
                .map(|p| (p.name.clone(), p.version.clone()))
                .collect()
        })
        .unwrap_or_default();

    let store_provider: Box<dyn PackageProvider> = if force_remote {
        Box::new(
            StoreProvider::new_force_remote()
                .with_refresh_flags(flags.force_refresh, flags.no_remote_check),
        )
    } else {
        Box::new(
            StoreProvider::new().with_refresh_flags(flags.force_refresh, flags.no_remote_check),
        )
    };

    let providers: Vec<Box<dyn PackageProvider>> = vec![
        Box::new(WorkspaceProvider),
        Box::new(CoreBundledProvider::new()),
        store_provider,
    ];

    let mut resolved = BTreeMap::new();
    let mut packages = Vec::new();
    let mut errors = Vec::new();
    let mut resolved_by_alias: BTreeMap<String, String> = BTreeMap::new();
    let mut resolved_by_identity: BTreeMap<String, String> = BTreeMap::new();

    // BFS queue: (dep_name, dependency, requester, declared_from_root)
    let mut queue: std::collections::VecDeque<(
        String,
        super::manifest::Dependency,
        String,
        PathBuf,
    )> = std::collections::VecDeque::new();

    // Enqueue root dependencies
    for (name, dep) in &manifest.deps {
        queue.push_back((
            name.clone(),
            dep.clone(),
            "root".to_string(),
            manifest.root_dir.clone(),
        ));
    }

    while let Some((name, dep, requester, declared_from_root)) = queue.pop_front() {
        // Pin generation-only registry deps to locked exact versions.
        // A generation-only version has no '.' (e.g. "a" vs "a.2").
        let dep = match &dep {
            Dependency::Registry {
                org,
                name: dep_name,
                version,
            } if !version.contains('.') && locked_versions.contains_key(&name) => {
                let locked_ver = &locked_versions[&name];
                // Only pin if the locked version belongs to the same generation
                if locked_ver.starts_with(version) {
                    Dependency::Registry {
                        org: org.clone(),
                        name: dep_name.clone(),
                        version: locked_ver.clone(),
                    }
                } else {
                    dep
                }
            }
            _ => dep,
        };
        let requested_identity = dep_decl_identity(&dep, &declared_from_root);

        if let Some(existing_identity) = resolved_by_alias.get(&name) {
            if existing_identity != &requested_identity {
                // RC-1q: Version coexistence (pnpm-style).
                // When a transitive dependency requests a different version of a registry
                // package that is already resolved, allow both versions to coexist by
                // installing the new version under a version-qualified alias key.
                if let Dependency::Registry { version, .. } = &dep {
                    let versioned_key = format!("{}@{}", name, version);
                    if resolved_by_alias.contains_key(&versioned_key) {
                        // Already resolved under this version-qualified key
                        continue;
                    }
                    // Resolve the conflicting version under a version-qualified key.
                    // Note: We are inside `if let Dependency::Registry { .. }`,
                    // so `dep` is always Registry here. No need for Path root_dir override.
                    if !try_resolve_with_providers(
                        &providers,
                        &versioned_key,
                        &dep,
                        manifest,
                        &requested_identity,
                        &mut resolved_by_alias,
                        &mut resolved,
                        &mut packages,
                        &mut resolved_by_identity,
                        &mut queue,
                        &mut errors,
                    ) {
                        errors.push(format!(
                            "Dependency '{}': no provider can resolve version '{}' for coexistence",
                            name, version
                        ));
                    }
                } else {
                    // RC-1t: Non-registry alias conflicts (path deps) remain a hard error
                    // with an improved message explaining the situation.
                    errors.push(format!(
                        "Dependency alias conflict: '{}' requested by {} as '{}' \
                         but already resolved as '{}'. \
                         Version coexistence is only supported for registry dependencies. \
                         For path dependencies, use distinct alias names in packages.tdm.",
                        name, requester, requested_identity, existing_identity
                    ));
                }
            }
            continue;
        }

        // For path deps, resolve relative to the declaring package's root directory.
        let effective_manifest = if matches!(dep, Dependency::Path { .. }) {
            let mut m = manifest.clone();
            m.root_dir = declared_from_root.clone();
            m
        } else {
            manifest.clone()
        };

        if !try_resolve_with_providers(
            &providers,
            &name,
            &dep,
            &effective_manifest,
            &requested_identity,
            &mut resolved_by_alias,
            &mut resolved,
            &mut packages,
            &mut resolved_by_identity,
            &mut queue,
            &mut errors,
        ) {
            errors.push(format!(
                "Dependency '{}': no provider can resolve this dependency type",
                name
            ));
        }
    }

    ResolveResult {
        resolved,
        packages,
        errors,
    }
}

/// Install resolved dependencies by creating symlinks in `.taida/deps/`.
///
/// RC-1q: Supports version-qualified package names (e.g., `org/pkg@version`).
/// These are installed to `.taida/deps/org/pkg@version/` directories, enabling
/// multiple versions of the same package to coexist (pnpm-style).
pub fn install_deps(manifest: &Manifest, result: &ResolveResult) -> Result<(), String> {
    let taida_dir = manifest.root_dir.join(".taida");
    let deps_dir = manifest.root_dir.join(".taida").join("deps");
    std::fs::create_dir_all(&taida_dir).map_err(|e| format!("Cannot create .taida/: {}", e))?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_deps_dir = taida_dir.join(format!(".deps.tmp-{}-{}", std::process::id(), nonce));

    let _ = std::fs::remove_dir_all(&tmp_deps_dir);
    std::fs::create_dir_all(&tmp_deps_dir)
        .map_err(|e| format!("Cannot create staging deps dir: {}", e))?;

    if let Err(e) = install_deps_into(result, &tmp_deps_dir) {
        let _ = std::fs::remove_dir_all(&tmp_deps_dir);
        return Err(e);
    }

    if deps_dir.exists() {
        replace_existing_deps_dir(&tmp_deps_dir, &deps_dir).map_err(|e| {
            let _ = std::fs::remove_dir_all(&tmp_deps_dir);
            format!("Cannot replace .taida/deps atomically: {}", e)
        })?;
        // After an atomic exchange, tmp_deps_dir contains the previous deps.
        let _ = std::fs::remove_dir_all(&tmp_deps_dir);
    } else if let Err(e) = std::fs::rename(&tmp_deps_dir, &deps_dir) {
        let _ = std::fs::remove_dir_all(&tmp_deps_dir);
        return Err(format!("Cannot install .taida/deps: {}", e));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn replace_existing_deps_dir(tmp_deps_dir: &Path, deps_dir: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    const RENAME_EXCHANGE: libc::c_uint = 2;

    let tmp = CString::new(tmp_deps_dir.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let deps = CString::new(deps_dir.as_os_str().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let rc = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            tmp.as_ptr(),
            libc::AT_FDCWD,
            deps.as_ptr(),
            RENAME_EXCHANGE,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
fn replace_existing_deps_dir(_tmp_deps_dir: &Path, _deps_dir: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic directory exchange is only implemented on Linux",
    ))
}

fn install_deps_into(result: &ResolveResult, deps_dir: &Path) -> Result<(), String> {
    // Install each resolved package using its provider's install method
    let providers: Vec<Box<dyn PackageProvider>> = vec![
        Box::new(WorkspaceProvider),
        Box::new(CoreBundledProvider::new()),
        Box::new(StoreProvider::new()),
    ];

    for pkg in &result.packages {
        // Find the appropriate provider for installation
        let installed = providers.iter().any(|provider| {
            let can_install = match &pkg.source {
                PackageSource::Path(_) => provider.name() == "workspace",
                PackageSource::CoreBundled => provider.name() == "core-bundled",
                PackageSource::Store { .. } => provider.name() == "store",
            };
            if can_install {
                if let Err(e) = provider.install(pkg, deps_dir) {
                    eprintln!("  Warning: {}", e);
                    return false;
                }
                return true;
            }
            false
        });

        if !installed {
            // Fallback: create symlink directly
            // RC-1q: pkg.name may be version-qualified (e.g., "org/pkg@b.12")
            // which installs to `.taida/deps/org/pkg@b.12/`
            let link_path = deps_dir.join(&pkg.name);
            if let Some(parent) = link_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Cannot create parent dir for '{}': {}", pkg.name, e))?;
            }
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&pkg.path, &link_path)
                    .map_err(|e| format!("Cannot create symlink for '{}': {}", pkg.name, e))?;
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_dir(&pkg.path, &link_path)
                    .map_err(|e| format!("Cannot create symlink for '{}': {}", pkg.name, e))?;
            }
        }
    }

    Ok(())
}

/// RC1.5: Install addon prebuilds for all resolved packages that have `native/addon.toml`.
///
/// For each package, checks if `native/addon.toml` exists in the package dir.
/// If present, parses the manifest, detects the host target, and fetches the prebuild
/// binary via `fetch_prebuild`. The `.so` is placed at
/// `.taida/deps/<pkg>/native/lib<name>.<ext>`.
///
/// Fails fast on any addon-related error (partial state is not left behind).
///
/// `force_refresh`: if true, bypasses the cache and re-downloads.
pub fn install_addon_prebuilds(
    manifest: &Manifest,
    result: &ResolveResult,
    force_refresh: bool,
    lockfile: Option<&Lockfile>,
    allow_local_addon_build: bool,
) -> Result<BTreeMap<String, crate::pkg::lockfile::LockedAddon>, String> {
    let deps_dir = manifest.root_dir.join(".taida").join("deps");
    let mut addon_info: BTreeMap<String, crate::pkg::lockfile::LockedAddon> = BTreeMap::new();

    // Detect host target once
    let host = host_target::detect_host_target().map_err(|e| {
        format!(
            "cannot detect host target for addon prebuild: {} (supported: {})",
            e.host_triple,
            host_target::supported_targets().join(", ")
        )
    })?;

    for pkg in &result.packages {
        let addon_toml = pkg.path.join("native").join("addon.toml");
        if !addon_toml.exists() {
            continue; // No addon manifest -> skip
        }

        // Parse the addon manifest
        let addon_manifest = parse_addon_manifest(&addon_toml).map_err(|e| e.to_string())?;

        // Check if prebuild is configured
        if !addon_manifest.prebuild.has_prebuild() {
            // Prebuild not configured -> skip (RC1 "manual placement" mode)
            continue;
        }

        // RC2.7: attempt prebuild fetch, then potentially fall back to
        // local build if allowed. The "try prebuild" result is captured
        // so fallback decisions can inspect the error variant.
        let prebuild_result =
            try_fetch_prebuild(pkg, &addon_manifest, &host, lockfile, force_refresh);

        match prebuild_result {
            Ok((fetched_path, expected_sha256)) => {
                // RC2.7B-010: use shared helper for binary placement
                place_addon_binary(
                    &deps_dir,
                    &pkg.name,
                    &addon_manifest.library,
                    host.cdylib_ext(),
                    &fetched_path,
                )?;

                // RC2.7B-004: record the actual SHA from the prebuild
                addon_info.insert(
                    pkg.name.clone(),
                    crate::pkg::lockfile::LockedAddon {
                        target: host.as_triple().to_string(),
                        sha256: expected_sha256,
                    },
                );
            }
            Err(PrebuildFailure::IntegrityMismatch(msg)) => {
                // RC2.7: integrity mismatch is ALWAYS a hard error.
                // Never fall back to local build for security reasons.
                return Err(msg);
            }
            Err(PrebuildFailure::UnsupportedTarget(msg)) => {
                // RC2.7B-009: unsupported target is ALWAYS a hard error.
                // Never fall back to local build — the addon was not
                // designed for this platform.
                return Err(msg);
            }
            Err(PrebuildFailure::Unavailable(msg)) => {
                // RC2.7-4b: prebuild missing/unavailable/transport failure.
                if allow_local_addon_build {
                    // Attempt local cargo build fallback.
                    eprintln!(
                        "[addon] prebuild unavailable; falling back to local cargo build for {}@{}",
                        pkg.name, pkg.version
                    );
                    let built_path = try_local_addon_build(pkg, &addon_manifest, &host)?;

                    // RC2.7B-010: use shared helper for binary placement
                    place_addon_binary(
                        &deps_dir,
                        &pkg.name,
                        &addon_manifest.library,
                        host.cdylib_ext(),
                        &built_path,
                    )?;

                    // RC2.7 design: local build artifacts are NOT recorded in
                    // taida.lock's addon stanza (provenance differs from prebuilds).
                } else {
                    // No fallback — emit helpful error suggesting the flag.
                    return Err(format!(
                        "{}\n\
                         action:\n\
                         \x20 - ask the addon author to upload a prebuild for {}\n\
                         \x20 - or retry with `taida ingot install --allow-local-addon-build`",
                        msg,
                        host.as_triple()
                    ));
                }
            }
        }
    }

    Ok(addon_info)
}

// ── RC2.7B-010: shared binary placement helper ──────────────

/// Place an addon binary at `.taida/deps/<pkg>/native/lib<library>.<ext>`.
///
/// Creates the `native/` directory if needed and copies the binary from
/// `source_path`. Extracted from the prebuild success and local build
/// fallback arms to eliminate code duplication (RC2.7B-010).
fn place_addon_binary(
    deps_dir: &Path,
    pkg_name: &str,
    library: &str,
    ext: &str,
    source_path: &Path,
) -> Result<PathBuf, String> {
    let native_dir = deps_dir.join(pkg_name).join("native");
    std::fs::create_dir_all(&native_dir)
        .map_err(|e| format!("cannot create native dir for '{}': {}", pkg_name, e))?;

    let dest = native_dir.join(format!("lib{}.{}", library, ext));
    std::fs::copy(source_path, &dest)
        .map_err(|e| format!("cannot copy addon binary to {}: {}", dest.display(), e))?;

    Ok(dest)
}

// ── RC2.7: prebuild fetch result type ────────────────────────

/// Prebuild fetch failure classification for fallback policy.
enum PrebuildFailure {
    /// Integrity mismatch — never eligible for fallback.
    IntegrityMismatch(String),
    /// Missing / unavailable / transport failure — eligible for local build fallback.
    Unavailable(String),
    /// RC2.7B-009: host target is not registered in addon.toml / addon.lock.toml.
    /// Never eligible for fallback, regardless of `--allow-local-addon-build`.
    /// Design: Section C "unsupported target では fallback しない".
    UnsupportedTarget(String),
}

/// SHA-source decision for a single `(addon, host)` pair.
///
/// Exposed so unit tests can assert the decision table without spinning
/// up a release download or a real package tarball.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShaSource {
    /// Use the SHA already present in `addon.toml`'s
    /// `[library.prebuild.targets]` — it is a real, non-placeholder
    /// digest for our host triple.
    AddonToml,
    /// Consult `addon.lock.toml` from the release asset. Either:
    /// - `addon.toml` has no row for our host triple, or
    /// - `addon.toml`'s row is a `sha256:0{64}` placeholder (C14B-012).
    LockfileFallback,
    /// Addon declares no prebuild at all — hard `UnsupportedTarget`
    /// error, no fallback eligible.
    NoPrebuild,
}

/// Decide which digest source to trust for a prebuild fetch.
///
/// See [`ShaSource`] for the outcomes. The input is deliberately
/// minimal — just the row that `addon.toml` carries (if any) and
/// whether the manifest declares a prebuild URL template — so the
/// decision can be unit-tested without threading the whole
/// `AddonManifest` / `HostTarget` / resolver context in.
pub(crate) fn choose_sha_source(
    addon_toml_entry: Option<&str>,
    has_prebuild_url: bool,
) -> ShaSource {
    match addon_toml_entry {
        Some(sha) if crate::addon::prebuild_fetcher::is_placeholder_sha(sha) => {
            // C14B-012: placeholder row cannot be trusted. Requires
            // the manifest to at least declare a prebuild URL, since
            // the lockfile asset only exists when a release shipped.
            if has_prebuild_url {
                ShaSource::LockfileFallback
            } else {
                ShaSource::NoPrebuild
            }
        }
        Some(_) => ShaSource::AddonToml,
        None => {
            if has_prebuild_url {
                ShaSource::LockfileFallback
            } else {
                ShaSource::NoPrebuild
            }
        }
    }
}

/// Attempt to fetch a prebuild for the given package.
///
/// Returns `Ok((path, expected_sha256))` with the cached binary path and the
/// expected SHA-256 hex string on success, or a classified error for the
/// fallback policy to decide on.
///
/// RC2.7B-004: the SHA is returned so the caller can record it in the lockfile.
fn try_fetch_prebuild(
    pkg: &ResolvedPackage,
    addon_manifest: &crate::addon::manifest::AddonManifest,
    host: &host_target::HostTarget,
    lockfile: Option<&Lockfile>,
    force_refresh: bool,
) -> Result<(PathBuf, String), PrebuildFailure> {
    // Check if the host target is supported by this addon.
    //
    // C14B-012: `taida init --target rust-addon` seeds
    // `[library.prebuild.targets]` with `sha256:0{64}` placeholders so
    // the scaffolded package type-checks before the first CI release
    // computes real digests. Those placeholders live at the tag-time
    // commit forever (the tarball fetched by install is immutable),
    // so treating them as "real SHAs" leads to a permanent integrity
    // mismatch. `choose_sha_source` routes placeholder entries to the
    // `addon.lock.toml` release-asset fallback the same way as
    // "target absent".
    let addon_toml_entry = addon_manifest.prebuild.targets.get(host.as_triple());
    let source = choose_sha_source(
        addon_toml_entry.map(String::as_str),
        addon_manifest.prebuild.has_prebuild(),
    );
    let must_use_lock_fallback = matches!(source, ShaSource::LockfileFallback)
        && addon_toml_entry
            .map(|s| crate::addon::prebuild_fetcher::is_placeholder_sha(s))
            .unwrap_or(false);

    let expected_sha256: String = match source {
        ShaSource::AddonToml => {
            // Happy path: addon.toml carries a real, non-placeholder
            // SHA for our host triple. No lockfile fetch needed.
            addon_toml_entry
                .expect("AddonToml implies Some entry")
                .clone()
        }
        ShaSource::NoPrebuild => {
            // addon.toml carries no prebuild config at all and there's
            // nothing to consult — the addon simply does not ship a
            // prebuild for any platform.
            return Err(PrebuildFailure::UnsupportedTarget(format!(
                "addon '{}' is not available for your platform\n\
                 \x20 host target:    {}",
                addon_manifest.package,
                host.as_triple(),
            )));
        }
        ShaSource::LockfileFallback => {
            // RC2.6 addon.lock.toml fallback (extended by C14B-012 to
            // cover placeholder SHA-256 rows): download
            // `addon.lock.toml` from the GitHub Release asset and use
            // it as the authoritative SHA source. The lockfile is
            // CI-generated at release time, so its rows match the
            // actual cdylib uploads byte-for-byte.
            let lockfile_text = match crate::addon::prebuild_fetcher::fetch_release_lockfile(
                &pkg.name,
                &pkg.version,
            ) {
                Ok(text) => text,
                Err(e) => {
                    // If we got here solely because of a placeholder, the
                    // error message should mention the placeholder gap so
                    // the addon author knows to upload `addon.lock.toml`.
                    let context = if must_use_lock_fallback {
                        format!(
                            "addon '{}' ships a placeholder sha256 in addon.toml at this tag; \
                             addon.lock.toml fallback is required but could not be fetched: {}",
                            addon_manifest.package, e
                        )
                    } else {
                        format!(
                            "addon '{}' prebuild lockfile fetch failed: {}",
                            addon_manifest.package, e
                        )
                    };
                    return Err(PrebuildFailure::Unavailable(context));
                }
            };
            let addon_lockfile = match crate::addon::lockfile::parse_lockfile_str(
                std::path::Path::new("addon.lock.toml"),
                &lockfile_text,
            ) {
                Ok(lf) => lf,
                Err(e) => {
                    return Err(PrebuildFailure::Unavailable(format!(
                        "Failed to parse addon.lock.toml for '{}@{}': {}",
                        pkg.name, pkg.version, e
                    )));
                }
            };
            match addon_lockfile.targets.get(host.as_triple()) {
                Some(sha) => sha.clone(),
                None => {
                    // RC2.7B-009: target not registered in addon.lock.toml
                    // -> UnsupportedTarget (never eligible for fallback).
                    // Extended wording: if we got here via a placeholder
                    // in addon.toml, the message points both at the
                    // placeholder and the missing lockfile row.
                    let available: Vec<&str> =
                        addon_lockfile.targets.keys().map(String::as_str).collect();
                    let hint = if must_use_lock_fallback {
                        "addon.toml row is a placeholder sha256 and addon.lock.toml has no row for your platform"
                    } else {
                        "addon.toml does not list your platform and addon.lock.toml has no row for it"
                    };
                    return Err(PrebuildFailure::UnsupportedTarget(format!(
                        "addon '{}' prebuild is not available for your platform\n\
                         \x20 host target:    {}\n\
                         \x20 reason:         {}\n\
                         \x20 available targets in addon.lock.toml:\n\
                         \x20   - {}",
                        addon_manifest.package,
                        host.as_triple(),
                        hint,
                        if available.is_empty() {
                            "(none)".to_string()
                        } else {
                            available.join("\n    - ")
                        },
                    )));
                }
            }
        }
    };

    // Strip sha256: prefix
    let expected_sha256 = expected_sha256
        .strip_prefix("sha256:")
        .unwrap_or(&expected_sha256)
        .to_string();

    // Expand URL template
    let url = crate::addon::url_template::expand_url_template(
        addon_manifest.prebuild.url_template.as_ref().unwrap(),
        &pkg.version,
        host.as_triple(),
        host.cdylib_ext(),
        &addon_manifest.library,
    )
    .map_err(|e| {
        PrebuildFailure::Unavailable(format!("addon URL template expansion failed: {}", e))
    })?;

    // RC1.5-3b-4: cross-check with lockfile if available
    if let Some(lf) = lockfile {
        for locked in &lf.packages {
            if locked.name == pkg.name
                && let Some(ref locked_addon) = locked.addon
                && locked_addon.target == host.as_triple()
                && locked_addon.sha256 != expected_sha256
            {
                // RC1.5-3b-5: mismatch -> reject (integrity)
                return Err(PrebuildFailure::IntegrityMismatch(format!(
                    "lockfile addon SHA-256 mismatch for package '{}' (expected {}, got {}). Re-run `taida ingot install --force-refresh`.",
                    pkg.name, expected_sha256, locked_addon.sha256
                )));
            }
        }
    }

    // Parse out org/name from package id
    let (org, name) = pkg.name.split_once('/').ok_or_else(|| {
        PrebuildFailure::Unavailable(format!(
            "cannot parse package '{}' as org/name for addon prebuild",
            pkg.name
        ))
    })?;

    // Fetch prebuild binary
    let mut reporter = make_progress_reporter(&pkg.name, host.as_triple());
    let fetch_result = if force_refresh {
        let cache_root = std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".taida/addon-cache"));
        if let Some(ref root) = cache_root {
            let pkg_cache = root
                .join(org)
                .join(name)
                .join(&pkg.version)
                .join(host.as_triple());
            if pkg_cache.exists() {
                let _ = std::fs::remove_dir_all(&pkg_cache);
            }
        }
        fetch_prebuild_with_progress(
            &pkg.name,
            &pkg.version,
            host.as_triple(),
            &addon_manifest.library,
            host.cdylib_ext(),
            &url,
            &expected_sha256,
            &mut *reporter,
        )
    } else {
        fetch_prebuild_with_progress(
            &pkg.name,
            &pkg.version,
            host.as_triple(),
            &addon_manifest.library,
            host.cdylib_ext(),
            &url,
            &expected_sha256,
            &mut *reporter,
        )
    };

    match fetch_result {
        Ok(path) => {
            // C26B-030 / SEC-011: run cosign bundle verification after
            // the SHA-256 check succeeds. Policy is resolved per-URL so
            // non-official mirrors stay `Disabled` by default, first-
            // party github.com/taida-lang/* URLs get `BestEffort`, and
            // anyone who wants hard-fail coverage sets
            // `TAIDA_VERIFY_SIGNATURES=required`. Integrity is already
            // proven against the manifest SHA-256 at this point —
            // cosign adds the supply-chain identity proof on top.
            use crate::addon::signature_verify::{
                VerifyError, VerifyOutcome, VerifyPolicy, verify_artifact,
            };
            let policy = VerifyPolicy::resolve(&url);
            match verify_artifact(&path, &url, policy) {
                Ok(VerifyOutcome::Verified) => {
                    eprintln!("  SEC-011 verify: OK ({})", pkg.name);
                }
                Ok(VerifyOutcome::Warned(reason)) => {
                    eprintln!("  SEC-011 verify: WARN ({}): {reason}", pkg.name);
                }
                Ok(VerifyOutcome::Skipped) => {
                    // Silent by design — non-official URLs and explicit
                    // `TAIDA_VERIFY_SIGNATURES=0` both resolve here.
                }
                Err(VerifyError::SignatureRejected { stderr }) => {
                    return Err(PrebuildFailure::IntegrityMismatch(format!(
                        "SEC-011 signature verification rejected '{}': {}",
                        pkg.name,
                        stderr.trim()
                    )));
                }
                Err(e) => {
                    return Err(PrebuildFailure::IntegrityMismatch(format!(
                        "SEC-011 signature verification failed for '{}': {e}",
                        pkg.name
                    )));
                }
            }
            Ok((path, expected_sha256))
        }
        Err(FetchError::IntegrityMismatch { expected, actual }) => {
            Err(PrebuildFailure::IntegrityMismatch(format!(
                "addon integrity check failed for '{}'\n\
                 \x20 expected: {}\n\
                 \x20 actual:   {}",
                pkg.name, expected, actual
            )))
        }
        Err(FetchError::UnsupportedTarget {
            ref host,
            ref supported,
        }) => {
            // RC2.7B-009: explicit match for UnsupportedTarget from fetch layer
            Err(PrebuildFailure::UnsupportedTarget(format!(
                "addon prebuild is not available for your platform\n\
                 \x20 host target:    {}\n\
                 \x20 supported targets:\n\
                 \x20   - {}",
                host,
                if supported.is_empty() {
                    "(none)".to_string()
                } else {
                    supported.join("\n    - ")
                },
            )))
        }
        Err(e) => Err(PrebuildFailure::Unavailable(addon_fetch_error_as_string(
            &e,
        ))),
    }
}

/// RC2.7-4b: attempt local cargo build when prebuild is unavailable.
///
/// Uses the shared build helper from `crate::addon::build` with
/// `CARGO_TARGET_DIR` pointing at an external cache directory
/// so the source tree in the package store is not polluted.
#[cfg(feature = "native")]
fn try_local_addon_build(
    pkg: &ResolvedPackage,
    _addon_manifest: &crate::addon::manifest::AddonManifest,
    host: &host_target::HostTarget,
) -> Result<PathBuf, String> {
    // Build cache: ~/.taida/addon-build/{org}/{name}/{version}/{integrity}/{target}/
    let home = std::env::var("HOME")
        .map_err(|_| "cannot determine home directory ($HOME not set)".to_string())?;
    let (org, name) = pkg.name.split_once('/').ok_or_else(|| {
        format!(
            "cannot parse package '{}' as org/name for local addon build",
            pkg.name
        )
    })?;
    let build_cache = PathBuf::from(&home)
        .join(".taida")
        .join("addon-build")
        .join(org)
        .join(name)
        .join(&pkg.version)
        .join(&pkg.integrity)
        .join(host.as_triple());

    std::fs::create_dir_all(&build_cache)
        .map_err(|e| format!("cannot create addon build cache dir: {}", e))?;

    let build_output = crate::addon::build::build_addon_artifacts(&pkg.path, Some(&build_cache))?;

    Ok(build_output.cdylib_path)
}

#[cfg(not(feature = "native"))]
fn try_local_addon_build(
    pkg: &ResolvedPackage,
    _addon_manifest: &crate::addon::manifest::AddonManifest,
    _host: &host_target::HostTarget,
) -> Result<PathBuf, String> {
    Err(format!(
        "local addon build for '{}' requires the `native` feature",
        pkg.name,
    ))
}

/// RC15B-002: builds a progress reporter closure that prints a one-line
/// carriage-return indicator to stderr while an addon binary downloads.
///
/// The reporter deliberately stays silent for the first ~256 KiB so
/// tiny sample addons (and file:// copies during tests) don't clutter
/// terminal output. Once the indicator starts it updates at most about
/// 20 times per second because the underlying fetcher already throttles
/// its own callbacks to ~64 KiB granularity.
///
/// The indicator uses `\r` + a fixed-width layout so successive updates
/// overwrite the same line. A final newline is printed when the
/// download completes (signalled by `so_far == total`) so subsequent CLI
/// output doesn't overwrite the final progress line.
fn make_progress_reporter(
    pkg_name: &str,
    target: &str,
) -> Box<dyn FnMut(u64, Option<u64>) + Send + 'static> {
    use std::io::Write;

    let label = format!("    downloading {} ({})", pkg_name, target);
    let mut started = false;
    let mut newline_printed = false;

    Box::new(move |so_far: u64, total: Option<u64>| {
        // Stay silent until the payload gets big enough to matter.
        const MIN_BYTES_TO_SHOW: u64 = 256 * 1024;
        let big_enough = match total {
            Some(t) => t >= MIN_BYTES_TO_SHOW,
            None => so_far >= MIN_BYTES_TO_SHOW,
        };
        if !big_enough {
            return;
        }

        let stderr = std::io::stderr();
        let mut lock = stderr.lock();
        if !started {
            started = true;
        }

        match total {
            Some(t) if t > 0 => {
                let pct = (so_far as f64 / t as f64 * 100.0).min(100.0);
                let _ = write!(
                    lock,
                    "\r{} {:>6.1}%  {:>10}/{:<10}",
                    label,
                    pct,
                    human_bytes(so_far),
                    human_bytes(t)
                );
            }
            _ => {
                let _ = write!(lock, "\r{} {:>10}", label, human_bytes(so_far));
            }
        }
        let _ = lock.flush();

        // Trailing newline when we reach the known end.
        if let Some(t) = total
            && so_far >= t
            && !newline_printed
        {
            newline_printed = true;
            let _ = writeln!(lock);
        }
    })
}

/// Format a byte count as a short human-readable string.
fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{} B", bytes)
    }
}

fn addon_fetch_error_as_string(err: &FetchError) -> String {
    match err {
        FetchError::UnsupportedTarget { host, supported } => format!(
            "addon not available for platform '{}' (supported: {})",
            host,
            supported.join(", ")
        ),
        FetchError::DownloadFailed { message } => format!("addon download failed: {}", message),
        FetchError::IntegrityMismatch { expected, actual } => format!(
            "addon integrity check failed (expected {}, got {})",
            expected, actual
        ),
        FetchError::SizeLimitExceeded {
            max_bytes,
            actual_bytes,
        } => format!(
            "addon binary too large ({} bytes, max {} bytes)",
            actual_bytes, max_bytes
        ),
        FetchError::CacheIoError { message } => format!("addon cache I/O error: {}", message),
    }
}

/// Generate/update the lockfile after dependency resolution.
pub fn write_lockfile(manifest: &Manifest, result: &ResolveResult) -> Result<(), String> {
    let lock_path = manifest.root_dir.join(".taida").join("taida.lock");

    // Ensure .taida/ directory exists
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create .taida/ directory: {}", e))?;
    }
    let lockfile = Lockfile::from_resolved(&result.packages);

    // Check if lockfile is already up to date
    if let Ok(Some(existing)) = Lockfile::read(&lock_path)
        && existing.is_up_to_date(&result.packages)
    {
        return Ok(()); // Already up to date, skip write
    }

    lockfile.write(&lock_path)
}

/// RC1.5-3b: Generate/update lockfile with addon info.
pub fn write_lockfile_with_addons(
    manifest: &Manifest,
    result: &ResolveResult,
    addons: &std::collections::BTreeMap<String, crate::pkg::lockfile::LockedAddon>,
) -> Result<(), String> {
    let lock_path = manifest.root_dir.join(".taida").join("taida.lock");

    // Ensure .taida/ directory exists
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create .taida/ directory: {}", e))?;
    }
    let mut lockfile = Lockfile::from_resolved(&result.packages);

    // Attach addon info
    for (pkg_name, addon) in addons {
        lockfile.with_addon(pkg_name, addon.target.clone(), addon.sha256.clone())?;
    }

    lockfile.write(&lock_path)
}

/// Resolve a package import path using installed dependencies.
///
/// Supports both canonical `org/name` IDs (→ `.taida/deps/org/name/`)
/// and legacy single-segment names (→ `.taida/deps/name/`).
///
/// RC-1q: Also supports version-qualified IDs (→ `.taida/deps/org/name@version/`).
/// When a versioned import (e.g. `>>> alice/http@b.12`) is encountered and the
/// unversioned path does not exist, tries the version-qualified directory.
pub fn resolve_package_import(project_root: &Path, package_id: &str) -> Option<PathBuf> {
    // Try canonical org/name path first
    let dep_path = project_root.join(".taida").join("deps").join(package_id);
    if dep_path.exists() {
        return Some(dep_path);
    }
    // Fallback: single-segment (local path deps use name-only)
    if !package_id.contains('/') {
        return None;
    }
    None
}

/// Resolve a versioned package import path using installed dependencies.
///
/// RC-1q: Tries version-qualified directory first (`org/name@version/`),
/// then falls back to unversioned (`org/name/`).
/// This enables pnpm-style version coexistence where multiple versions
/// of the same package are installed side-by-side.
pub fn resolve_package_import_versioned(
    project_root: &Path,
    package_id: &str,
    version: &str,
) -> Option<PathBuf> {
    let deps_dir = project_root.join(".taida").join("deps");

    // Try version-qualified path first: org/name@version
    let versioned_id = format!("{}@{}", package_id, version);
    let versioned_path = deps_dir.join(&versioned_id);
    if versioned_path.exists() {
        return Some(versioned_path);
    }

    // Fall back to unversioned path: org/name
    let unversioned_path = deps_dir.join(package_id);
    if unversioned_path.exists() {
        return Some(unversioned_path);
    }

    None
}

/// Result of resolving a package module import path.
#[derive(Debug)]
pub struct PackageModuleResolution {
    /// The package root directory (e.g. `.taida/deps/taida-lang/crypto/`)
    pub pkg_dir: PathBuf,
    /// The submodule path within the package, if any (e.g. `hash` for `taida-lang/crypto/hash`)
    pub submodule: Option<String>,
}

/// Resolve a non-versioned package import path using longest-prefix matching.
///
/// Given `import_path` like `taida-lang/crypto`, tries `.taida/deps/taida-lang/crypto/`
/// as a package root. If that doesn't exist, falls back to first-slash split
/// (`.taida/deps/taida-lang/` + submodule `crypto`).
///
/// For `taida-lang/crypto/hash`, first tries `.taida/deps/taida-lang/crypto/hash/`,
/// then `.taida/deps/taida-lang/crypto/` + submodule `hash`,
/// then `.taida/deps/taida-lang/` + submodule `crypto/hash`.
///
/// Returns the package directory and optional submodule path.
pub fn resolve_package_module(
    project_root: &Path,
    import_path: &str,
) -> Option<PackageModuleResolution> {
    let deps_dir = project_root.join(".taida").join("deps");

    // Longest-prefix matching: try the full path, then progressively shorter prefixes
    let segments: Vec<&str> = import_path.split('/').collect();
    // Start from full path, go down to min 2 segments (org/name) if multi-segment,
    // or 1 segment for single-segment package names.
    let min_prefix = if segments.len() >= 2 { 2 } else { 1 };
    for prefix_len in (min_prefix..=segments.len()).rev() {
        let prefix = segments[..prefix_len].join("/");
        let candidate = deps_dir.join(&prefix);
        if candidate.exists() && candidate.is_dir() {
            let submodule = if prefix_len < segments.len() {
                Some(segments[prefix_len..].join("/"))
            } else {
                None
            };
            return Some(PackageModuleResolution {
                pkg_dir: candidate,
                submodule,
            });
        }
    }

    None
}

/// Resolve a versioned package import path using longest-prefix matching.
///
/// RC-1q: Like `resolve_package_module` but tries version-qualified directories
/// first (e.g., `.taida/deps/alice/http@b.12/`) before falling back to
/// unversioned directories. This enables pnpm-style version coexistence.
///
/// Supports submodule imports: `alice/pkg/submod@b.12` resolves to
/// `.taida/deps/alice/pkg@b.12/submod.td` via longest-prefix matching.
pub fn resolve_package_module_versioned(
    project_root: &Path,
    import_path: &str,
    version: &str,
) -> Option<PackageModuleResolution> {
    let deps_dir = project_root.join(".taida").join("deps");

    // Longest-prefix matching with version-qualified directories
    let segments: Vec<&str> = import_path.split('/').collect();
    let min_prefix = if segments.len() >= 2 { 2 } else { 1 };

    // First pass: try version-qualified directories
    for prefix_len in (min_prefix..=segments.len()).rev() {
        let prefix = segments[..prefix_len].join("/");
        let versioned_prefix = format!("{}@{}", prefix, version);
        let candidate = deps_dir.join(&versioned_prefix);
        if candidate.exists() && candidate.is_dir() {
            let submodule = if prefix_len < segments.len() {
                Some(segments[prefix_len..].join("/"))
            } else {
                None
            };
            return Some(PackageModuleResolution {
                pkg_dir: candidate,
                submodule,
            });
        }
    }

    // Second pass: fall back to unversioned directories
    resolve_package_module(project_root, import_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── C14B-012: choose_sha_source decision table ─────────────

    const PLACEHOLDER: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    const REAL_SHA: &str =
        "sha256:d9c093fe0123456789abcdef0123456789abcdef0123456789abcdef01234567";

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nonce))
    }

    #[test]
    fn choose_sha_source_prefers_addon_toml_when_real_sha_present() {
        // Happy path: addon.toml has a real, CI-computed digest for
        // our host. No need to consult the lockfile asset.
        assert_eq!(
            choose_sha_source(Some(REAL_SHA), true),
            ShaSource::AddonToml
        );
    }

    #[test]
    fn choose_sha_source_routes_placeholder_to_lockfile() {
        // C14B-012 core contract: a `sha256:0{64}` placeholder in
        // addon.toml must NOT be trusted. We route to the lockfile
        // asset, which carries the real CI-computed digests.
        assert_eq!(
            choose_sha_source(Some(PLACEHOLDER), true),
            ShaSource::LockfileFallback
        );
    }

    #[test]
    fn choose_sha_source_routes_missing_target_to_lockfile_when_prebuild_declared() {
        // No row for this host triple in addon.toml, but the manifest
        // declares a prebuild URL — so a lockfile asset is expected
        // and consulted (pre-existing RC2.6 behaviour).
        assert_eq!(choose_sha_source(None, true), ShaSource::LockfileFallback);
    }

    #[test]
    fn choose_sha_source_no_prebuild_when_placeholder_and_no_url() {
        // Edge case: a placeholder is present but the manifest does
        // not declare a prebuild URL. There's no lockfile asset to
        // consult, so the addon simply does not ship for any platform.
        assert_eq!(
            choose_sha_source(Some(PLACEHOLDER), false),
            ShaSource::NoPrebuild
        );
    }

    #[test]
    fn choose_sha_source_no_prebuild_when_missing_and_no_url() {
        // No row, no URL — no prebuild. Hard error downstream.
        assert_eq!(choose_sha_source(None, false), ShaSource::NoPrebuild);
    }

    #[test]
    fn test_resolve_deps_local_path() {
        let dir = PathBuf::from("/tmp/taida_test_pkg_resolve");
        let dep_dir = dir.join("dep_lib");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dep_dir).unwrap();
        fs::write(dep_dir.join("lib.td"), "// lib").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "mylib".to_string(),
                    Dependency::Path {
                        path: "./dep_lib".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
        assert_eq!(result.resolved.len(), 1);
        assert!(result.resolved.contains_key("mylib"));
        assert_eq!(result.packages.len(), 1);
        assert_eq!(result.packages[0].name, "mylib");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_deps_missing_path() {
        let dir = PathBuf::from("/tmp/taida_test_pkg_resolve_missing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "missing".to_string(),
                    Dependency::Path {
                        path: "./nonexistent".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert_eq!(result.errors.len(), 1);
        assert!(
            result.errors[0].contains("missing"),
            "Error should mention 'missing' dep name, got: {}",
            result.errors[0]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_install_deps() {
        let dir = PathBuf::from("/tmp/taida_test_pkg_install");
        let dep_dir = dir.join("dep_lib");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dep_dir).unwrap();
        fs::write(dep_dir.join("lib.td"), "// lib").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "mylib".to_string(),
                    Dependency::Path {
                        path: "./dep_lib".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(result.errors.is_empty());

        install_deps(&manifest, &result).unwrap();

        // Check symlink exists
        let link = dir.join(".taida").join("deps").join("mylib");
        assert!(link.exists(), "Symlink should exist at {:?}", link);
        assert!(
            link.join("lib.td").exists(),
            "lib.td should be accessible through symlink"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_install_deps_preserves_existing_deps_on_staging_failure() {
        let dir = unique_test_dir("taida_test_pkg_install_txn");
        let dep_a = dir.join("dep_a");
        let dep_b = dir.join("dep_b");
        let existing_dir = dir.join(".taida").join("deps").join("existing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dep_a).unwrap();
        fs::create_dir_all(&dep_b).unwrap();
        fs::create_dir_all(&existing_dir).unwrap();
        fs::write(existing_dir.join("keep.td"), "// keep").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: dir.clone(),
            exports: Vec::new(),
        };
        let result = ResolveResult {
            resolved: BTreeMap::new(),
            packages: vec![
                ResolvedPackage {
                    name: "dup".to_string(),
                    version: "0.1.0".to_string(),
                    source: PackageSource::Path(dep_a.display().to_string()),
                    path: dep_a,
                    integrity: String::new(),
                },
                ResolvedPackage {
                    name: "dup".to_string(),
                    version: "0.1.0".to_string(),
                    source: PackageSource::Path(dep_b.display().to_string()),
                    path: dep_b,
                    integrity: String::new(),
                },
            ],
            errors: Vec::new(),
        };

        let err = install_deps(&manifest, &result).expect_err("duplicate install should fail");
        assert!(
            err.contains("Cannot create symlink for 'dup'"),
            "unexpected error: {}",
            err
        );
        assert!(
            existing_dir.join("keep.td").exists(),
            "existing deps must survive staging failure"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_install_deps_replaces_existing_deps_with_atomic_exchange() {
        let dir = unique_test_dir("taida_test_pkg_install_exchange");
        let dep_dir = dir.join("dep_new");
        let old_dir = dir.join(".taida").join("deps").join("old");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dep_dir).unwrap();
        fs::write(dep_dir.join("lib.td"), "// new lib").unwrap();
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("old.td"), "// old").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: dir.clone(),
            exports: Vec::new(),
        };
        let result = ResolveResult {
            resolved: BTreeMap::new(),
            packages: vec![ResolvedPackage {
                name: "newlib".to_string(),
                version: "0.1.0".to_string(),
                source: PackageSource::Path(dep_dir.display().to_string()),
                path: dep_dir,
                integrity: String::new(),
            }],
            errors: Vec::new(),
        };

        install_deps(&manifest, &result).unwrap();

        let deps = dir.join(".taida").join("deps");
        assert!(deps.join("newlib").join("lib.td").exists());
        assert!(
            !deps.join("old").exists(),
            "old deps should be removed after the exchange"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_package_import() {
        let dir = PathBuf::from("/tmp/taida_test_pkg_import");
        let _ = fs::remove_dir_all(&dir);

        // Single-segment (local path deps)
        let local_dir = dir.join(".taida").join("deps").join("mylib");
        fs::create_dir_all(&local_dir).unwrap();
        let result = resolve_package_import(&dir, "mylib");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), local_dir);

        // Canonical org/name (registry deps)
        let org_dir = dir.join(".taida").join("deps").join("alice").join("http");
        fs::create_dir_all(&org_dir).unwrap();
        let result = resolve_package_import(&dir, "alice/http");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), org_dir);

        let result = resolve_package_import(&dir, "missing");
        assert!(result.is_none());

        let result = resolve_package_import(&dir, "bob/missing");
        assert!(result.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_package_module_longest_prefix() {
        let dir = PathBuf::from("/tmp/taida_test_pkg_module_resolve");
        let _ = fs::remove_dir_all(&dir);

        // Setup: .taida/deps/taida-lang/crypto/ as package root
        let crypto_dir = dir
            .join(".taida")
            .join("deps")
            .join("taida-lang")
            .join("crypto");
        fs::create_dir_all(&crypto_dir).unwrap();
        fs::write(crypto_dir.join("main.td"), "// crypto").unwrap();

        // "taida-lang/crypto" → package root, no submodule
        let res = resolve_package_module(&dir, "taida-lang/crypto");
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.pkg_dir, crypto_dir);
        assert!(res.submodule.is_none());

        // "taida-lang/crypto/hash" → package root + submodule "hash"
        let res = resolve_package_module(&dir, "taida-lang/crypto/hash");
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.pkg_dir, crypto_dir);
        assert_eq!(res.submodule.as_deref(), Some("hash"));

        // "taida-lang/missing" → None
        let res = resolve_package_module(&dir, "taida-lang/missing");
        assert!(res.is_none());

        // Single-segment package
        let mylib_dir = dir.join(".taida").join("deps").join("mylib");
        fs::create_dir_all(&mylib_dir).unwrap();
        let res = resolve_package_module(&dir, "mylib");
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.pkg_dir, mylib_dir);
        assert!(res.submodule.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_registry_core_bundled() {
        let dir = PathBuf::from("/tmp/taida_test_resolve_bundled");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "taida-lang/os".to_string(),
                    Dependency::Registry {
                        org: "taida-lang".to_string(),
                        name: "os".to_string(),
                        version: "a.1".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
        assert_eq!(result.resolved.len(), 1);
        assert!(result.resolved.contains_key("taida-lang/os"));
        assert_eq!(result.packages.len(), 1);
        assert_eq!(result.packages[0].version, "a.1");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore] // Requires network access — run with `cargo test -- --ignored`
    fn test_resolve_registry_store_uncached() {
        // StoreProvider now tries to actually download, so uncached packages
        // will fail with a download error (not "not yet implemented")
        let dir = PathBuf::from("/tmp/taida_test_resolve_store");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "nonexistent-org-xyz/http".to_string(),
                    Dependency::Registry {
                        org: "nonexistent-org-xyz".to_string(),
                        name: "http".to_string(),
                        version: "a.1".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        // Should fail because the package doesn't exist on GitHub
        assert_eq!(result.errors.len(), 1);
        assert!(
            result.errors[0].contains("Failed") || result.errors[0].contains("failed"),
            "Error should indicate download failure, got: {}",
            result.errors[0]
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_mixed_deps() {
        let dir = PathBuf::from("/tmp/taida_test_resolve_mixed");
        let dep_dir = dir.join("local_lib");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dep_dir).unwrap();
        fs::write(dep_dir.join("main.td"), "// lib").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "local_lib".to_string(),
                    Dependency::Path {
                        path: "./local_lib".to_string(),
                    },
                );
                deps.insert(
                    "os".to_string(),
                    Dependency::Registry {
                        org: "taida-lang".to_string(),
                        name: "os".to_string(),
                        version: "a.1".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
        assert_eq!(result.resolved.len(), 2);
        assert!(result.resolved.contains_key("local_lib"));
        assert!(result.resolved.contains_key("os"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_transitive_path_dep_resolves_from_declaring_manifest_root() {
        let dir = PathBuf::from("/tmp/taida_test_transitive_path_context");
        let dep_a = dir.join("dep_a");
        let dep_b = dep_a.join("dep_b");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dep_b).unwrap();
        fs::write(dep_a.join("main.td"), "// dep_a").unwrap();
        fs::write(dep_b.join("main.td"), "// dep_b").unwrap();
        fs::write(
            dep_a.join("packages.tdm"),
            r#"
name <= "dep_a"
version <= "0.1.0"
deps <= @(
  dep_b <= @(path <= "./dep_b")
)
"#,
        )
        .unwrap();

        let manifest = Manifest {
            name: "root".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "dep_a".to_string(),
                    Dependency::Path {
                        path: "./dep_a".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
        let dep_b_path = result
            .resolved
            .get("dep_b")
            .expect("dep_b should be resolved transitively");
        assert_eq!(
            dep_b_path,
            &dep_b.canonicalize().unwrap(),
            "transitive path dependency must resolve relative to dep_a/packages.tdm"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_alias_conflict_same_name_different_identity_reports_error() {
        let dir = PathBuf::from("/tmp/taida_test_alias_conflict");
        let dep_a = dir.join("dep_a");
        let dep_c = dir.join("dep_c");
        let dep_a_shared = dep_a.join("shared_a");
        let dep_c_shared = dep_c.join("shared_c");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dep_a_shared).unwrap();
        fs::create_dir_all(&dep_c_shared).unwrap();
        fs::write(dep_a.join("main.td"), "// dep_a").unwrap();
        fs::write(dep_c.join("main.td"), "// dep_c").unwrap();
        fs::write(dep_a_shared.join("main.td"), "// shared from a").unwrap();
        fs::write(dep_c_shared.join("main.td"), "// shared from c").unwrap();

        fs::write(
            dep_a.join("packages.tdm"),
            r#"
name <= "dep_a"
deps <= @(
  shared <= @(path <= "./shared_a")
)
"#,
        )
        .unwrap();
        fs::write(
            dep_c.join("packages.tdm"),
            r#"
name <= "dep_c"
deps <= @(
  shared <= @(path <= "./shared_c")
)
"#,
        )
        .unwrap();

        let manifest = Manifest {
            name: "root".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "dep_a".to_string(),
                    Dependency::Path {
                        path: "./dep_a".to_string(),
                    },
                );
                deps.insert(
                    "dep_c".to_string(),
                    Dependency::Path {
                        path: "./dep_c".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("Dependency alias conflict") && e.contains("shared")),
            "expected alias conflict error, got: {:?}",
            result.errors
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_write_lockfile() {
        let dir = PathBuf::from("/tmp/taida_test_write_lockfile");
        let dep_dir = dir.join("dep_lib");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dep_dir).unwrap();
        fs::write(dep_dir.join("main.td"), "// lib").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "mylib".to_string(),
                    Dependency::Path {
                        path: "./dep_lib".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(result.errors.is_empty());

        write_lockfile(&manifest, &result).unwrap();

        let lock_path = dir.join(".taida").join("taida.lock");
        assert!(lock_path.exists(), ".taida/taida.lock should be created");

        let content = fs::read_to_string(&lock_path).unwrap();
        assert!(content.contains("mylib"));
        assert!(content.contains("path:./dep_lib"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_install_and_lockfile_core_bundled() {
        let dir = PathBuf::from("/tmp/taida_test_install_bundled");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "os".to_string(),
                    Dependency::Registry {
                        org: "taida-lang".to_string(),
                        name: "os".to_string(),
                        version: "a.1".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);

        install_deps(&manifest, &result).unwrap();
        write_lockfile(&manifest, &result).unwrap();

        // Check symlink exists
        let link = dir.join(".taida").join("deps").join("os");
        assert!(link.exists(), "os symlink should exist");

        // Check lockfile
        let lock_path = dir.join(".taida").join("taida.lock");
        assert!(lock_path.exists());
        let content = fs::read_to_string(&lock_path).unwrap();
        assert!(content.contains("os"));
        assert!(content.contains("bundled"));
        assert!(content.contains("a.1"));

        let _ = fs::remove_dir_all(&dir);
    }

    // ── FL-20 regression: resolve_deps_locked pins generation-only versions ──

    #[test]
    fn test_resolve_deps_locked_pins_generation_version() {
        use super::super::lockfile::{LockedPackage, Lockfile};

        let dir = PathBuf::from("/tmp/taida_test_resolve_locked");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Manifest declares os@a (generation-only)
        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "os".to_string(),
                    Dependency::Registry {
                        org: "taida-lang".to_string(),
                        name: "os".to_string(),
                        version: "a".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.clone(),
            exports: Vec::new(),
        };

        // Lockfile records os@a.1 (exact version)
        let lockfile = Lockfile {
            version: 1,
            packages: vec![LockedPackage {
                name: "os".to_string(),
                version: "a.1".to_string(),
                source: "bundled".to_string(),
                integrity: "fnv1a:0000000000000002".to_string(),
                addon: None,
            }],
        };

        // resolve_deps_locked should pin os to a.1 (from lockfile)
        let result = resolve_deps_locked(&manifest, &lockfile);
        assert!(result.errors.is_empty(), "Errors: {:?}", result.errors);
        assert_eq!(result.packages.len(), 1);
        assert_eq!(
            result.packages[0].version, "a.1",
            "Locked version should be used instead of re-resolving"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // ── BT-10: Package version boundary tests ──

    /// RAII guard for test directories — ensures cleanup even on panic.
    struct TestDir(PathBuf);
    impl TestDir {
        fn new(path: &str) -> Self {
            let p = PathBuf::from(path);
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &PathBuf {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_bt10_version_zero() {
        // Version "0.0.0" should be accepted
        let dir = TestDir::new("/tmp/taida_test_bt10_version_zero");
        let dep_dir = dir.path().join("dep_lib");
        fs::create_dir_all(&dep_dir).unwrap();
        fs::write(dep_dir.join("lib.td"), "// lib").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.0.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "mylib".to_string(),
                    Dependency::Path {
                        path: "./dep_lib".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.path().clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(
            result.errors.is_empty(),
            "Version 0.0.0 should be accepted: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_bt10_self_dependency() {
        // A package depending on itself should not cause infinite loop
        let dir = TestDir::new("/tmp/taida_test_bt10_self_dep");
        fs::write(dir.path().join("main.td"), "// self").unwrap();
        fs::write(
            dir.path().join("packages.tdm"),
            r#"
name <= "self_pkg"
version <= "0.1.0"
deps <= @(
  self_pkg <= @(path <= ".")
)
"#,
        )
        .unwrap();

        let manifest = Manifest {
            name: "self_pkg".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "self_pkg".to_string(),
                    Dependency::Path {
                        path: ".".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.path().clone(),
            exports: Vec::new(),
        };

        // Should either succeed (resolving to itself) or produce an error,
        // but must not hang or panic.
        let result = resolve_deps(&manifest);
        assert!(
            result.resolved.len() <= 1,
            "Self-dependency should resolve at most once"
        );
    }

    #[test]
    fn test_bt10_diamond_dependency() {
        // Diamond: root -> A -> shared, root -> B -> shared
        // Should resolve "shared" exactly once
        let dir = TestDir::new("/tmp/taida_test_bt10_diamond");
        let dep_a = dir.path().join("dep_a");
        let dep_b = dir.path().join("dep_b");
        let shared = dir.path().join("shared");
        fs::create_dir_all(&dep_a).unwrap();
        fs::create_dir_all(&dep_b).unwrap();
        fs::create_dir_all(&shared).unwrap();
        fs::write(dep_a.join("main.td"), "// dep_a").unwrap();
        fs::write(dep_b.join("main.td"), "// dep_b").unwrap();
        fs::write(shared.join("main.td"), "// shared").unwrap();

        fs::write(
            dep_a.join("packages.tdm"),
            r#"
name <= "dep_a"
deps <= @(
  shared <= @(path <= "../shared")
)
"#,
        )
        .unwrap();
        fs::write(
            dep_b.join("packages.tdm"),
            r#"
name <= "dep_b"
deps <= @(
  shared <= @(path <= "../shared")
)
"#,
        )
        .unwrap();

        let manifest = Manifest {
            name: "root".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "dep_a".to_string(),
                    Dependency::Path {
                        path: "./dep_a".to_string(),
                    },
                );
                deps.insert(
                    "dep_b".to_string(),
                    Dependency::Path {
                        path: "./dep_b".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.path().clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        assert!(
            result.errors.is_empty(),
            "Diamond dependency should resolve without errors: {:?}",
            result.errors
        );
        assert!(
            result.resolved.contains_key("dep_a"),
            "dep_a should be resolved"
        );
        assert!(
            result.resolved.contains_key("dep_b"),
            "dep_b should be resolved"
        );
        assert!(
            result.resolved.contains_key("shared"),
            "shared should be resolved"
        );
    }

    #[test]
    fn test_bt10_circular_dependency_a_b_a() {
        // Circular: A -> B -> A
        // Should not loop forever. Should either resolve or error.
        let dir = TestDir::new("/tmp/taida_test_bt10_circular");
        let dep_a = dir.path().join("dep_a");
        let dep_b = dir.path().join("dep_b");
        fs::create_dir_all(&dep_a).unwrap();
        fs::create_dir_all(&dep_b).unwrap();
        fs::write(dep_a.join("main.td"), "// dep_a").unwrap();
        fs::write(dep_b.join("main.td"), "// dep_b").unwrap();

        fs::write(
            dep_a.join("packages.tdm"),
            r#"
name <= "dep_a"
deps <= @(
  dep_b <= @(path <= "../dep_b")
)
"#,
        )
        .unwrap();
        fs::write(
            dep_b.join("packages.tdm"),
            r#"
name <= "dep_b"
deps <= @(
  dep_a <= @(path <= "../dep_a")
)
"#,
        )
        .unwrap();

        let manifest = Manifest {
            name: "root".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "dep_a".to_string(),
                    Dependency::Path {
                        path: "./dep_a".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.path().clone(),
            exports: Vec::new(),
        };

        // Must terminate (no infinite loop), regardless of whether it errors
        let result = resolve_deps(&manifest);
        assert!(
            result.resolved.contains_key("dep_a"),
            "dep_a should be resolved"
        );
        assert!(
            result.resolved.contains_key("dep_b"),
            "dep_b should be resolved"
        );
    }

    // ── RC-1q: Version coexistence (pnpm-style) ──

    #[test]
    fn test_rc1q_version_coexistence_path_conflict() {
        // Scenario: root -> dep_a -> utils@a.1 (via registry)
        //           root -> dep_b -> utils@b.1 (via registry)
        //           root -> utils@a.1 (direct dep)
        // The resolver should allow utils@a.1 and utils@b.1 to coexist,
        // storing the second version under "utils@b.1" key.
        //
        // We simulate this by using local path deps for dep_a and dep_b,
        // where each declares a registry dep on the same package name with
        // different versions. Since core-bundled packages have fixed versions,
        // we test the coexistence logic directly.
        let dir = TestDir::new("/tmp/taida_test_rc1q_coexist");
        let dep_a = dir.path().join("dep_a");
        let dep_b = dir.path().join("dep_b");
        let shared_v1 = dir.path().join("shared_v1");
        let shared_v2 = dir.path().join("shared_v2");
        fs::create_dir_all(&dep_a).unwrap();
        fs::create_dir_all(&dep_b).unwrap();
        fs::create_dir_all(&shared_v1).unwrap();
        fs::create_dir_all(&shared_v2).unwrap();
        fs::write(dep_a.join("main.td"), "// dep_a").unwrap();
        fs::write(dep_b.join("main.td"), "// dep_b").unwrap();
        fs::write(shared_v1.join("main.td"), "// shared v1").unwrap();
        fs::write(shared_v2.join("main.td"), "// shared v2").unwrap();

        // dep_a depends on "shared" pointing to shared_v1
        fs::write(
            dep_a.join("packages.tdm"),
            r#"
name <= "dep_a"
deps <= @(
  shared <= @(path <= "../shared_v1")
)
"#,
        )
        .unwrap();

        // dep_b depends on "shared" pointing to shared_v2 (different path = conflict)
        fs::write(
            dep_b.join("packages.tdm"),
            r#"
name <= "dep_b"
deps <= @(
  shared <= @(path <= "../shared_v2")
)
"#,
        )
        .unwrap();

        let manifest = Manifest {
            name: "root".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: {
                let mut deps = BTreeMap::new();
                deps.insert(
                    "dep_a".to_string(),
                    Dependency::Path {
                        path: "./dep_a".to_string(),
                    },
                );
                deps.insert(
                    "dep_b".to_string(),
                    Dependency::Path {
                        path: "./dep_b".to_string(),
                    },
                );
                deps
            },
            root_dir: dir.path().clone(),
            exports: Vec::new(),
        };

        let result = resolve_deps(&manifest);
        // Path deps produce an alias conflict error (version coexistence is registry-only)
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("alias conflict") && e.contains("shared")),
            "Path dep conflict should produce alias error, got: {:?}",
            result.errors
        );
        // Verify the improved error message mentions path deps
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("path dependencies")),
            "Error should mention path dependencies, got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_rc1q_version_coexistence_registry_normal() {
        // Scenario: root -> dep_a (path) -> shared@a.1 (registry-like, but simulated via path)
        //           root -> dep_b (path) -> shared@b.1 (registry-like, but simulated via path)
        // Since StoreProvider is a stub, we test the coexistence logic indirectly:
        // Both dep_a and dep_b declare "shared" as a path dep pointing to different dirs.
        // Path deps do NOT support coexistence (that's tested above).
        //
        // Instead, we verify that version-qualified directories can be resolved
        // correctly after install_deps creates them, which is the end-to-end
        // Registry coexistence outcome.
        let dir = TestDir::new("/tmp/taida_test_rc1q_coexist_registry_normal");
        let deps_dir = dir.path().join(".taida").join("deps");

        // Simulate what install_deps would create for coexisting registry packages:
        // .taida/deps/alice/utils/     -> version a.1 (unversioned = first resolved)
        // .taida/deps/alice/utils@b.1/ -> version b.1 (version-qualified = coexisting)
        let utils_v1 = deps_dir.join("alice").join("utils");
        let utils_v2 = deps_dir.join("alice").join("utils@b.1");
        fs::create_dir_all(&utils_v1).unwrap();
        fs::create_dir_all(&utils_v2).unwrap();
        fs::write(utils_v1.join("main.td"), "// utils a.1").unwrap();
        fs::write(utils_v2.join("main.td"), "// utils b.1").unwrap();
        fs::write(
            utils_v1.join("packages.tdm"),
            "name <= \"utils\"\nversion <= \"a.1\"\n",
        )
        .unwrap();
        fs::write(
            utils_v2.join("packages.tdm"),
            "name <= \"utils\"\nversion <= \"b.1\"\n",
        )
        .unwrap();

        // Unversioned resolution: finds alice/utils (v1)
        let res = resolve_package_module(dir.path(), "alice/utils");
        assert!(res.is_some(), "Should find unversioned package");
        assert_eq!(res.unwrap().pkg_dir, utils_v1);

        // Versioned resolution: finds alice/utils@b.1 (v2)
        let res = resolve_package_import_versioned(dir.path(), "alice/utils", "b.1");
        assert!(res.is_some(), "Should find version-qualified package");
        assert_eq!(res.unwrap(), utils_v2);

        // Versioned resolution for v1: no alice/utils@a.1 dir, falls back to unversioned
        let res = resolve_package_import_versioned(dir.path(), "alice/utils", "a.1");
        assert!(res.is_some(), "Should fall back to unversioned package");
        assert_eq!(res.unwrap(), utils_v1);
    }

    #[test]
    fn test_rc1q_resolve_package_import_versioned() {
        // Test the versioned package import resolution function
        let dir = TestDir::new("/tmp/taida_test_rc1q_import_versioned");
        let deps_dir = dir.path().join(".taida").join("deps");

        // Create unversioned package directory
        let unversioned = deps_dir.join("alice").join("http");
        fs::create_dir_all(&unversioned).unwrap();
        fs::write(unversioned.join("main.td"), "// http v1").unwrap();

        // Create version-qualified package directory
        let versioned = deps_dir.join("alice").join("http@b.12");
        fs::create_dir_all(&versioned).unwrap();
        fs::write(versioned.join("main.td"), "// http v2").unwrap();

        // Versioned resolution should find the version-qualified directory
        let result = resolve_package_import_versioned(dir.path(), "alice/http", "b.12");
        assert!(result.is_some(), "Should find versioned package");
        assert_eq!(result.unwrap(), versioned);

        // Versioned resolution for a non-existent version should fall back to unversioned
        let result = resolve_package_import_versioned(dir.path(), "alice/http", "c.1");
        assert!(result.is_some(), "Should fall back to unversioned package");
        assert_eq!(result.unwrap(), unversioned);

        // Unversioned resolution should still find the unversioned directory
        let result = resolve_package_import(dir.path(), "alice/http");
        assert!(result.is_some(), "Unversioned resolution should work");
        assert_eq!(result.unwrap(), unversioned);
    }

    #[test]
    fn test_rc1q_resolve_package_module_versioned() {
        // Test versioned package module resolution with longest-prefix matching
        let dir = TestDir::new("/tmp/taida_test_rc1q_module_versioned");
        let deps_dir = dir.path().join(".taida").join("deps");

        // Create version-qualified package directory
        let versioned = deps_dir.join("alice").join("http@b.12");
        fs::create_dir_all(&versioned).unwrap();
        fs::write(versioned.join("main.td"), "// http v2").unwrap();

        // Also create unversioned
        let unversioned = deps_dir.join("alice").join("http");
        fs::create_dir_all(&unversioned).unwrap();
        fs::write(unversioned.join("main.td"), "// http v1").unwrap();

        // Versioned resolution should prefer the version-qualified directory
        let res = resolve_package_module_versioned(dir.path(), "alice/http", "b.12");
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.pkg_dir, versioned);
        assert!(res.submodule.is_none());

        // Non-existent version should fall back to unversioned
        let res = resolve_package_module_versioned(dir.path(), "alice/http", "c.1");
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.pkg_dir, unversioned);
        assert!(res.submodule.is_none());

        // Submodule within versioned package
        let res = resolve_package_module_versioned(dir.path(), "alice/http/router", "b.12");
        assert!(res.is_some());
        let res = res.unwrap();
        assert_eq!(res.pkg_dir, versioned);
        assert_eq!(res.submodule.as_deref(), Some("router"));
    }

    #[test]
    fn test_rc1q_lockfile_with_version_qualified_names() {
        // Verify lockfile correctly handles version-qualified package names
        use super::super::lockfile::Lockfile;
        use super::super::provider::PackageSource;

        let _dir = TestDir::new("/tmp/taida_test_rc1q_lockfile");

        let packages = vec![
            ResolvedPackage {
                name: "alice/http".to_string(),
                version: "a.1".to_string(),
                source: PackageSource::Store {
                    org: "alice".to_string(),
                    name: "http".to_string(),
                },
                path: PathBuf::from("/tmp/test/a1"),
                integrity: "fnv1a:0000000000000001".to_string(),
            },
            ResolvedPackage {
                name: "alice/http@b.12".to_string(),
                version: "b.12".to_string(),
                source: PackageSource::Store {
                    org: "alice".to_string(),
                    name: "http".to_string(),
                },
                path: PathBuf::from("/tmp/test/b12"),
                integrity: "fnv1a:0000000000000002".to_string(),
            },
        ];

        let lockfile = Lockfile::from_resolved(&packages);
        let serialized = lockfile.to_string();

        // Should contain both entries
        assert!(
            serialized.contains("alice/http\"") || serialized.contains("alice/http\n"),
            "Should contain unversioned entry"
        );
        assert!(
            serialized.contains("alice/http@b.12"),
            "Should contain version-qualified entry"
        );

        // Roundtrip
        let parsed = Lockfile::parse(&serialized).unwrap();
        assert_eq!(
            parsed.packages.len(),
            2,
            "Both versions should be preserved"
        );

        // Verify both are up to date
        assert!(lockfile.is_up_to_date(&packages));
    }

    #[test]
    fn test_rc1q_install_versioned_deps() {
        // Test that version-qualified packages are installed to version-qualified directories
        let dir = TestDir::new("/tmp/taida_test_rc1q_install_versioned");
        let pkg_dir_v1 = dir.path().join("pkg_v1");
        let pkg_dir_v2 = dir.path().join("pkg_v2");
        fs::create_dir_all(&pkg_dir_v1).unwrap();
        fs::create_dir_all(&pkg_dir_v2).unwrap();
        fs::write(pkg_dir_v1.join("main.td"), "// v1").unwrap();
        fs::write(pkg_dir_v2.join("main.td"), "// v2").unwrap();

        let manifest = Manifest {
            name: "test".to_string(),
            version: "0.1.0".to_string(),
            description: String::new(),
            entry: "main.td".to_string(),
            deps: BTreeMap::new(),
            root_dir: dir.path().clone(),
            exports: Vec::new(),
        };

        // Simulate resolved packages with version-qualified names
        let result = ResolveResult {
            resolved: {
                let mut m = BTreeMap::new();
                m.insert(
                    "mylib".to_string(),
                    pkg_dir_v1.canonicalize().unwrap_or(pkg_dir_v1.clone()),
                );
                m.insert(
                    "mylib@b.1".to_string(),
                    pkg_dir_v2.canonicalize().unwrap_or(pkg_dir_v2.clone()),
                );
                m
            },
            packages: vec![
                ResolvedPackage {
                    name: "mylib".to_string(),
                    version: "a.1".to_string(),
                    source: PackageSource::Path("./pkg_v1".to_string()),
                    path: pkg_dir_v1.canonicalize().unwrap_or(pkg_dir_v1),
                    integrity: "fnv1a:0000000000000001".to_string(),
                },
                ResolvedPackage {
                    name: "mylib@b.1".to_string(),
                    version: "b.1".to_string(),
                    source: PackageSource::Path("./pkg_v2".to_string()),
                    path: pkg_dir_v2.canonicalize().unwrap_or(pkg_dir_v2),
                    integrity: "fnv1a:0000000000000002".to_string(),
                },
            ],
            errors: Vec::new(),
        };

        install_deps(&manifest, &result).unwrap();

        // Check both symlinks exist
        let link_v1 = dir.path().join(".taida").join("deps").join("mylib");
        let link_v2 = dir.path().join(".taida").join("deps").join("mylib@b.1");
        assert!(link_v1.exists(), "Unversioned symlink should exist");
        assert!(link_v2.exists(), "Version-qualified symlink should exist");
        assert!(
            link_v1.join("main.td").exists(),
            "v1 main.td should be accessible"
        );
        assert!(
            link_v2.join("main.td").exists(),
            "v2 main.td should be accessible"
        );
    }

    #[test]
    fn test_rc1q_multi_level_coexistence_resolution() {
        // Test multi-level coexistence: A@v1 -> B@v1, A@v2 -> B@v2.
        // After install_deps, the directory structure would be:
        //   .taida/deps/alice/a/       (A v1, unversioned)
        //   .taida/deps/alice/a@v2/    (A v2, version-qualified)
        //   .taida/deps/bob/b/         (B v1, unversioned)
        //   .taida/deps/bob/b@v2/      (B v2, version-qualified)
        //
        // We verify that each version resolves to the correct directory.
        let dir = TestDir::new("/tmp/taida_test_rc1q_multi_level_coexist");
        let deps_dir = dir.path().join(".taida").join("deps");

        // Create directory structure for two packages, two versions each
        let a_v1 = deps_dir.join("alice").join("a");
        let a_v2 = deps_dir.join("alice").join("a@v2");
        let b_v1 = deps_dir.join("bob").join("b");
        let b_v2 = deps_dir.join("bob").join("b@v2");

        for d in [&a_v1, &a_v2, &b_v1, &b_v2] {
            fs::create_dir_all(d).unwrap();
            fs::write(d.join("main.td"), format!("// {}", d.display())).unwrap();
        }

        // A@v1 depends on B@v1 (unversioned), A@v2 depends on B@v2 (version-qualified)
        fs::write(
            a_v1.join("packages.tdm"),
            "name <= \"a\"\nversion <= \"v1\"\ndeps <= @(\n  bob/b <= @(path <= \"../../bob/b\")\n)\n",
        ).unwrap();
        fs::write(
            a_v2.join("packages.tdm"),
            "name <= \"a\"\nversion <= \"v2\"\ndeps <= @(\n  bob/b <= @(path <= \"../../bob/b@v2\")\n)\n",
        ).unwrap();

        // Verify each package version resolves correctly
        let res = resolve_package_import_versioned(dir.path(), "alice/a", "v1");
        assert!(res.is_some(), "A@v1 should fall back to unversioned");
        assert_eq!(res.unwrap(), a_v1);

        let res = resolve_package_import_versioned(dir.path(), "alice/a", "v2");
        assert!(res.is_some(), "A@v2 should find version-qualified dir");
        assert_eq!(res.unwrap(), a_v2);

        let res = resolve_package_import_versioned(dir.path(), "bob/b", "v1");
        assert!(res.is_some(), "B@v1 should fall back to unversioned");
        assert_eq!(res.unwrap(), b_v1);

        let res = resolve_package_import_versioned(dir.path(), "bob/b", "v2");
        assert!(res.is_some(), "B@v2 should find version-qualified dir");
        assert_eq!(res.unwrap(), b_v2);

        // Cross-check: unversioned resolution always returns the base directory
        let res = resolve_package_module(dir.path(), "alice/a");
        assert!(res.is_some());
        assert_eq!(res.unwrap().pkg_dir, a_v1);

        let res = resolve_package_module(dir.path(), "bob/b");
        assert!(res.is_some());
        assert_eq!(res.unwrap().pkg_dir, b_v1);
    }
}
