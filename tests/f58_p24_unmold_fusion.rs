/// Regression tests for the direct-form unmold fusion.
///
/// `Lax[x]() >=> v` with a statically-scalar `x`, and `Div/Mod[a, b]()
/// >=> v` with a non-zero Int-literal divisor, are fused at lowering
/// time: no Lax is materialised, no runtime unmold runs, and the
/// has_value branch disappears. Everything outside those proofs — zero
/// or variable divisors, non-scalar payloads — must keep the
/// materialised path and its exact semantics (empty Lax falls back to
/// the default on unmold).
mod common;

use common::{run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::Command;

fn build_and_run_native(td: &Path, dir: &Path, stem: &str) -> String {
    let bin = dir.join(format!("{stem}_native"));
    let status = Command::new(taida_bin())
        .args(["build", "native"])
        .arg(td)
        .arg("-o")
        .arg(&bin)
        .status()
        .expect("taida build native runs");
    assert!(status.success(), "native build failed for {stem}");
    let out = Command::new(&bin).output().expect("native binary runs");
    assert!(out.status.success(), "native run failed for {stem}");
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn build_and_run_wasm(td: &Path, dir: &Path, stem: &str) -> Option<String> {
    let wasmtime = wasmtime_bin()?;
    let wasm = dir.join(format!("{stem}.wasm"));
    let status = Command::new(taida_bin())
        .args(["build", "wasm-min"])
        .arg(td)
        .arg("-o")
        .arg(&wasm)
        .status()
        .expect("taida build wasm-min runs");
    assert!(status.success(), "wasm build failed for {stem}");
    let out = Command::new(&wasmtime)
        .arg(&wasm)
        .output()
        .expect("wasmtime runs");
    assert!(out.status.success(), "wasm run failed for {stem}");
    Some(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn assert_parity(dir: &Path, stem: &str, source: &str) -> String {
    let td = dir.join(format!("{stem}.td"));
    std::fs::write(&td, source).expect("write fixture");
    let interp = run_interpreter(&td).expect("interpreter runs");
    let native = build_and_run_native(&td, dir, stem);
    assert_eq!(interp, native, "{stem}: interp vs native");
    if let Some(wasm) = build_and_run_wasm(&td, dir, stem) {
        assert_eq!(interp, wasm, "{stem}: interp vs wasm-min");
    } else {
        eprintln!("SKIP: wasmtime not found, wasm leg skipped for {stem}");
    }
    interp
}

/// Fused forms: scalar Lax payloads of every kind and divisor-proven
/// Div/Mod (positive and negative literals).
#[test]
fn fused_unmold_preserves_scalar_semantics() {
    let dir = unique_temp_dir("f58_p24_fused");
    let out = assert_parity(
        &dir,
        "fused",
        r#"Lax[42]() >=> i
stdout(i)
Lax[2.5]() >=> f
stdout(f)
Lax[true]() >=> b
stdout(b)
Div[17, 5]() >=> q
stdout(q)
Mod[17, 5]() >=> r
stdout(r)
Div[17, 0 - 5]() >=> nq
stdout(nq)
x <= 100
Div[x, 3]() >=> vq
stdout(vq)
"#,
    );
    assert_eq!(out, "42\n2.5\ntrue\n3\n2\n-3\n33");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Non-fused forms keep the materialised Lax path: a zero-literal or
/// variable divisor can produce the empty Lax, whose unmold falls back
/// to the default (0 for Div/Mod).
#[test]
fn unproven_divisors_keep_empty_lax_semantics() {
    let dir = unique_temp_dir("f58_p24_unproven");
    let out = assert_parity(
        &dir,
        "unproven",
        r#"Div[7, 0]() >=> z
stdout(z)
d <= 0
Div[7, d]() >=> vz
stdout(vz)
e <= 4
Div[7, e]() >=> ve
stdout(ve)
"#,
    );
    assert_eq!(out, "0\n0\n1");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A non-scalar Lax payload (string) keeps the materialised path —
/// fusion is restricted to scalars until the retain/release pairing is
/// covered by the IR-level escape pass.
#[test]
fn non_scalar_lax_keeps_materialised_path() {
    let dir = unique_temp_dir("f58_p24_str_lax");
    let out = assert_parity(
        &dir,
        "str_lax",
        r#"Lax["payload"]() >=> s
stdout(s)
stdout(s.length())
"#,
    );
    assert_eq!(out, "payload\n7");
    let _ = std::fs::remove_dir_all(&dir);
}

/// User-defined molds must unmold on every backend. The WASM runtime
/// rejected every emitted `__type` literal with a low-address guard
/// (the data segment starts at global-base 1024, the guard cut at
/// 4096), so `boxed >=> v` silently returned the carrier pack instead
/// of the value — for every user mold, ever. String headers made the
/// `__type` slot exactly identifiable and the guard now requires the
/// string magic instead.
#[test]
fn user_mold_unmold_works_on_every_backend() {
    let dir = unique_temp_dir("f58_p24_user_mold");
    let out = assert_parity(
        &dir,
        "user_mold",
        r#"Mold[T] => SmallMold[T] = @(
  a <= 1,
  b <= 2
)

boxed <= SmallMold[777]()
boxed >=> v
stdout(v)
"#,
    );
    assert_eq!(out, "777");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A user mold may legitimately carry hundreds of fields; the unmold
/// pack-shape validation must scale with the field count (it verifies
/// the tail sentinel in-bounds) rather than capping it — a cap would
/// silently skip the unmold on wasm only. The value is displayed
/// directly: wasm identification is positive-only (string magic
/// required, no byte heuristics), so an untagged Int whose value lands
/// in the data segment can no longer be mistaken for a pointer — this
/// very fixture used to print "37" through that misclassification.
#[test]
fn large_field_count_mold_unmold_not_skipped() {
    let dir = unique_temp_dir("f58_p24_large_mold");
    let fields: Vec<String> = (1..=256).map(|i| format!("  f{i} <= {i}")).collect();
    let source = format!(
        "Mold[T] => BigMold[T] = @(\n{}\n)\n\nboxed <= BigMold[4242]()\nboxed >=> v\nstdout(v)\n",
        fields.join(",\n")
    );
    let out = assert_parity(&dir, "large_mold", &source);
    assert_eq!(out, "4242");
    let _ = std::fs::remove_dir_all(&dir);
}
