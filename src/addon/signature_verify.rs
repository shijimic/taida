//! C26B-030 / SEC-011 — install-side signature verification wiring.
//!
//! Closes the install-side half of the Sigstore / SLSA supply-chain
//! gate. The release side (`.github/workflows/release.yml` — the
//! `sign` and `provenance` jobs) produces `.cosign.bundle` files
//! alongside every official artefact; this module is the hook that
//! consumes them inside `taida ingot install` so the signature matters.
//!
//! # Policy matrix
//!
//! | `TAIDA_VERIFY_SIGNATURES` | Official URL | Behaviour                        |
//! |---------------------------|--------------|----------------------------------|
//! | unset / empty             | yes          | `BestEffort` (warn if missing)   |
//! | unset / empty             | no           | `Disabled`                       |
//! | `0` / `off` / `false`     | any          | `Disabled`                       |
//! | `1` / `on` / `best-effort`| any          | `BestEffort`                     |
//! | `required` / `enforce`    | any          | `Required` (hard fail on gap)    |
//!
//! # Trust boundary
//!
//! The heavy-lifting cryptography is performed by `cosign verify-blob`
//! via the already-audited `scripts/release/verify-signatures.sh`
//! (C26B-007 Sub-phase 7.4). This module only handles:
//!
//! 1. deciding whether verification should run at all for a given URL,
//! 2. fetching the `.cosign.bundle` file next to the artefact,
//! 3. driving `cosign` as a child process,
//! 4. mapping its exit code back into a structured policy verdict.
//!
//! Rewriting the Rekor + Fulcio + DSSE verification path in Rust would
//! duplicate `cosign` 1:1 and introduce another parsing surface that
//! would itself need audit. The `cosign` binary is the verification
//! truth; we only wire it up.
//!
//! Tests use a temporary fake `cosign` executable on `PATH` so the
//! same process-spawn path is exercised without requiring cosign to be
//! installed on every developer laptop / CI machine.

use std::path::{Path, PathBuf};
use std::process::Command;

// ── Policy ──────────────────────────────────────────────────────

/// What to do when the signature for an official artefact is missing
/// or cannot be checked (e.g. `cosign` is not on `$PATH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyPolicy {
    /// Never look at signatures. Used for non-official URLs and when
    /// `TAIDA_VERIFY_SIGNATURES=0`.
    Disabled,
    /// Look at signatures when the bundle + `cosign` are available,
    /// warn if they are not, but do not fail the install. Default
    /// for first-party URLs when the env flag is unset.
    BestEffort,
    /// Treat a missing bundle / missing `cosign` / failed verification
    /// as a hard error. Set `TAIDA_VERIFY_SIGNATURES=required` on
    /// CI / paranoid production installs.
    Required,
}

impl VerifyPolicy {
    /// Resolve the policy for a single fetch, given the artefact URL.
    ///
    /// - `TAIDA_VERIFY_SIGNATURES=required` → [`VerifyPolicy::Required`]
    ///   for every URL (including `file://` — used by the failing-
    ///   path integration test).
    /// - `TAIDA_VERIFY_SIGNATURES=0` / `off` / `false` →
    ///   [`VerifyPolicy::Disabled`] unconditionally.
    /// - `TAIDA_VERIFY_SIGNATURES=1` / `on` / `best-effort` →
    ///   [`VerifyPolicy::BestEffort`] unconditionally.
    /// - Unset / empty:
    ///   - [`is_official_release_url`] returns `true` → [`BestEffort`]
    ///   - otherwise → [`Disabled`]
    pub fn resolve(url: &str) -> Self {
        match std::env::var("TAIDA_VERIFY_SIGNATURES")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("required") | Some("REQUIRED") | Some("enforce") | Some("ENFORCE") => {
                Self::Required
            }
            Some("0") | Some("off") | Some("OFF") | Some("false") | Some("FALSE") => Self::Disabled,
            Some("1") | Some("on") | Some("ON") | Some("true") | Some("TRUE")
            | Some("best-effort") | Some("BEST-EFFORT") => Self::BestEffort,
            _ => {
                if is_official_release_url(url) {
                    Self::BestEffort
                } else {
                    Self::Disabled
                }
            }
        }
    }
}

// ── Detection of official URLs ──────────────────────────────────

/// Returns true when `url` looks like a GitHub release asset published
/// by an official `taida-lang/*` repository. Official release assets
/// are the only artefacts signed by the `release.yml` workflow, so
/// only their downloads benefit from signature verification.
///
/// The matcher is intentionally broad (any https URL whose host is
/// `github.com` and whose path starts with `/taida-lang/`) so
/// first-party addons (`taida-lang/terminal`, future first-party
/// addons) pick up verification automatically.
pub fn is_official_release_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    let rest = &url["https://".len()..];
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h, p),
        None => return false,
    };
    matches!(host, "github.com" | "www.github.com")
        && path.starts_with("taida-lang/")
        && path.contains("/releases/")
}

// ── Outcomes ────────────────────────────────────────────────────

/// Verdict from a single verification attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    /// Policy was [`VerifyPolicy::Disabled`] — we did not look.
    Skipped,
    /// Policy was [`VerifyPolicy::BestEffort`] and no bundle was
    /// found (or `cosign` was missing). A warning was emitted to
    /// the caller; the install should proceed.
    Warned(String),
    /// `cosign verify-blob` (or the test stub) returned success.
    Verified,
}

/// Reason a verification failed when the policy demanded success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// Policy was [`VerifyPolicy::Required`] and the `.cosign.bundle`
    /// file was missing or could not be fetched.
    BundleMissing(String),
    /// Policy was [`VerifyPolicy::Required`] and the `cosign` binary
    /// was not available on `PATH`.
    CosignUnavailable,
    /// `cosign verify-blob` returned a non-zero exit code.
    SignatureRejected { stderr: String },
    /// Something in the process invocation itself went wrong (spawn,
    /// I/O) — distinct from a cosign-reported signature failure so
    /// operators can distinguish infra breakage from attack.
    InvocationError(String),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BundleMissing(detail) => {
                write!(
                    f,
                    "SEC-011 signature bundle missing for required verification: {detail}"
                )
            }
            Self::CosignUnavailable => {
                write!(
                    f,
                    "SEC-011 required verification requested but cosign binary is not on PATH; \
                     install cosign or set TAIDA_VERIFY_SIGNATURES=best-effort"
                )
            }
            Self::SignatureRejected { stderr } => {
                write!(
                    f,
                    "SEC-011 cosign rejected the signature: {}",
                    stderr.trim()
                )
            }
            Self::InvocationError(detail) => {
                write!(f, "SEC-011 verification invocation error: {detail}")
            }
        }
    }
}

impl std::error::Error for VerifyError {}

// ── Bundle fetch ────────────────────────────────────────────────

/// Convert an artefact URL into the companion bundle URL by appending
/// `.cosign.bundle`. The release workflow always co-locates the two.
pub fn bundle_url_for(url: &str) -> String {
    format!("{url}.cosign.bundle")
}

/// Returns the on-disk path where the `.cosign.bundle` for
/// `artifact` should live (same directory, `<artifact>.cosign.bundle`).
pub fn bundle_path_for(artifact: &Path) -> PathBuf {
    let mut out = artifact.as_os_str().to_os_string();
    out.push(".cosign.bundle");
    PathBuf::from(out)
}

/// Try to fetch the bundle into `dest`. Returns `Ok(true)` on success,
/// `Ok(false)` if the bundle does not exist upstream (HTTP 404 /
/// missing file), and `Err` for real I/O errors that should be
/// surfaced.
///
/// The implementation intentionally mirrors
/// [`prebuild_fetcher::download_from_url`] but without the SHA-256
/// gating (the bundle's integrity is not known ahead of time; cosign
/// itself verifies the bundle against the transparency log).
pub fn fetch_bundle(src_url: &str, dest: &Path) -> Result<bool, VerifyError> {
    if let Some(path) = src_url.strip_prefix("file://") {
        match std::fs::read(path) {
            Ok(data) => {
                std::fs::write(dest, data).map_err(|e| {
                    VerifyError::InvocationError(format!("cannot write bundle to {dest:?}: {e}"))
                })?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(VerifyError::InvocationError(format!(
                "cannot read bundle file://{path}: {e}"
            ))),
        }
    } else if src_url.starts_with("https://") {
        fetch_bundle_https(src_url, dest)
    } else {
        Err(VerifyError::InvocationError(format!(
            "unsupported bundle URL scheme: {src_url}"
        )))
    }
}

#[cfg(feature = "community")]
fn fetch_bundle_https(src_url: &str, dest: &Path) -> Result<bool, VerifyError> {
    use reqwest::StatusCode;
    use reqwest::blocking::Client;

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| VerifyError::InvocationError(format!("cannot build HTTP client: {e}")))?;
    let resp = client
        .get(src_url)
        .send()
        .map_err(|e| VerifyError::InvocationError(format!("bundle fetch failed: {e}")))?;
    let status = resp.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !status.is_success() {
        return Err(VerifyError::InvocationError(format!(
            "bundle fetch returned HTTP {status}"
        )));
    }
    let bytes = resp
        .bytes()
        .map_err(|e| VerifyError::InvocationError(format!("bundle body read failed: {e}")))?;
    std::fs::write(dest, &bytes).map_err(|e| {
        VerifyError::InvocationError(format!("cannot write bundle to {dest:?}: {e}"))
    })?;
    Ok(true)
}

#[cfg(not(feature = "community"))]
fn fetch_bundle_https(_src_url: &str, _dest: &Path) -> Result<bool, VerifyError> {
    // Without the `community` feature we treat HTTPS bundle fetches as
    // "bundle not available" rather than failing hard — this keeps
    // unit-test / no-network builds working. Required-policy callers
    // still get a deterministic error downstream when the bundle
    // check reports missing.
    Ok(false)
}

// ── cosign invocation ───────────────────────────────────────────

/// Identity regex the release workflow signs under. Mirrors
/// `scripts/release/verify-signatures.sh::COSIGN_IDENTITY_REGEXP`
/// default.
const COSIGN_IDENTITY_REGEXP: &str = "^https://github.com/taida-lang/";
/// OIDC issuer used by GitHub Actions workflows — pinned literal.
const COSIGN_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";

/// Run `cosign verify-blob` against `artifact` using the bundle at
/// `bundle_path`. Returns `Ok(())` on verified, `Err` otherwise.
///
/// Production flow never honours a test bypass environment variable:
/// it always resolves and executes `cosign` from `PATH`.
pub fn run_cosign_verify(artifact: &Path, bundle_path: &Path) -> Result<(), VerifyError> {
    // Detect cosign availability explicitly so the `Required`-policy
    // error path can surface `CosignUnavailable` distinctly from
    // `SignatureRejected`.
    let which = Command::new("cosign").arg("version").output();
    match which {
        Ok(o) if o.status.success() => {}
        _ => return Err(VerifyError::CosignUnavailable),
    }

    let output = Command::new("cosign")
        .arg("verify-blob")
        .arg("--bundle")
        .arg(bundle_path)
        .arg("--certificate-identity-regexp")
        .arg(COSIGN_IDENTITY_REGEXP)
        .arg("--certificate-oidc-issuer")
        .arg(COSIGN_OIDC_ISSUER)
        .arg(artifact)
        .output()
        .map_err(|e| VerifyError::InvocationError(format!("cannot spawn cosign: {e}")))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(VerifyError::SignatureRejected {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

// ── Top-level driver ────────────────────────────────────────────

/// End-to-end verification of one artefact: pick policy, fetch the
/// bundle if needed, and drive `cosign`. Returns the structured
/// verdict so `install` can decide whether to log, warn, or abort.
///
/// `policy` normally comes from [`VerifyPolicy::resolve`]; callers
/// may override it for specific subcommand flags.
pub fn verify_artifact(
    artifact: &Path,
    artifact_url: &str,
    policy: VerifyPolicy,
) -> Result<VerifyOutcome, VerifyError> {
    if matches!(policy, VerifyPolicy::Disabled) {
        return Ok(VerifyOutcome::Skipped);
    }

    let bundle_path = bundle_path_for(artifact);
    if !bundle_path.exists() {
        let bundle_url = bundle_url_for(artifact_url);
        let fetched = fetch_bundle(&bundle_url, &bundle_path);
        match fetched {
            Ok(true) => {}
            Ok(false) => {
                let msg = format!(
                    "cosign bundle missing at {bundle_url} (expected alongside {artifact_url})"
                );
                return match policy {
                    VerifyPolicy::Required => Err(VerifyError::BundleMissing(msg)),
                    VerifyPolicy::BestEffort => Ok(VerifyOutcome::Warned(msg)),
                    VerifyPolicy::Disabled => Ok(VerifyOutcome::Skipped),
                };
            }
            Err(e) => {
                return match policy {
                    VerifyPolicy::Required => Err(e),
                    VerifyPolicy::BestEffort => Ok(VerifyOutcome::Warned(e.to_string())),
                    VerifyPolicy::Disabled => Ok(VerifyOutcome::Skipped),
                };
            }
        }
    }

    match run_cosign_verify(artifact, &bundle_path) {
        Ok(()) => Ok(VerifyOutcome::Verified),
        Err(VerifyError::CosignUnavailable) => match policy {
            VerifyPolicy::Required => Err(VerifyError::CosignUnavailable),
            VerifyPolicy::BestEffort => Ok(VerifyOutcome::Warned(
                "cosign binary not on PATH; skipping SEC-011 verification (set \
                 TAIDA_VERIFY_SIGNATURES=required to fail hard)"
                    .to_string(),
            )),
            VerifyPolicy::Disabled => Ok(VerifyOutcome::Skipped),
        },
        Err(e @ VerifyError::SignatureRejected { .. }) => Err(e),
        Err(e) => match policy {
            VerifyPolicy::Required => Err(e),
            VerifyPolicy::BestEffort => Ok(VerifyOutcome::Warned(e.to_string())),
            VerifyPolicy::Disabled => Ok(VerifyOutcome::Skipped),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn official_url_detection() {
        assert!(is_official_release_url(
            "https://github.com/taida-lang/terminal/releases/download/@a.7/libterminal.so"
        ));
        assert!(is_official_release_url(
            "https://github.com/taida-lang/taida/releases/download/@c.26/taida-linux.tar.gz"
        ));
        assert!(!is_official_release_url(
            "https://github.com/some-fork/terminal/releases/download/@a.7/libterminal.so"
        ));
        assert!(!is_official_release_url(
            "http://github.com/taida-lang/terminal/releases/download/@a.7/libterminal.so"
        ));
        assert!(!is_official_release_url("file:///tmp/libterminal.so"));
    }

    #[test]
    fn policy_resolution_defaults() {
        // Env guard: avoid stomping on parallel tests or external flag.
        let guard = EnvGuard::unset("TAIDA_VERIFY_SIGNATURES");
        assert_eq!(
            VerifyPolicy::resolve(
                "https://github.com/taida-lang/terminal/releases/download/@a.7/x.so"
            ),
            VerifyPolicy::BestEffort
        );
        assert_eq!(
            VerifyPolicy::resolve("https://example.org/x.so"),
            VerifyPolicy::Disabled
        );
        drop(guard);
    }

    #[test]
    fn policy_resolution_env_overrides() {
        let _g = EnvGuard::set("TAIDA_VERIFY_SIGNATURES", "required");
        assert_eq!(
            VerifyPolicy::resolve("https://example.org/x.so"),
            VerifyPolicy::Required
        );
        drop(_g);

        let _g = EnvGuard::set("TAIDA_VERIFY_SIGNATURES", "0");
        assert_eq!(
            VerifyPolicy::resolve(
                "https://github.com/taida-lang/terminal/releases/download/@a.7/x.so"
            ),
            VerifyPolicy::Disabled
        );
        drop(_g);

        let _g = EnvGuard::set("TAIDA_VERIFY_SIGNATURES", "best-effort");
        assert_eq!(
            VerifyPolicy::resolve("https://example.org/x.so"),
            VerifyPolicy::BestEffort
        );
        drop(_g);
    }

    #[test]
    fn bundle_paths() {
        assert_eq!(
            bundle_url_for("https://example.org/x.tar.gz"),
            "https://example.org/x.tar.gz.cosign.bundle"
        );
        assert_eq!(
            bundle_path_for(Path::new("/tmp/x.tar.gz")),
            PathBuf::from("/tmp/x.tar.gz.cosign.bundle")
        );
    }

    // ── env guard (tests run in parallel, but these three tests
    // lock the same env var so serial ordering is enforced at the
    // `#[test]` level via cargo nextest's serial detection for
    // envvar mutations) ──

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let lock = match ENV_LOCK.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let prev = std::env::var(key).ok();
            // SAFETY: tests for SEC-011 env parsing; production code
            // never writes this env var.
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                key,
                prev,
                _lock: lock,
            }
        }

        fn unset(key: &'static str) -> Self {
            let lock = match ENV_LOCK.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let prev = std::env::var(key).ok();
            // SAFETY: as above.
            unsafe {
                std::env::remove_var(key);
            }
            Self {
                key,
                prev,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: restores the env to its pre-test value.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }
}
