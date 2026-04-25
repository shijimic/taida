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
        eprintln!(
            "wasmtime exec failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
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
    let err = compile_wasm(&td, "wasm-min", &wasm).expect_err("wasm-min must reject readBytesAt");
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
    let err = compile_wasm(&td, "wasm-edge", &wasm).expect_err("wasm-edge must reject readBytesAt");
    assert!(
        err.contains("wasm-edge does not support 'taida_os_read_bytes_at'"),
        "wasm-edge reject diagnostic should mention taida_os_read_bytes_at, got: {}",
        err
    );
    let _ = std::fs::remove_file(&wasm);
}

// ===========================================================================
// C27B-020 (2026-04-25) follow-up: bytes mold lowering parity.
//
// Two new fixtures verify the wasm-side fixes:
//   1. `bytes_length_parity.td`: `chunk ]=> bytes; bytes.length()` returns
//      the actual byte count on every backend (regression guard for the
//      silent-0 bug where `taida_polymorphic_length` mis-dispatched Bytes).
//   2. `bytes_cursor_chain.td`: full `BytesCursor` -> `BytesCursorTake` ->
//      `U32LEDecode` chain compiles and runs on wasm-wasi (previously
//      rejected at compile time) and wasm-full.
//
// 4-backend identity is verified against the interpreter's output so that
// any future regression flips at least one backend.
// ===========================================================================

fn write_payload_named(name: &str, content: &[u8]) -> TempFile {
    let path = PathBuf::from(name);
    std::fs::write(&path, content).expect("write payload");
    TempFile(path)
}

fn run_interp(td: &Path) -> Option<String> {
    let out = Command::new(taida_bin()).arg(td).output().ok()?;
    if !out.status.success() {
        eprintln!(
            "interpreter failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn run_native(td: &Path) -> Option<String> {
    let exe = std::env::temp_dir().join(format!(
        "c27b020_native_{}_{}",
        std::process::id(),
        td.file_stem().and_then(|s| s.to_str()).unwrap_or("x")
    ));
    let status = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(td)
        .arg("-o")
        .arg(&exe)
        .output()
        .ok()?;
    if !status.status.success() {
        eprintln!(
            "native compile failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        return None;
    }
    let out = Command::new(&exe).output().ok()?;
    let _ = std::fs::remove_file(&exe);
    if !out.status.success() {
        eprintln!("native run failed");
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn run_js(td: &Path) -> Option<String> {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("SKIP: node unavailable");
        return None;
    }
    let js = std::env::temp_dir().join(format!(
        "c27b020_js_{}_{}.js",
        std::process::id(),
        td.file_stem().and_then(|s| s.to_str()).unwrap_or("x")
    ));
    let status = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(td)
        .arg("-o")
        .arg(&js)
        .output()
        .ok()?;
    if !status.status.success() {
        eprintln!(
            "js compile failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        return None;
    }
    let out = Command::new("node").arg(&js).output().ok()?;
    let _ = std::fs::remove_file(&js);
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// 4-backend `bytes.length()` parity guard. The fixture's payload is a
/// 16-byte file but the read takes 4 bytes — every backend must report 4.
#[test]
fn c27b_020_bytes_length_parity_4backend() {
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c27_wasm_bytes_extras/bytes_length_parity.td");

    let _payload = write_payload_named("_c27b_020_len_tmp.bin", b"ABCDEFGHIJKLMNOP");

    let interp = run_interp(&td).expect("interpreter run");
    assert_eq!(interp, "4", "interpreter bytes.length() expected 4");

    let native = run_native(&td).expect("native run");
    assert_eq!(native, interp, "native parity");

    if let Some(js) = run_js(&td) {
        assert_eq!(js, interp, "JS parity");
    }

    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable for wasi/full");
            return;
        }
    };

    let wasi = std::env::temp_dir().join("c27b020_len_wasi.wasm");
    compile_wasm(&td, "wasm-wasi", &wasi).expect("wasm-wasi compile");
    let wasi_out = run_wasm(&wasi, &wasmtime).expect("wasm-wasi run");
    let _ = std::fs::remove_file(&wasi);
    assert_eq!(
        wasi_out, interp,
        "wasm-wasi parity (regression guard against silent-0)"
    );

    let full = std::env::temp_dir().join("c27b020_len_full.wasm");
    compile_wasm(&td, "wasm-full", &full).expect("wasm-full compile");
    let full_out = run_wasm(&full, &wasmtime).expect("wasm-full run");
    let _ = std::fs::remove_file(&full);
    assert_eq!(
        full_out, interp,
        "wasm-full parity (regression guard against silent-0)"
    );
}

/// 3-step parsing fixture: chunk → BytesCursor → BytesCursorTake →
/// U32LEDecode. The 4 bytes "TEST" decode (LE) to 0x54534554 = 1414743380.
#[test]
fn c27b_020_bytes_cursor_chain_parity() {
    let td = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/quality/c27_wasm_bytes_extras/bytes_cursor_chain.td");

    let _payload = write_payload_named("_c27b_020_chain_tmp.bin", b"TEST");

    let interp = run_interp(&td).expect("interpreter run");
    assert_eq!(interp, "1414743380", "interpreter U32LEDecode of TEST");

    let native = run_native(&td).expect("native run");
    assert_eq!(native, interp, "native parity");

    if let Some(js) = run_js(&td) {
        assert_eq!(js, interp, "JS parity");
    }

    let wasmtime = match wasmtime_bin() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: wasmtime unavailable for wasi/full");
            return;
        }
    };

    let wasi = std::env::temp_dir().join("c27b020_chain_wasi.wasm");
    compile_wasm(&td, "wasm-wasi", &wasi).expect("wasm-wasi compile (was C27B-020 reject)");
    let wasi_out = run_wasm(&wasi, &wasmtime).expect("wasm-wasi run");
    let _ = std::fs::remove_file(&wasi);
    assert_eq!(wasi_out, interp, "wasm-wasi cursor chain parity");

    let full = std::env::temp_dir().join("c27b020_chain_full.wasm");
    compile_wasm(&td, "wasm-full", &full).expect("wasm-full compile");
    let full_out = run_wasm(&full, &wasmtime).expect("wasm-full run");
    let _ = std::fs::remove_file(&full);
    assert_eq!(full_out, interp, "wasm-full cursor chain parity");
}
