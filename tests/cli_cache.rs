//! CLI `taida cache` and WASM build cache tests.
//!
//! Covers: RC-8a WASM runtime cache hit, RC-8d cache clean command.
//!
//! RCB-29: Split from `todo_cli.rs` (1764 lines) into responsibility-based test files.

mod common;

use common::{taida_bin, unique_temp_dir};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

// -----------------------------------------------------------------------
// T-1: WASM runtime cache -- second build should hit cache (RC-8a)
// -----------------------------------------------------------------------

#[test]
fn test_rc8a_wasm_cache_hit_on_second_build() {
    let td = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/01_hello.td");
    if !td.exists() {
        return; // skip if examples missing
    }

    let tmp = unique_temp_dir("rc8a_cache_hit");
    let _ = fs::create_dir_all(&tmp);
    let wasm_out = tmp.join("hello.wasm");

    // First build -- compiles runtime (cache miss)
    let out1 = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min", "-o"])
        .arg(&wasm_out)
        .arg(&td)
        .output()
        .expect("first wasm build");
    assert!(
        out1.status.success(),
        "first build failed: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(
        wasm_out.exists(),
        "wasm output should exist after first build"
    );

    let _ = fs::remove_file(&wasm_out);

    // Second build -- should hit cache (faster, same result)
    let out2 = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min", "-o"])
        .arg(&wasm_out)
        .arg(&td)
        .output()
        .expect("second wasm build");
    assert!(
        out2.status.success(),
        "second build (cache hit) failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(
        wasm_out.exists(),
        "wasm output should exist after cache hit build"
    );

    let _ = fs::remove_dir_all(&tmp);
}

// -----------------------------------------------------------------------
// T-2: `taida cache clean` (RC-8d)
// -----------------------------------------------------------------------

#[test]
fn test_rc8d_cache_clean_removes_files() {
    let cache_dir = PathBuf::from("target/wasm-rt-cache");
    let _ = fs::create_dir_all(&cache_dir);

    let fake_o = cache_dir.join("test_clean.deadbeef.o");
    let fake_tmp = cache_dir.join("test_clean.deadbeef.42.0.tmp.o");
    let _ = fs::write(&fake_o, b"fake");
    let _ = fs::write(&fake_tmp, b"fake");

    let output = Command::new(taida_bin())
        .args(["cache", "clean"])
        .output()
        .expect("cache clean");
    assert!(
        output.status.success(),
        "cache clean failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Cleaned") || stdout.contains("already clean"),
        "should report cleaning result, got: {}",
        stdout
    );

    assert!(!fake_o.exists(), "fake .o should be removed by cache clean");
    assert!(
        !fake_tmp.exists(),
        "fake .tmp.o should be removed by cache clean"
    );
}

/// Regression: `taida cache clean` in a project with `.taida/` + `packages.tdm`
/// must find the project-local cache (`.taida/cache/wasm-rt/`), not the fallback.
#[test]
fn test_cache_clean_finds_project_local_cache() {
    let tmp = unique_temp_dir("cache_proj_local");
    let _ = fs::create_dir_all(tmp.join(".taida").join("cache").join("wasm-rt"));
    let _ = fs::write(tmp.join("packages.tdm"), "");

    // Place a fake cached file in the project-local cache
    let fake_o = tmp
        .join(".taida")
        .join("cache")
        .join("wasm-rt")
        .join("fake.deadbeef.o");
    let _ = fs::write(&fake_o, b"fake");
    assert!(fake_o.exists(), "setup: fake .o should exist");

    let output = Command::new(taida_bin())
        .args(["cache", "clean"])
        .current_dir(&tmp)
        .output()
        .expect("cache clean in project dir");
    assert!(
        output.status.success(),
        "cache clean failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !fake_o.exists(),
        "project-local cache .o should be removed by cache clean"
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn test_rc8d_cache_unknown_subcommand() {
    let output = Command::new(taida_bin())
        .args(["cache", "bogus"])
        .output()
        .expect("cache bogus");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown cache command"),
        "should reject unknown subcommand, got: {}",
        stderr
    );
}

#[test]
fn test_rc8d_cache_help() {
    let output = Command::new(taida_bin())
        .args(["cache", "--help"])
        .output()
        .expect("cache --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("clean"),
        "cache help should mention 'clean', got: {}",
        stdout
    );
}
