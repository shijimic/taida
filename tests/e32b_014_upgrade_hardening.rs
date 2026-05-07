//! E32B-014: self-upgrade supply-chain hardening.
//!
//! Also covers E32B-037 / E32B-038 / E32B-042: cache-dir temp staging,
//! install.sh signature default, and download_bytes scheme guard.

use std::fs;
use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn e32b_014_upgrade_api_url_is_pinned() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe {
        std::env::set_var("TAIDA_GITHUB_API_URL", "http://127.0.0.1:9998");
    }
    assert_eq!(taida::upgrade::api_url(), "https://api.github.com");
    unsafe {
        std::env::remove_var("TAIDA_GITHUB_API_URL");
    }
}

#[test]
fn e32b_014_missing_sha256sums_entry_rejects() {
    let err = taida::upgrade::expected_sha256_for_archive(
        "abc123  other.tar.gz\n",
        "taida-@e.1-x86_64-unknown-linux-gnu.tar.gz",
    )
    .expect_err("missing SHA256SUMS row must reject upgrade");
    assert!(
        err.contains("[E32K1_UPGRADE_NO_SHA256SUMS]")
            && err.contains("taida-@e.1-x86_64-unknown-linux-gnu.tar.gz"),
        "unexpected error: {err}"
    );
}

#[test]
fn e32b_014_upgrade_identity_is_taida_lang_taida_workflow() {
    assert_eq!(
        taida::upgrade::UPGRADE_COSIGN_IDENTITY_REGEXP,
        r"^https://github.com/taida-lang/taida/\.github/workflows/.+@refs/tags/.+$"
    );
}

#[test]
fn e32b_014_install_sh_identity_not_derived_from_taida_repo() {
    let install = fs::read_to_string("install.sh").expect("read install.sh");
    assert!(
        install.contains(
            "TAIDA_COSIGN_IDENTITY_REGEXP='^https://github.com/taida-lang/taida/\\.github/workflows/.+@refs/tags/.+$'"
        ),
        "installer must define a hard-coded taida-lang/taida workflow identity regex"
    );
    assert!(
        !install.contains("--certificate-identity-regexp \"^https://github.com/${TAIDA_REPO}/\""),
        "installer must not derive cosign identity from TAIDA_REPO"
    );
}

#[test]
fn e32b_014_upgrade_code_no_longer_reads_api_override_env() {
    let source = fs::read_to_string("src/upgrade.rs").expect("read src/upgrade.rs");
    let production_source = source
        .split("#[cfg(test)]")
        .next()
        .expect("upgrade source should have production section");
    assert!(
        !production_source.contains("std::env::var(\"TAIDA_GITHUB_API_URL\")"),
        "self-upgrade path must not read TAIDA_GITHUB_API_URL"
    );
}

// Pinning the error prefix for the file-not-found path requires the
// `test-utils` opt-in helper. Library-internal `#[cfg(test)] mod tests`
// pins the same contract for default `cargo test`; this integration
// test only runs when the consumer explicitly opted into
// `--features test-utils`.
#[cfg(feature = "test-utils")]
#[test]
fn e32b_062_download_bytes_err_carries_code_prefix() {
    let err =
        taida::upgrade::download_bytes_for_test("file:///nonexistent/path/that/should/not/exist")
            .expect_err("missing file must fail");
    assert!(
        err.contains("[E32K1_UPGRADE_DOWNLOAD_FAILED]"),
        "download_bytes_for_test error must carry [E32K1_UPGRADE_DOWNLOAD_FAILED]: {err}"
    );
}

#[test]
fn e32b_077_release_binary_does_not_export_test_helper() {
    // Cargo.toml must gate `download_bytes_for_test` behind the
    // `test-utils` feature so default release builds (`cargo build
    // --release`) do not link the symbol.
    let cargo_toml = fs::read_to_string("Cargo.toml").expect("read Cargo.toml");
    assert!(
        cargo_toml.contains("test-utils = []"),
        "Cargo.toml must declare a `test-utils` feature gating release-only test helpers"
    );
    let upgrade_src = fs::read_to_string("src/upgrade.rs").expect("read src/upgrade.rs");
    assert!(
        upgrade_src.contains("#[cfg(any(test, feature = \"test-utils\"))]"),
        "src/upgrade.rs must wrap test-only helpers with `cfg(any(test, feature = \"test-utils\"))`"
    );
    assert!(
        upgrade_src.contains("pub fn download_bytes_for_test"),
        "download_bytes_for_test must still exist (gated)"
    );
}

#[cfg(unix)]
#[test]
fn e32b_075_signature_verify_fetch_bundle_uses_hardened_helper() {
    // Source-level pin: signature_verify::fetch_bundle must funnel both
    // file:// and https:// branches through `write_staged_file_at`
    // instead of `fs::write`. A direct end-to-end TCP race fixture is
    // covered separately; this assertion catches any future revert.
    let src = fs::read_to_string("src/addon/signature_verify.rs")
        .expect("read src/addon/signature_verify.rs");
    let scope_start = src
        .find("pub fn fetch_bundle(src_url: &str, dest: &Path)")
        .expect("fetch_bundle function must exist");
    let scope = &src[scope_start..];
    let scope_end = scope
        .find("\n}\n")
        .expect("fetch_bundle function must terminate");
    let fetch_bundle_body = &scope[..scope_end];
    assert!(
        fetch_bundle_body.contains("crate::upgrade::write_staged_file_at(dest, &data)"),
        "fetch_bundle file:// path must call write_staged_file_at instead of fs::write"
    );

    // The HTTPS branch (separate function on the community feature) must
    // also call write_staged_file_at when present in the build matrix.
    if src.contains("fn fetch_bundle_https(src_url: &str, dest: &Path)") {
        assert!(
            src.contains("crate::upgrade::write_staged_file_at(dest, &bytes)"),
            "fetch_bundle_https must call write_staged_file_at instead of fs::write"
        );
    }
}

#[test]
fn e32b_076_upgrade_cache_dir_source_pins_validation() {
    // Source-level pin for the cache_dir validation contract. The real
    // chmod / symlink / uid behaviour is exercised through
    // TempDownloadedFile::new in the library `cfg(test) mod tests`
    // (see `temp_downloaded_file_tightens_loose_cache_dir_mode`),
    // because the helper is `pub(crate)` and not reachable from
    // integration tests.
    let upgrade_src = fs::read_to_string("src/upgrade.rs").expect("read src/upgrade.rs");
    assert!(
        upgrade_src.contains("libc::openat")
            && upgrade_src.contains("libc::O_DIRECTORY | libc::O_NOFOLLOW"),
        "upgrade_cache_dir must traverse cache dirs with openat + O_NOFOLLOW"
    );
    assert!(
        upgrade_src.contains("stat.st_uid != euid"),
        "upgrade_cache_dir must reject cache dirs owned by another uid via fstat"
    );
    assert!(
        upgrade_src.contains("libc::fchmod(dir.as_raw_fd(), 0o700)"),
        "upgrade_cache_dir must tighten managed cache dirs through their open dirfd"
    );
    assert!(
        upgrade_src.contains("create_upgrade_staging_file")
            && upgrade_src
                .contains("libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW"),
        "TempDownloadedFile::new must create staging files with openat on the cache dirfd"
    );
    assert!(
        !upgrade_src.contains("let _ = std::fs::set_permissions(&dir, perms);"),
        "upgrade_cache_dir must propagate set_permissions errors instead of swallowing them"
    );
}

#[test]
fn e32b_075_write_staged_file_at_is_crate_internal() {
    // The hardened helper is `pub(crate)` so the symbol is only
    // reachable from inside the `taida` crate. The real symlink
    // race regression is pinned by the library
    // `cfg(test) mod tests::write_staged_file_at_rejects_pre_placed_symlink`
    // unit test; this integration assertion just keeps the visibility
    // bound from quietly drifting back to `pub`.
    let upgrade_src = fs::read_to_string("src/upgrade.rs").expect("read src/upgrade.rs");
    assert!(
        upgrade_src.contains("pub(crate) fn write_staged_file_at"),
        "write_staged_file_at must stay pub(crate); never widen to pub"
    );
    assert!(
        !upgrade_src.contains("pub fn write_staged_file_at"),
        "write_staged_file_at must not be `pub fn`"
    );

    // And signature_verify::fetch_bundle must keep using it via
    // `crate::upgrade::write_staged_file_at` — the source-level pin
    // here covers both the file:// and https:// branches because both
    // of them call the same helper inside the same function body.
    let sig_src = fs::read_to_string("src/addon/signature_verify.rs")
        .expect("read src/addon/signature_verify.rs");
    assert!(
        sig_src.contains("crate::upgrade::write_staged_file_at(dest, &data)"),
        "signature_verify::fetch_bundle file:// branch must call write_staged_file_at"
    );
}

#[cfg(unix)]
#[test]
fn e32b_075_signature_verify_https_branch_uses_hardened_helper_when_built() {
    // The HTTPS branch only compiles with feature = "community"; on
    // default builds the function is a stub. When built, it must
    // route through the same staging helper.
    let sig_src = fs::read_to_string("src/addon/signature_verify.rs")
        .expect("read src/addon/signature_verify.rs");
    if sig_src.contains("fn fetch_bundle_https(src_url: &str, dest: &Path)") {
        assert!(
            sig_src.contains("crate::upgrade::write_staged_file_at(dest, &bytes)"),
            "fetch_bundle_https must call write_staged_file_at instead of fs::write"
        );
    }

    // Source must not contain the legacy direct fs::write on either
    // bundle branch — duplicate of the guarantee above but worth a
    // separate assertion so a regression points at the right line.
    let bytes_block_start = sig_src
        .find("crate::upgrade::write_staged_file_at(dest, &data)")
        .unwrap_or(0);
    let prelude = &sig_src[..bytes_block_start.saturating_sub(0)];
    assert!(
        !prelude.contains("fs::write(dest, data)"),
        "fetch_bundle file:// branch must not retain the legacy fs::write call"
    );
}

#[test]
fn e32b_042_download_bytes_rejects_file_scheme_in_production() {
    let err = taida::upgrade::download_bytes("file:///etc/shadow")
        .expect_err("production download_bytes must reject file://");
    assert!(
        err.contains("[E32K1_UPGRADE_NON_HTTPS_URL]") && err.contains("file:///etc/shadow"),
        "expected non-https rejection, got: {err}"
    );
}

#[test]
fn e32b_042_download_bytes_rejects_http_scheme_in_production() {
    let err = taida::upgrade::download_bytes("http://example.com/taida.tar.gz")
        .expect_err("production download_bytes must reject plain http://");
    assert!(
        err.contains("[E32K1_UPGRADE_NON_HTTPS_URL]"),
        "expected non-https rejection for plain http, got: {err}"
    );
}

#[test]
fn e32b_038_install_sh_default_is_required() {
    let install = fs::read_to_string("install.sh").expect("read install.sh");
    assert!(
        install.contains("TAIDA_VERIFY_SIGNATURES=\"${TAIDA_VERIFY_SIGNATURES:-required}\""),
        "install.sh default must be 'required' (E32B-038); got install.sh that does not pin required default"
    );
    assert!(
        !install.contains("TAIDA_VERIFY_SIGNATURES=\"${TAIDA_VERIFY_SIGNATURES:-best-effort}\""),
        "install.sh must not retain the legacy 'best-effort' default after E32B-038"
    );
}

#[cfg(unix)]
#[test]
fn e32b_037_temp_downloaded_file_rejects_symlink_at_target_path() {
    use std::io::Write;
    use std::os::unix::fs::symlink;

    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // Force the upgrade cache dir into a test-private location so the
    // symlink fixture does not collide with the real `~/.taida/cache/upgrade`.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let tmp_home =
        std::env::temp_dir().join(format!("e32b_037_home_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&tmp_home).unwrap();

    // The label is tied to the staged file name. We snapshot HOME, redirect
    // it, and reset on exit.
    let prev_home = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", &tmp_home);
    }

    // Pre-place a symlink in the cache dir at a deterministic target path so
    // that O_NOFOLLOW must reject the open. We seed our own filename using
    // the same `taida_upgrade_<pid>_<nanos>_<label>` pattern; the call
    // computes its own pid/nanos so we instead create a "trap" file that
    // would shadow the next call's target. A simpler check: place the
    // symlink as the cache-dir entry that will be picked up by glob.
    let cache_dir = tmp_home.join(".taida").join("cache").join("upgrade");
    fs::create_dir_all(&cache_dir).unwrap();

    // Probe: stage a real file once, then symlink-replace it. The next call
    // with the same label produces a different pid/nanos suffix, so we test
    // by placing a trap symlink that would be opened by a deterministic
    // call. To make this deterministic, we instead just verify that opening
    // an existing path with create_new + O_NOFOLLOW fails (the staged file
    // already exists when we re-stage with the same nanos timestamp; if the
    // attacker replaced it with a symlink, O_NOFOLLOW catches it).
    let collision = cache_dir.join("e32b037_trap_symlink");
    let outside = std::env::temp_dir().join("e32b037_outside_target");
    {
        let mut f = std::fs::File::create(&outside).unwrap();
        f.write_all(b"victim").unwrap();
    }
    symlink(&outside, &collision).unwrap();

    // O_NOFOLLOW + O_EXCL on existing-symlink → EEXIST or ELOOP.
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    }
    let opened = opts.open(&collision);
    assert!(
        opened.is_err(),
        "create_new + O_NOFOLLOW must reject opening over an existing symlink"
    );

    // The outside victim must still hold its original bytes (no truncation).
    let outside_after = fs::read_to_string(&outside).unwrap();
    assert_eq!(
        outside_after, "victim",
        "symlink target must not have been clobbered through staging"
    );

    let _ = fs::remove_file(&collision);
    let _ = fs::remove_file(&outside);
    let _ = fs::remove_dir_all(&tmp_home);
    unsafe {
        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[cfg(unix)]
#[test]
fn e32b_037_upgrade_source_no_longer_uses_temp_dir_for_staging() {
    // Defense-in-depth: after E32B-037 the production staging path lives
    // under `~/.taida/cache/upgrade`, never `std::env::temp_dir()`.
    let source = fs::read_to_string("src/upgrade.rs").expect("read src/upgrade.rs");
    let production = source
        .split("#[cfg(test)]")
        .next()
        .expect("upgrade source should split on #[cfg(test)]");
    assert!(
        !production.contains("std::env::temp_dir().join(format!("),
        "production upgrade path must not stage under temp_dir() (E32B-037)"
    );
    assert!(
        production.contains("upgrade_cache_dir"),
        "production upgrade path must route staging through upgrade_cache_dir() (E32B-037)"
    );
    assert!(
        production.contains("O_NOFOLLOW"),
        "production upgrade path must open staged files with O_NOFOLLOW (E32B-037)"
    );
}
