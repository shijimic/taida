//! C26B-020 柱 3 — WASM lowering for readBytesAt(path, offset, len).
//!
//! The interpreter / JS / native implementations landed in Round 1 wD
//! (柱 1).  This suite covers the 4-backend parity extension to
//! wasm-wasi / wasm-full, and verifies that wasm-min / wasm-edge reject
//! the API at compile time with a profile-specific error message so
//! callers pick an appropriate backend (no silent stub).
//!
//! The fixtures are pre-created by the harness (writing a 16-byte
//! payload for the success case; no file for the error case) because
//! the `Bytes[...]()` constructor mold is implemented only on
//! wasm-full and not on wasm-wasi — constructing Bytes in .td source
//! would limit this suite to a single profile.

mod common;

use common::{taida_bin, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Compile a .td to a given wasm profile and return the output wasm path
/// on success, or the emitted stderr string on compile failure.
fn compile_wasm(td: &Path, target: &str, out: &Path) -> Result<(), String> {
    let output = Command::new(taida_bin())
        .args(["build", "--target", target])
        .arg(td)
        .arg("-o")
        .arg(out)
        .output()
        .map_err(|e| format!("spawn taida failed: {}", e))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

/// Pre-create the 16-byte payload file in CWD that the success fixture reads.
/// Returns a guard that removes the file on drop (best-effort).
struct TempFile(PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn write_payload() -> TempFile {
    let path = PathBuf::from("_c26b_020_wasi_tmp.bin");
    std::fs::write(&path, b"ABCDEFGHIJKLMNOP").expect("write payload");
    TempFile(path)
}

fn run_wasm(wasm: &Path, wasmtime: &Path) -> Option<String> {
    let out = Command::new(wasmtime)
        .args(["run", "--dir=.", "--"])
        .arg(wasm)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!("wasmtime exec failed: {}", String::from_utf8_lossy(&out.stderr));
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// wasm-wasi: readBytesAt success cases (normal chunk, truncated tail,
/// beyond-EOF) all produce Lax success (hasValue == true), matching the
/// interpreter / native contract.
#[test]
fn c26b_020_wasm_wasi_read_bytes_at_success() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable");
            return;
        }
    };
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_wasm_bytes/readBytesAt_basic.td");
    let wasm = std::env::temp_dir().join("c26b020_wasi_basic.wasm");
    compile_wasm(&td, "wasm-wasi", &wasm).expect("wasm-wasi compile");

    let _payload = write_payload();
    let out = run_wasm(&wasm, &wasmtime).expect("wasm-wasi run");
    let _ = std::fs::remove_file(&wasm);

    assert_eq!(
        out, "true\ntrue\ntrue",
        "wasm-wasi readBytesAt basic: expected Lax success for all 3 chunks"
    );
}

/// wasm-wasi: readBytesAt error-path cases (missing file, negative
/// offset/len, over-ceiling len) all produce Lax empty (hasValue == false).
#[test]
fn c26b_020_wasm_wasi_read_bytes_at_errors() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable");
            return;
        }
    };
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_wasm_bytes/readBytesAt_errors.td");
    let wasm = std::env::temp_dir().join("c26b020_wasi_errors.wasm");
    compile_wasm(&td, "wasm-wasi", &wasm).expect("wasm-wasi compile");

    let out = run_wasm(&wasm, &wasmtime).expect("wasm-wasi run");
    let _ = std::fs::remove_file(&wasm);

    assert_eq!(
        out, "false\nfalse\nfalse\nfalse",
        "wasm-wasi readBytesAt errors: expected Lax empty for all 4 cases"
    );
}

/// wasm-full: same success fixture, now linked against rt_core + rt_wasi + rt_full.
/// Bytes produced by wasi_io are layout-compatible with rt_full's `_wf_is_bytes()`.
#[test]
fn c26b_020_wasm_full_read_bytes_at_success() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable");
            return;
        }
    };
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_wasm_bytes/readBytesAt_basic.td");
    let wasm = std::env::temp_dir().join("c26b020_full_basic.wasm");
    compile_wasm(&td, "wasm-full", &wasm).expect("wasm-full compile");

    let _payload = write_payload();
    let out = run_wasm(&wasm, &wasmtime).expect("wasm-full run");
    let _ = std::fs::remove_file(&wasm);

    assert_eq!(
        out, "true\ntrue\ntrue",
        "wasm-full readBytesAt basic: expected Lax success for all 3 chunks"
    );
}

/// wasm-full errors.
#[test]
fn c26b_020_wasm_full_read_bytes_at_errors() {
    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable");
            return;
        }
    };
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_wasm_bytes/readBytesAt_errors.td");
    let wasm = std::env::temp_dir().join("c26b020_full_errors.wasm");
    compile_wasm(&td, "wasm-full", &wasm).expect("wasm-full compile");

    let out = run_wasm(&wasm, &wasmtime).expect("wasm-full run");
    let _ = std::fs::remove_file(&wasm);

    assert_eq!(
        out, "false\nfalse\nfalse\nfalse",
        "wasm-full readBytesAt errors: expected Lax empty for all 4 cases"
    );
}

/// wasm-min must reject readBytesAt at compile time with the generic OS
/// operations diagnostic.
#[test]
fn c26b_020_wasm_min_rejects_read_bytes_at() {
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_wasm_bytes/readBytesAt_basic.td");
    let wasm = std::env::temp_dir().join("c26b020_min.wasm");
    let err = compile_wasm(&td, "wasm-min", &wasm)
        .expect_err("wasm-min must reject readBytesAt");
    assert!(
        err.contains("wasm-min does not support OS operations"),
        "wasm-min reject diagnostic should mention OS operations, got: {}",
        err
    );
    let _ = std::fs::remove_file(&wasm);
}

/// wasm-edge must reject readBytesAt at compile time with a profile-specific
/// diagnostic pointing at wasm-wasi / native.
#[test]
fn c26b_020_wasm_edge_rejects_read_bytes_at() {
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c26_wasm_bytes/readBytesAt_basic.td");
    let wasm = std::env::temp_dir().join("c26b020_edge.wasm");
    let err = compile_wasm(&td, "wasm-edge", &wasm)
        .expect_err("wasm-edge must reject readBytesAt");
    assert!(
        err.contains("wasm-edge does not support 'taida_os_read_bytes_at'"),
        "wasm-edge reject diagnostic should mention taida_os_read_bytes_at, got: {}",
        err
    );
    let _ = std::fs::remove_file(&wasm);
}
