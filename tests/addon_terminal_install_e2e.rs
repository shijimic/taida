//! RC1.5 Phase 4 -- end-to-end install-timeline integration test.
//!
//! RC15B-101 fix: file:// URLs now require relative paths only.
//! RC15B-105 adds negative test cases: absolute paths, path traversal,
//! symlink attacks, concurrent installs, cleanup, scheme validation.
//!
//! Each fetch test uses a unique `package_id + version` combination so
//! tests don't collide in the shared cache even when run in parallel.

#![cfg(feature = "native")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

fn taida_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_taida"))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn find_terminal_cdylib() -> Option<PathBuf> {
    let target_root = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir().join("target"));
    let lib_name = if cfg!(target_os = "linux") {
        "libtaida_addon_terminal_sample.so"
    } else if cfg!(target_os = "macos") {
        "libtaida_addon_terminal_sample.dylib"
    } else if cfg!(target_os = "windows") {
        "taida_addon_terminal_sample.dll"
    } else {
        return None;
    };
    let candidates = [
        target_root.join("debug").join(lib_name),
        target_root.join("release").join(lib_name),
        target_root.join("debug").join("deps").join(lib_name),
        target_root.join("release").join("deps").join(lib_name),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn compute_sha256(path: &Path) -> String {
    let data = fs::read(path).expect("must read addon cdylib");
    let mut hasher = taida::crypto::Sha256::new();
    hasher.update(&data);
    hasher.finalize_hex()
}

fn detect_target_triple() -> &'static str {
    if cfg!(target_os = "linux") {
        if cfg!(target_arch = "x86_64") {
            "x86_64-unknown-linux-gnu"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64-unknown-linux-gnu"
        } else {
            "unknown-linux-gnu"
        }
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "x86_64") {
            "x86_64-apple-darwin"
        } else if cfg!(target_arch = "aarch64") {
            "aarch64-apple-darwin"
        } else {
            "unknown-apple-darwin"
        }
    } else {
        "unsupported"
    }
}

fn cdylib_ext() -> &'static str {
    #[cfg(target_os = "linux")]
    return "so";
    #[cfg(target_os = "macos")]
    return "dylib";
    #[cfg(target_os = "windows")]
    return "dll";
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return "unknown";
}

// ── Test work-dir framework ──────────────────────────────────

/// Create a unique CWD-relative work directory for a single test.
/// Each test gets its own directory so tests don't collide even when
/// running in parallel. Returns both the (relative, absolute) paths.
/// The caller should clean up the work dir at the end of the test.
fn make_work_dir(test_id: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let rel = format!(
        ".taida-test-temp-e2e/{}_{}_{}",
        test_id,
        std::process::id(),
        nanos
    );
    let abs = std::env::current_dir().expect("CWD").join(&rel);
    fs::create_dir_all(&abs).expect("create work dir");
    abs
}

/// Copy a cdylib into the given work_dir and return (filename, sha256).
fn copy_cdylib_to(work_dir: &Path, src: &Path) -> (String, String) {
    let ext = cdylib_ext();
    let filename = format!("terminal_local.{}", ext);
    let dest = work_dir.join(&filename);
    fs::copy(src, &dest).expect("copy cdylib to work dir");
    let sha = compute_sha256(&dest);
    (filename, sha)
}

/// Create a symlink in work_dir pointing to src. Returns (link_name, sha256).
fn make_symlink_in(work_dir: &Path, src: &Path) -> (String, String) {
    let ext = cdylib_ext();
    let link_name = format!("terminal_link.{}", ext);
    let link_path = work_dir.join(&link_name);
    let _ = fs::remove_file(&link_path);
    let abs_src = src.canonicalize().expect("canonicalize");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&abs_src, &link_path).expect("symlink");
    #[cfg(not(unix))]
    std::os::windows::fs::symlink_file(&abs_src, &link_path).expect("symlink");
    let sha = compute_sha256(src);
    (link_name, sha)
}

/// Run fetch_prebuild with the work_dir as CWD.
/// Uses unique pkg_id/version to avoid cache collision with other tests.
#[allow(clippy::too_many_arguments)]
fn fetch_in_work(
    work_dir: &Path,
    pkg_id: &str,
    version: &str,
    target: &str,
    url: &str,
    sha256: &str,
    lib_name: &str,
    ext: &str,
) -> Result<PathBuf, taida::addon::prebuild_fetcher::FetchError> {
    let saved = std::env::current_dir().expect("get CWD");
    std::env::set_current_dir(work_dir).expect("set CWD to work dir");
    let result = taida::addon::prebuild_fetcher::fetch_prebuild(
        pkg_id, version, target, lib_name, ext, url, sha256,
    );
    std::env::set_current_dir(&saved).ok();
    result
}

// ── RC15B-101: file:// relative-path happy path ───────────────

#[test]
fn file_relative_path_happy_path() {
    let cdylib = match find_terminal_cdylib() {
        Some(p) => p,
        None => {
            return;
        }
    };
    let work = make_work_dir("file_relative_happy");
    let (fname, sha) = copy_cdylib_to(&work, &cdylib);
    let target = detect_target_triple();
    let ext = cdylib_ext();

    let r = fetch_in_work(
        &work,
        "test-pkg-relative/file-relative",
        "v-e2e-1",
        target,
        &format!("file://{}", fname),
        &sha,
        "terminal",
        ext,
    );
    assert!(r.is_ok(), "relative file:// must succeed: {:?}", r.err());
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-101: file:// absolute path is rejected ─────────────

#[test]
fn file_absolute_path_is_rejected() {
    let work = make_work_dir("file_absolute");
    let target = detect_target_triple();
    let r = fetch_in_work(
        &work,
        "test-pkg-abs/file-abs",
        "v-e2e-1",
        target,
        "file:///etc/passwd",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "terminal",
        "so",
    );
    assert!(r.is_err(), "absolute file:// must be rejected");
    let msg = format!("{:?}", r.unwrap_err());
    assert!(msg.contains("absolute path"), "got: {}", msg);
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-101: file:// with path traversal is rejected ───────

#[test]
fn file_path_traversal_is_rejected() {
    let work = make_work_dir("file_traversal");
    let target = detect_target_triple();
    let r = fetch_in_work(
        &work,
        "test-pkg-trav/file-trav",
        "v-e2e-1",
        target,
        "file://./../../../etc/passwd",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "terminal",
        "so",
    );
    assert!(r.is_err(), "path traversal file:// must be rejected");
    let msg = format!("{:?}", r.unwrap_err());
    assert!(msg.contains("path traversal"), "got: {}", msg);
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-105: Unsupported scheme ────────────────────────────

#[test]
fn http_scheme_is_rejected() {
    let work = make_work_dir("http_scheme");
    let target = detect_target_triple();
    let r = fetch_in_work(
        &work,
        "test-pkg-http/file-http",
        "v-e2e-1",
        target,
        "http://example.com/terminal.so",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "terminal",
        "so",
    );
    assert!(r.is_err(), "http:// must be rejected");
    let msg = format!("{:?}", r.unwrap_err());
    assert!(msg.contains("unsupported URL scheme"), "got: {}", msg);
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-105: Non-existent file ─────────────────────────────

#[test]
fn file_not_found_produces_download_failed() {
    let work = make_work_dir("file_not_found");
    let target = detect_target_triple();
    let r = fetch_in_work(
        &work,
        "test-pkg-nf/file-nf",
        "v-e2e-1",
        target,
        "file://nonexistent_file.so",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "terminal",
        "so",
    );
    assert!(r.is_err());
    assert!(matches!(
        r.unwrap_err(),
        taida::addon::prebuild_fetcher::FetchError::DownloadFailed { .. }
    ));
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-105: file:// with non-existent relative path ───────

#[test]
fn file_relative_nonexistent_is_rejected() {
    let work = make_work_dir("file_rel_nonexist");
    let target = detect_target_triple();
    let r = fetch_in_work(
        &work,
        "test-pkg-rne/file-rne",
        "v-e2e-1",
        target,
        "file://a/b/c/not_exist.so",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "terminal",
        "so",
    );
    assert!(r.is_err());
    assert!(matches!(
        r.unwrap_err(),
        taida::addon::prebuild_fetcher::FetchError::DownloadFailed { .. }
    ));
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-105: Symlink without traversal ─────────────────────

#[test]
fn file_through_symlink_without_traversal() {
    let cdylib = match find_terminal_cdylib() {
        Some(p) => p,
        None => {
            return;
        }
    };
    let work = make_work_dir("file_symlink");
    let target = detect_target_triple();
    let ext = cdylib_ext();

    // Copy the real file and create a symlink next to it.
    let (link_name, sha) = make_symlink_in(&work, &cdylib);

    let r = fetch_in_work(
        &work,
        "test-pkg-symlink/file-symlink",
        "v-e2e-1",
        target,
        &format!("file://{}", link_name),
        &sha,
        "terminal",
        ext,
    );
    // Symlink without .. is accepted.
    assert!(
        r.is_ok(),
        "symlink without .. should succeed: {:?}",
        r.err()
    );
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-105: Package ID traversal ──────────────────────────

#[test]
fn package_id_traversal_in_org_is_rejected() {
    let work = make_work_dir("pkg_id_traversal");
    let target = detect_target_triple();
    let r = fetch_in_work(
        &work,
        "../../../malicious/terminal",
        "v-e2e-1",
        target,
        "file://some/file.so",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "terminal",
        "so",
    );
    assert!(r.is_err(), "traversal in org must be rejected");
    let msg = format!("{:?}", r.unwrap_err());
    assert!(msg.contains("invalid package id"), "got: {}", msg);
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-105: Concurrent install simulation ─────────────────

#[test]
fn concurrent_fetch_simulation_same_addon() {
    let cdylib = match find_terminal_cdylib() {
        Some(p) => p,
        None => {
            return;
        }
    };
    let work = make_work_dir("concurrent_fetch");
    let target = detect_target_triple();
    let ext = cdylib_ext();
    let (fname, sha) = copy_cdylib_to(&work, &cdylib);
    let url = format!("file://{}", fname);

    let r1 = fetch_in_work(
        &work,
        "test-pkg-conc/file-conc",
        "v-e2e-1",
        target,
        &url,
        &sha,
        "terminal",
        ext,
    );
    let r2 = fetch_in_work(
        &work,
        "test-pkg-conc/file-conc",
        "v-e2e-1",
        target,
        &url,
        &sha,
        "terminal",
        ext,
    );

    assert!(r1.is_ok(), "first fetch: {:?}", r1.err());
    assert!(r2.is_ok(), "second (cache): {:?}", r2.err());
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-105: Temp file cleanup on failure ──────────────────

#[test]
fn temp_file_cleaned_on_integrity_failure() {
    let cdylib = match find_terminal_cdylib() {
        Some(p) => p,
        None => {
            return;
        }
    };
    let work = make_work_dir("temp_file_cleanup");
    let target = detect_target_triple();
    let ext = cdylib_ext();
    let (fname, _sha) = copy_cdylib_to(&work, &cdylib);
    let wrong_sha = "0000000000000000000000000000000000000000000000000000000000000000";

    let r = fetch_in_work(
        &work,
        "test-pkg-cleanup/file-cleanup",
        "v-e2e-1",
        target,
        &format!("file://{}", fname),
        wrong_sha,
        "terminal",
        ext,
    );
    assert!(r.is_err(), "wrong SHA must be rejected");

    let err = r.unwrap_err();
    assert!(
        matches!(
            err,
            taida::addon::prebuild_fetcher::FetchError::IntegrityMismatch { .. }
        ),
        "must be IntegrityMismatch, got: {:?}",
        err
    );

    // No .tmp files in cache.
    if let Ok(home) = std::env::var("HOME") {
        let cache_dir = PathBuf::from(home)
            .join(".taida/addon-cache/taida-lang/terminal/v-e2e-1")
            .join(target);
        if cache_dir.exists() {
            for e in fs::read_dir(&cache_dir).unwrap().filter_map(|x| x.ok()) {
                assert!(
                    !e.path().to_string_lossy().ends_with(".tmp"),
                    ".tmp file should not remain: {:?}",
                    e.path()
                );
            }
        }
    }
    let _ = fs::remove_dir_all(&work);
}

// ── RC15B-105: Package ID with special characters ────────────

#[test]
fn package_id_with_special_chars_is_rejected() {
    let work = make_work_dir("pkg_id_special");
    let target = detect_target_triple();
    let r = fetch_in_work(
        &work,
        "org!/na@me",
        "v-e2e-1",
        target,
        "file://file.so",
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "terminal",
        "so",
    );
    assert!(r.is_err());
    let msg = format!("{:?}", r.unwrap_err());
    assert!(msg.contains("invalid package id"), "got: {}", msg);
    let _ = fs::remove_dir_all(&work);
}

// ── Legacy e2e tests (adapted for RC15B-101) ────────────────

#[test]
fn addon_terminal_install_and_call() {
    let cdylib = match find_terminal_cdylib() {
        Some(p) => p,
        None => {
            return;
        }
    };
    let work = make_work_dir("install_and_call");
    let target = detect_target_triple();
    let ext = cdylib_ext();
    let (fname, sha) = copy_cdylib_to(&work, &cdylib);
    let url = format!("file://{}", fname);
    let pkg_id = "taida-lang/terminal";
    let version = "a.1";

    // Phase 1: fresh fetch
    let r1 = fetch_in_work(&work, pkg_id, version, target, &url, &sha, "terminal", ext);
    assert!(r1.is_ok(), "fetch: {:?}", r1.err());
    let fetched = r1.unwrap();
    assert!(fetched.exists());
    assert_eq!(compute_sha256(&fetched), sha);

    // Phase 2: cache hit
    let r2 = fetch_in_work(&work, pkg_id, version, target, &url, &sha, "terminal", ext);
    assert!(r2.is_ok(), "cache hit: {:?}", r2.err());

    // Phase 3: wrong SHA (clear cache first)
    if let Ok(home) = std::env::var("HOME") {
        let cache_entry = PathBuf::from(home).join(format!(
            ".taida/addon-cache/taida-lang/terminal/{}/{}",
            version, target
        ));
        let _ = fs::remove_dir_all(&cache_entry);
    }
    let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
    let r3 = fetch_in_work(&work, pkg_id, version, target, &url, wrong, "terminal", ext);
    assert!(r3.is_err());
    let msg = format!("{:?}", r3.unwrap_err());
    assert!(
        msg.contains("IntegrityMismatch") || msg.contains("integrity"),
        "got: {}",
        msg
    );
    let _ = fs::remove_dir_all(&work);
}

#[test]
fn addon_terminal_sha256_mismatch_produces_error() {
    let cdylib = match find_terminal_cdylib() {
        Some(p) => p,
        None => {
            return;
        }
    };
    let work = make_work_dir("sha256_mismatch");
    let target = detect_target_triple();
    let ext = cdylib_ext();
    let (fname, real_sha) = copy_cdylib_to(&work, &cdylib);
    let wrong = "0000000000000000000000000000000000000000000000000000000000000000";

    let r = fetch_in_work(
        &work,
        "taida-lang/terminal",
        "a.1-e2e-mismatch",
        target,
        &format!("file://{}", fname),
        wrong,
        "terminal",
        ext,
    );
    assert!(r.is_err());
    let err = r.unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("IntegrityMismatch"), "got: {}", msg);
    let displayed = format!("{err}");
    assert!(
        displayed.contains(&real_sha[..10]),
        "must show actual hash: {}",
        displayed
    );
    let _ = fs::remove_dir_all(&work);
}

#[test]
fn addon_terminal_cache_hit_uses_cached_file() {
    let cdylib = match find_terminal_cdylib() {
        Some(p) => p,
        None => {
            return;
        }
    };
    let work = make_work_dir("cache_hit");
    let target = detect_target_triple();
    let ext = cdylib_ext();
    let (fname, sha) = copy_cdylib_to(&work, &cdylib);
    let url = format!("file://{}", fname);

    let r1 = fetch_in_work(
        &work,
        "taida-lang/terminal",
        "a.1-e2e-cachehit",
        target,
        &url,
        &sha,
        "terminal",
        ext,
    );
    assert!(r1.is_ok(), "first fetch: {:?}", r1.err());

    // Verify files
    if let Ok(home) = std::env::var("HOME") {
        let cache_dir = PathBuf::from(home).join(format!(
            ".taida/addon-cache/taida-lang/terminal/a.1-e2e-cachehit/{}",
            target
        ));
        let cached = cache_dir.join(format!("libterminal.{}", ext));
        let sidecar = cache_dir.join(".manifest-sha256");
        assert!(cached.exists(), "cached file must exist");
        assert!(sidecar.exists(), "sidecar must exist");

        let size_before = std::fs::metadata(&cached).ok().map(|m| m.len());

        let r2 = fetch_in_work(
            &work,
            "taida-lang/terminal",
            "a.1-e2e-cachehit",
            target,
            &url,
            &sha,
            "terminal",
            ext,
        );
        assert!(r2.is_ok(), "cache hit: {:?}", r2.err());

        let size_after = std::fs::metadata(&cached).ok().map(|m| m.len());
        assert_eq!(
            size_before, size_after,
            "size shouldn't change on cache hit: {:?} vs {:?}",
            size_before, size_after
        );

        let sidecar_data = fs::read_to_string(&sidecar).expect("read sidecar");
        assert!(sidecar_data.contains(&sha), "sidecar must contain SHA-256");
    }
    let _ = fs::remove_dir_all(&work);
}

// ── Interpreter round-trip ───────────────────────────────────

#[test]
fn terminal_addon_term_print_interpreter_round_trip() {
    let cdylib = match find_terminal_cdylib() {
        Some(p) => p,
        None => {
            return;
        }
    };
    let project = unique_temp_dir("rc15_terminal_interpreter");
    let _ = fs::remove_dir_all(&project);
    fs::create_dir_all(&project).unwrap();

    let deps_terminal = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("terminal");
    let native_dir = deps_terminal.join("native");
    fs::create_dir_all(&native_dir).unwrap();

    let lib_name = if cfg!(target_os = "linux") {
        "libtaida_addon_terminal_sample.so"
    } else if cfg!(target_os = "macos") {
        "libtaida_addon_terminal_sample.dylib"
    } else {
        "taida_addon_terminal_sample.dll"
    };
    fs::copy(&cdylib, native_dir.join(lib_name)).unwrap();

    fs::write(
        native_dir.join("addon.toml"),
        r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/terminal"
library = "taida_addon_terminal_sample"

[functions]
termPrint = 1
termPrintLn = 1
termReadLine = 0
termSize = 0
termIsTty = 0
"#,
    )
    .unwrap();

    fs::write(
        project.join("main.td"),
        r#">>> taida-lang/terminal => @(termPrint, termPrintLn)
termPrint("hello from terminal")
termPrintLn("done")
"#,
    )
    .unwrap();

    let output = Command::new(taida_bin())
        .arg(project.join("main.td"))
        .output()
        .expect("run taida");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "taida must succeed\n{}\n{}",
        stdout,
        stderr
    );
    assert!(stdout.contains("hello from terminal"), "got: {}", stdout);
    assert!(stdout.contains("done"), "got: {}", stdout);

    let _ = fs::remove_dir_all(&project);
}
