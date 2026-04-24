//! C26B-030 / SEC-011 — install-side Sigstore signature verification.
//!
//! Release-side signing (Round 2 / wB, `.github/workflows/release.yml`
//! `sign` + `provenance` jobs) already lands every artefact with a
//! `.cosign.bundle` file. This test pins the **install side** of the
//! pipeline so a missing / rejected bundle fails hard when policy is
//! `Required`, and is a warning-only best-effort skip otherwise. The
//! review finding (C26 2026-04-25) was that the installer never called
//! `scripts/release/verify-signatures.sh` nor any in-process cosign
//! wrapper, so a compromised mirror could have swapped the binary out
//! without triggering the release-workflow's signature gate.
//!
//! The test exercises the full decision graph of
//! `src/addon/signature_verify.rs` directly (the module is covered
//! per-function in unit tests; these cases pin the acceptance contract
//! the C26B-030 blocker demands):
//!
//! 1. `Disabled` policy → never even looks at a missing bundle.
//! 2. `BestEffort` policy + missing bundle → `Warned` outcome, no panic.
//! 3. `Required` policy + missing bundle → hard `VerifyError::BundleMissing`.
//! 4. `Required` policy + bundle present + fake `ok` → `Verified`.
//! 5. `Required` policy + bundle present + fake `fail` →
//!    `VerifyError::SignatureRejected`.
//! 6. `Required` policy + bundle present + fake `missing_cosign` →
//!    `VerifyError::CosignUnavailable`.
//!
//! A temporary fake `cosign` executable on `PATH` lets the test drive
//! cosign's own exit code without requiring the real binary on every
//! CI runner. This intentionally exercises the same production
//! `Command::new("cosign")` path; there is no environment-variable
//! bypass inside the verifier.
//!
//! This test is the regression guard for the C26B-030 acceptance:
//!   - signature-bundle-missing rejects install when policy is hard
//!   - signature-verify passes under fake-ok
//!   - signature-verify rejects install under fake-fail
//!
//! It also doubles as the "unit or integration test" the blocker
//! requires at the install layer, alongside the per-function tests
//! already living in `src/addon/signature_verify.rs::tests`.

#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use taida::addon::signature_verify::{
    VerifyError, VerifyOutcome, VerifyPolicy, bundle_path_for, bundle_url_for,
    is_official_release_url, verify_artifact,
};

// ── env-var serialisation guard ─────────────────────────────────
//
// The module honours `TAIDA_VERIFY_SIGNATURES`; these tests also
// mutate `PATH` to inject a fake `cosign`. Multiple `#[test]`s inside
// this file would race on those envs if they ran in parallel, so we
// gate the whole section behind a process-global mutex.

use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn with_env_guard<F: FnOnce()>(f: F) {
    // poisoned-lock safe: we only need serialised access.
    let _g = match ENV_LOCK.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let prev_path = std::env::var("PATH").ok();
    // Always start from a known-clean env. Unsafe because
    // `std::env::remove_var` is unsafe on 1.74+; tests are the
    // only caller that writes this var.
    unsafe {
        std::env::remove_var("TAIDA_VERIFY_SIGNATURES");
    }
    f();
    unsafe {
        std::env::remove_var("TAIDA_VERIFY_SIGNATURES");
        match prev_path {
            Some(path) => std::env::set_var("PATH", path),
            None => std::env::remove_var("PATH"),
        }
    }
}

// Tempdir that auto-cleans on drop so failing assertions do not leak.
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("c26b030_{tag}_{}_{}", std::process::id(), nanos));
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_artifact(dir: &Path, name: &str, body: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, body).unwrap();
    p
}

fn install_fake_cosign(dir: &Path, mode: &str) {
    let bin = dir.join("fake-bin");
    fs::create_dir_all(&bin).unwrap();
    let cosign = bin.join("cosign");
    fs::write(
        &cosign,
        format!(
            r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${{1:-}}" = "version" ]; then
  exit 0
fi
if [ "${{1:-}}" = "verify-blob" ]; then
  case "{mode}" in
    ok) exit 0 ;;
    fail) echo "fake verify: signature rejected" >&2; exit 1 ;;
  esac
fi
echo "unexpected fake cosign invocation: $*" >&2
exit 2
"#
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&cosign).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&cosign, perms).unwrap();
    }
    let old_path = std::env::var("PATH").unwrap_or_default();
    unsafe {
        std::env::set_var("PATH", format!("{}:{old_path}", bin.display()));
    }
}

fn hide_real_cosign(dir: &Path) {
    let empty = dir.join("empty-path");
    fs::create_dir_all(&empty).unwrap();
    unsafe {
        std::env::set_var("PATH", empty);
    }
}

// ── tests ───────────────────────────────────────────────────────

#[test]
fn c26b030_disabled_policy_ignores_missing_bundle() {
    with_env_guard(|| {
        let td = TempDir::new("disabled");
        let artifact = write_artifact(td.path(), "libx.so", b"payload");
        // Non-official URL + unset env -> Disabled by default.
        let url = "https://mirror.example.org/libx.so";
        assert_eq!(VerifyPolicy::resolve(url), VerifyPolicy::Disabled);
        let outcome = verify_artifact(&artifact, url, VerifyPolicy::Disabled).unwrap();
        assert_eq!(outcome, VerifyOutcome::Skipped);
    });
}

#[test]
fn c26b030_best_effort_warns_on_missing_bundle() {
    with_env_guard(|| {
        let td = TempDir::new("best_effort");
        let artifact = write_artifact(td.path(), "libx.so", b"payload");
        // file:// bundle URL that does not exist — the fetch returns
        // `Ok(false)` and best-effort downgrades to a warning.
        let url = format!("file://{}", artifact.display());
        let outcome = verify_artifact(&artifact, &url, VerifyPolicy::BestEffort).unwrap();
        match outcome {
            VerifyOutcome::Warned(reason) => {
                assert!(
                    reason.contains("bundle"),
                    "best-effort warning should mention bundle: {reason}"
                );
            }
            other => panic!("expected Warned, got {other:?}"),
        }
    });
}

#[test]
fn c26b030_required_rejects_missing_bundle() {
    with_env_guard(|| {
        let td = TempDir::new("required_missing");
        let artifact = write_artifact(td.path(), "libx.so", b"payload");
        let url = format!("file://{}", artifact.display());
        let err = verify_artifact(&artifact, &url, VerifyPolicy::Required).unwrap_err();
        assert!(
            matches!(err, VerifyError::BundleMissing(_)),
            "expected BundleMissing, got {err:?}"
        );
        // Error display must name the bundle URL so operators can act.
        let disp = err.to_string();
        assert!(disp.contains(".cosign.bundle"), "{disp}");
    });
}

#[test]
fn c26b030_required_passes_on_fake_ok() {
    with_env_guard(|| {
        let td = TempDir::new("required_ok");
        let artifact = write_artifact(td.path(), "libx.so", b"payload");
        // Stage a fake bundle alongside the artifact so the fetch-
        // bundle step is a no-op.
        let bundle = bundle_path_for(&artifact);
        fs::write(&bundle, b"fake-bundle-content").unwrap();
        install_fake_cosign(td.path(), "ok");
        let url = format!("file://{}", artifact.display());
        let outcome = verify_artifact(&artifact, &url, VerifyPolicy::Required).unwrap();
        assert_eq!(outcome, VerifyOutcome::Verified);
    });
}

#[test]
fn c26b030_required_rejects_on_fake_signature_failure() {
    with_env_guard(|| {
        let td = TempDir::new("required_fail");
        let artifact = write_artifact(td.path(), "libx.so", b"payload");
        let bundle = bundle_path_for(&artifact);
        fs::write(&bundle, b"fake-bundle-content").unwrap();
        install_fake_cosign(td.path(), "fail");
        let url = format!("file://{}", artifact.display());
        let err = verify_artifact(&artifact, &url, VerifyPolicy::Required).unwrap_err();
        match err {
            VerifyError::SignatureRejected { stderr } => {
                assert!(stderr.contains("fake verify"), "{stderr}");
            }
            other => panic!("expected SignatureRejected, got {other:?}"),
        }
    });
}

#[test]
fn c26b030_required_surfaces_cosign_unavailable_distinctly() {
    with_env_guard(|| {
        let td = TempDir::new("required_nocosign");
        let artifact = write_artifact(td.path(), "libx.so", b"payload");
        let bundle = bundle_path_for(&artifact);
        fs::write(&bundle, b"fake-bundle-content").unwrap();
        hide_real_cosign(td.path());
        let url = format!("file://{}", artifact.display());
        let err = verify_artifact(&artifact, &url, VerifyPolicy::Required).unwrap_err();
        assert!(
            matches!(err, VerifyError::CosignUnavailable),
            "expected CosignUnavailable, got {err:?}"
        );
    });
}

#[test]
fn c26b030_best_effort_downgrades_cosign_unavailable_to_warning() {
    with_env_guard(|| {
        let td = TempDir::new("best_effort_nocosign");
        let artifact = write_artifact(td.path(), "libx.so", b"payload");
        let bundle = bundle_path_for(&artifact);
        fs::write(&bundle, b"fake-bundle-content").unwrap();
        hide_real_cosign(td.path());
        let url = format!("file://{}", artifact.display());
        let outcome = verify_artifact(&artifact, &url, VerifyPolicy::BestEffort).unwrap();
        match outcome {
            VerifyOutcome::Warned(reason) => {
                assert!(reason.to_lowercase().contains("cosign"), "{reason}");
            }
            other => panic!("expected Warned, got {other:?}"),
        }
    });
}

#[test]
fn c26b030_env_required_applies_to_non_official_urls() {
    with_env_guard(|| {
        unsafe {
            std::env::set_var("TAIDA_VERIFY_SIGNATURES", "required");
        }
        // Even non-official URLs resolve to Required when the env
        // flag demands it — this is how an ops team tightens their
        // CI to fail closed on any unsigned download.
        assert_eq!(
            VerifyPolicy::resolve("https://mirror.example.org/libx.so"),
            VerifyPolicy::Required
        );
    });
}

#[test]
fn c26b030_bundle_path_and_url_are_canonical() {
    // Defensive pin against anyone renaming `.cosign.bundle` —
    // `scripts/release/verify-signatures.sh` and
    // `.github/workflows/release.yml` both hardcode the same suffix
    // so the install-side contract must match.
    assert_eq!(
        bundle_url_for("https://example.org/x.tar.gz"),
        "https://example.org/x.tar.gz.cosign.bundle"
    );
    assert_eq!(
        bundle_path_for(Path::new("/tmp/x.tar.gz")),
        PathBuf::from("/tmp/x.tar.gz.cosign.bundle")
    );
}

#[test]
fn c26b030_official_url_matcher_is_tight() {
    // The matcher must not be fooled by look-alike hosts or
    // paths. Breaking any of these would widen the BestEffort
    // default into territory the release workflow does not sign.
    assert!(is_official_release_url(
        "https://github.com/taida-lang/terminal/releases/download/@a.7/libterminal.so"
    ));
    assert!(!is_official_release_url(
        "https://github.com/taida-lang-mirror/terminal/releases/download/@a.7/libterminal.so"
    ));
    assert!(!is_official_release_url(
        "https://evil.example.com/github.com/taida-lang/terminal/releases/x"
    ));
}
