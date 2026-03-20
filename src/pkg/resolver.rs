/// Common Package Resolver for Taida Lang.
///
/// Resolves all dependencies using the Provider pattern:
/// 1. WorkspaceProvider — local path dependencies
/// 2. CoreBundledProvider — core packages bundled with Taida (`taida-lang/*`)
/// 3. StoreProvider — external registry (stub, Phase 3+)
///
/// `taida deps` and `taida install` read `packages.tdm`, resolve all dependencies
/// through the provider chain, and create `.taida/deps/` symlinks.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::lockfile::Lockfile;
use super::manifest::{Dependency, Manifest};
use super::provider::{
    CoreBundledProvider, PackageProvider, PackageSource, ProviderResult, ResolvedPackage,
    StoreProvider, WorkspaceProvider,
};

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

/// Resolve all dependencies declared in the manifest using the provider chain.
///
/// Tries each provider in order for each dependency:
/// WorkspaceProvider -> CoreBundledProvider -> StoreProvider
pub fn resolve_deps(manifest: &Manifest) -> ResolveResult {
    resolve_deps_inner(manifest, false, None)
}

/// Resolve all dependencies using the provider chain, but pin generation-only
/// versions to their locked exact versions from an existing lockfile.
///
/// Used by `taida install` to ensure reproducible installs: if a lockfile
/// records `alice/demo@a.2`, a manifest dependency of `alice/demo@a` will
/// resolve to `a.2` instead of the latest version in generation `a`.
pub fn resolve_deps_locked(manifest: &Manifest, lockfile: &Lockfile) -> ResolveResult {
    resolve_deps_inner(manifest, false, Some(lockfile))
}

/// Resolve all dependencies, bypassing local cache for generation resolution.
///
/// Used by `taida update` to re-resolve generation-only versions (e.g. "a")
/// to the latest exact version (e.g. "a.47") by querying GitHub API directly.
pub fn resolve_deps_update(manifest: &Manifest) -> ResolveResult {
    resolve_deps_inner(manifest, true, None)
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

    let providers: Vec<Box<dyn PackageProvider>> = vec![
        Box::new(WorkspaceProvider),
        Box::new(CoreBundledProvider::new()),
        if force_remote {
            Box::new(StoreProvider::new_force_remote())
        } else {
            Box::new(StoreProvider::new())
        },
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
    let deps_dir = manifest.root_dir.join(".taida").join("deps");

    // Create deps directory
    std::fs::create_dir_all(&deps_dir).map_err(|e| format!("Cannot create .taida/deps/: {}", e))?;

    // Clean existing deps dir completely (supports nested org/name structure)
    if deps_dir.exists() {
        let _ = std::fs::remove_dir_all(&deps_dir);
    }
    std::fs::create_dir_all(&deps_dir).map_err(|e| format!("Cannot create .taida/deps/: {}", e))?;

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
                if let Err(e) = provider.install(pkg, &deps_dir) {
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
        };

        // Lockfile records os@a.1 (exact version)
        let lockfile = Lockfile {
            version: 1,
            packages: vec![LockedPackage {
                name: "os".to_string(),
                version: "a.1".to_string(),
                source: "bundled".to_string(),
                integrity: "fnv1a:0000000000000002".to_string(),
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
