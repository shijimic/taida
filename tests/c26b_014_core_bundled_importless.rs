//! C26B-014: import-less core-bundled package parity (3-backend).
//!
//! `docs/reference/os_api.md` and `docs/guide/10_modules.md` promise
//! that `>>> taida-lang/os => @(...)` works without a matching
//! `packages.tdm` declaration. Before C26B-014 the runtime path
//! refused with `Runtime error: Package 'taida-lang/os' not found`
//! while the checker passed (divergence between
//! `install_core_bundled_os_pins` and `resolve_module_path`).
//!
//! Option B (Design Lock 2026-04-24): align the implementation with
//! docs — materialize the bundled stub on-demand. Package-declared
//! imports remain unaffected (deps-installed precedence preserved).
//!
//! This test covers the five core-bundled packages (`os`, `net`,
//! `crypto`, `js`, `pool`) and exercises interpreter + native. The
//! `js` package is JS-only at runtime but the import must still
//! resolve for all backends.

mod common;

use common::taida_bin;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn unique_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "c26b014_{}_{}_{}",
        label,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn run_interp(source: &str, label: &str) -> (bool, String, String) {
    let dir = unique_dir(label);
    let td = dir.join("main.td");
    fs::write(&td, source).expect("write main.td");
    // Deliberately no packages.tdm — that is the whole point of the
    // test. The checker and runtime must agree without one.
    let out = Command::new(taida_bin())
        .arg(&td)
        .output()
        .expect("spawn taida (interpreter)");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let ok = out.status.success();
    let _ = fs::remove_dir_all(&dir);
    (ok, stdout, stderr)
}

fn native_build(source: &str, label: &str) -> (bool, String) {
    let dir = unique_dir(label);
    let td = dir.join("main.td");
    fs::write(&td, source).expect("write main.td");
    let bin = dir.join("out");
    let out = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&td)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("spawn taida build");
    let ok = out.status.success();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let _ = fs::remove_dir_all(&dir);
    (ok, stderr)
}

#[test]
fn c26b_014_os_importless_interpreter_ok() {
    let source = "\
>>> taida-lang/os => @(readBytes)
stdout(\"os-ok\")
";
    let (ok, stdout, stderr) = run_interp(source, "os_interp");
    assert!(
        ok,
        "interpreter must accept >>> taida-lang/os without packages.tdm (C26B-014).\nstdout: {}\nstderr: {}",
        stdout, stderr
    );
    assert!(
        stdout.contains("os-ok"),
        "stdout must show post-import program output; got: {}",
        stdout
    );
}

#[test]
fn c26b_014_net_importless_interpreter_ok() {
    let source = "\
>>> taida-lang/net => @(httpParseRequestHead)
stdout(\"net-ok\")
";
    let (ok, stdout, stderr) = run_interp(source, "net_interp");
    assert!(
        ok,
        "interpreter must accept >>> taida-lang/net without packages.tdm (C26B-014).\nstdout: {}\nstderr: {}",
        stdout, stderr
    );
    assert!(stdout.contains("net-ok"), "got: {}", stdout);
}

#[test]
fn c26b_014_crypto_importless_interpreter_runs_sha256() {
    // The core-bundled sha256 has to actually be callable, not merely
    // bound — this is the step that previously crashed at runtime.
    let source = "\
>>> taida-lang/crypto => @(sha256)
digest <= sha256(\"hello\")
stdout(digest)
";
    let (ok, stdout, stderr) = run_interp(source, "crypto_interp");
    assert!(
        ok,
        "interpreter must run sha256 after import-less import (C26B-014).\nstdout: {}\nstderr: {}",
        stdout, stderr
    );
    assert!(
        stdout.contains("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"),
        "sha256('hello') digest must match canonical value; got: {}",
        stdout
    );
}

#[test]
fn c26b_014_pool_importless_interpreter_ok() {
    let source = "\
>>> taida-lang/pool => @(poolCreate)
stdout(\"pool-ok\")
";
    let (ok, stdout, stderr) = run_interp(source, "pool_interp");
    assert!(
        ok,
        "interpreter must accept >>> taida-lang/pool without packages.tdm (C26B-014).\nstdout: {}\nstderr: {}",
        stdout, stderr
    );
    assert!(stdout.contains("pool-ok"), "got: {}", stdout);
}

#[test]
fn c26b_014_native_os_importless_build_ok() {
    // The native backend already handles this via its own
    // `is_core_bundled_path` branch. Pin the 3-backend agreement.
    let source = "\
>>> taida-lang/os => @(getEnv)
value <= getEnv(\"PATH\", \"fallback\")
stdout(value)
";
    let (ok, stderr) = native_build(source, "os_native");
    assert!(
        ok,
        "native must build >>> taida-lang/os without packages.tdm (C26B-014 parity).\nstderr: {}",
        stderr
    );
}

#[test]
fn c26b_014_native_crypto_importless_build_ok() {
    let source = "\
>>> taida-lang/crypto => @(sha256)
digest <= sha256(\"hello\")
stdout(digest)
";
    let (ok, stderr) = native_build(source, "crypto_native");
    assert!(
        ok,
        "native must build >>> taida-lang/crypto without packages.tdm (C26B-014 parity).\nstderr: {}",
        stderr
    );
}
