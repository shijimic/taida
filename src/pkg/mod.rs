// RC2.7 Phase 2: GitHub Release REST API driver.
pub mod api_diff;
pub mod facade;
#[cfg(feature = "community")]
pub mod github_release;
pub mod init;
pub mod lockfile;
/// Package management for Taida Lang.
///
/// Provides `taida init`, `taida ingot deps`, `taida ingot install`, and `taida ingot publish` commands.
/// Package definition uses `packages.tdm` in Taida's own syntax.
///
/// ## Architecture (Common Package Resolver)
///
/// Dependencies are resolved through a provider chain:
/// 1. **WorkspaceProvider** — local path dependencies (`Dependency::Path`)
/// 2. **CoreBundledProvider** — core packages bundled with Taida (`taida-lang/*`)
/// 3. **StoreProvider** — external registry packages (stub, Phase 3+)
///
/// Resolved dependencies are recorded in `.taida/taida.lock` for reproducibility.
pub mod manifest;
pub mod provider;
pub mod publish;
pub mod resolver;
pub mod store;
