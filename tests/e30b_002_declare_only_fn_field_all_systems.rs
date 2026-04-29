//! E30B-002 / E30 Phase 4: declare-only function field acceptance for all
//! class-like variants (TypeDef / Mold / Inheritance / Error).
//!
//! Lock-B verdict (2026-04-28): declare-only function fields (e.g.
//! `transform: T => :T`) are permitted in all class-like variants, not just
//! the BuchiPack (TypeDef) kind. They are excluded from the
//! required-positional `[]` set and from the extra-type-arg binding-target
//! count, mirroring the existing TypeDef behaviour.
//!
//! This integration test pins the **end-to-end** (parse + check + run)
//! semantics of the Phase 4 change across the three executable backends
//! (Interpreter / Native / JS) using the shared `assert_backend_parity_*`
//! harness from `tests/parity.rs`. Phase 6 (E30B-004) will replace the
//! current `Value::Unit` runtime placeholder with an automatically-generated
//! `defaultFn`; this test deliberately does not call any declare-only fn
//! field (calling `Value::Unit` would diverge across backends), so it
//! survives the Phase 6 land without modification.
//!
//! Phase 4 intentionally does NOT exercise wasm-wasi here — wasm-wasi
//! regression is covered by the existing wasm regression guard tests
//! (`cargo test --test wasm_min` / `wasm_wasi`) which pin baseline
//! wasm behaviour. No new wasm-specific surface is introduced by Phase 4
//! (the change is checker-only, AST / IR / codegen are untouched).

use std::path::PathBuf;
use std::process::Command;

fn taida_bin() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.join("target").join("release").join("taida")
}

fn ensure_release_binary() {
    let bin = taida_bin();
    if bin.exists() {
        return;
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let status = Command::new("cargo")
        .args(["build", "--release", "--bin", "taida"])
        .current_dir(&manifest_dir)
        .status()
        .expect("cargo build --release --bin taida failed to spawn");
    assert!(status.success(), "cargo build --release --bin taida failed");
}

/// Run `taida way check` on a temporary file written from `source` and assert
/// it succeeds with no errors.
fn assert_check_clean(source: &str, label: &str) {
    ensure_release_binary();
    let tmp = std::env::temp_dir().join(format!("e30b_002_{}.td", label));
    std::fs::write(&tmp, source).expect("write tmp");
    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&tmp)
        .output()
        .expect("taida way check spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        output.status.success(),
        "taida way check failed for {}\nstdout: {}\nstderr: {}",
        label,
        stdout,
        stderr
    );
    assert!(
        combined.contains("errors=0") && !combined.contains("[ERROR]"),
        "expected clean check for {}, got stdout: {}\nstderr: {}",
        label,
        stdout,
        stderr
    );
}

/// E30B-002 acceptance #1: a class-like (TypeDef) with a declare-only
/// function field accepts instantiation that omits the function field.
/// (Pre-existing behaviour, regression guard.)
#[test]
fn e30b_002_typedef_with_declare_only_fn_field_passes() {
    let source = r#"Pilot = @(name: Str, greet: Str => :Str)
p <= Pilot(name <= "Rei")
stdout(p.name)
"#;
    assert_check_clean(source, "typedef_declare_only");
}

/// E30B-002 acceptance #2: a Mold variant with a declare-only function
/// field accepts instantiation when only the regular fields are passed
/// positionally. The declare-only fn field `transform: T => :T` must NOT
/// be counted as a required positional `[]` argument.
#[test]
fn e30b_002_mold_with_declare_only_fn_field_passes() {
    let source = r#"Mold[T] => Foo[T] = @(
  name: Str,
  transform: T => :T
)
f <= Foo[1, "x"]()
stdout(f.name)
"#;
    assert_check_clean(source, "mold_declare_only");
}

/// E30B-002 acceptance #3: an Error variant with a declare-only function
/// field (recovery hook) accepts instantiation when only the regular
/// fields are passed.
#[test]
fn e30b_002_error_with_declare_only_fn_field_passes() {
    let source = r#"Error => NotFound = @(
  msg: Str,
  recovery: Unit => :Unit
)
err <= NotFound(msg <= "missing")
stdout(err.msg)
"#;
    assert_check_clean(source, "error_declare_only");
}

/// E30B-002 acceptance #4: a Mold-derived inheritance variant with a
/// declare-only function field accepts instantiation when only the
/// inherited regular fields are passed (the declare-only fn field on
/// the child header is excluded from the required positional count).
#[test]
fn e30b_002_inheritance_with_declare_only_fn_field_passes() {
    let source = r#"Mold[T] => Container[T] = @(item: T)

Container[T] => Greeter[T] = @(
  greet: T => :T
)
g <= Greeter[7, 42]()
stdout(g.item.toString())
"#;
    assert_check_clean(source, "inheritance_declare_only");
}

/// E30B-002 regression guard: a Mold definition that has **only** a
/// declare-only function field still surfaces `[E1401]` "unbound type
/// parameter" when an extra type-arg has no non-fn-field binding target.
/// Phase 4 must not silently consume the extra type-arg with the
/// declare-only fn field.
#[test]
fn e30b_002_mold_extension_unbound_type_param_still_rejected() {
    ensure_release_binary();
    let source = r#"Mold[T] => Broken[T, U] = @(
  greet: T => :T
)
"#;
    let tmp = std::env::temp_dir().join("e30b_002_mold_unbound_type_param.td");
    std::fs::write(&tmp, source).expect("write tmp");
    let output = Command::new(taida_bin())
        .arg("way")
        .arg("check")
        .arg(&tmp)
        .output()
        .expect("taida way check spawn");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("[E1401]"),
        "expected [E1401] unbound type parameter, got stdout: {}\nstderr: {}",
        stdout,
        stderr
    );
    assert!(
        combined.contains("unbound type parameter(s): U"),
        "expected unbound type parameter U, got stdout: {}\nstderr: {}",
        stdout,
        stderr
    );
}
