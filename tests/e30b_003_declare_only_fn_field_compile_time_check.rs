//! E30B-003 / E30 Phase 5: `[E1410]` compile-time reject path for
//! declare-only function fields whose return type cannot be auto-generated
//! by `defaultFn` (Lock-D verdict, Phase 6 land 済 `default_fn_generatable`
//! API を消費)。
//!
//! Lock-C verdict (E30 Phase 0, 2026-04-28):
//!   - declare-only function field 戻り型が opaque / unknown alias なら
//!     `[E1410]` で reject (definition-site)
//!   - primitive / class-like / List / Lax / Async / type-param T 戻り型は
//!     `[E1410]` 発火しない (defaultFn が自動充足、Phase 6 land 済)
//!
//! 4-backend parity 観点: Phase 5 は **checker-only 変更** (definition-site
//! reject)。`taida way check` の標準 error 出力は backend 非依存で、4-backend
//! いずれも同一の `[E1410]` を発火する。本 test は `taida way check` の output
//! を assert することで 4-backend 共通の compile-time 挙動を pin する。
//!
//! Phase 4 (E30B-002) 受理 fixture の regression guard:
//!   - 既存 `e30b_002_*_passes` 4 本はすべて generatable 戻り型 (Str / T /
//!     Unit / T) なので Phase 5 land 後も PASS する。本 test の
//!     `e30b_003_generatable_return_does_not_emit_e1410` で同条件を再
//!     pin する。

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

/// Run `taida way check` on a temporary file written from `source` and return
/// (combined stdout+stderr, status_success).
fn run_check(source: &str, label: &str) -> (String, bool) {
    ensure_release_binary();
    let tmp = std::env::temp_dir().join(format!("e30b_003_{}.td", label));
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
    (combined, output.status.success())
}

/// E30B-003 acceptance #1: TypeDef variant with declare-only function
/// field whose return type is opaque (unknown alias) → `[E1410]` fires.
#[test]
fn e30b_003_typedef_opaque_return_emits_e1410() {
    let source = r#"Pilot = @(
  name: Str,
  build: Str => :Opaque
)
"#;
    let (combined, _ok) = run_check(source, "typedef_opaque");
    assert!(
        combined.contains("[E1410]"),
        "expected [E1410] for TypeDef opaque return, got: {}",
        combined
    );
    assert!(
        combined.contains("declare-only function field"),
        "expected diagnostic message about declare-only function field, got: {}",
        combined
    );
    assert!(
        combined.contains("'build'"),
        "expected the field name 'build' in the diagnostic, got: {}",
        combined
    );
}

/// E30B-003 acceptance #2: Mold variant with declare-only function field
/// whose return type is opaque → `[E1410]` fires.
#[test]
fn e30b_003_mold_opaque_return_emits_e1410() {
    let source = r#"Mold[T] => Foo[T] = @(
  name: Str,
  transform: T => :Opaque
)
"#;
    let (combined, _ok) = run_check(source, "mold_opaque");
    assert!(
        combined.contains("[E1410]"),
        "expected [E1410] for Mold opaque return, got: {}",
        combined
    );
    assert!(
        combined.contains("'transform'"),
        "expected the field name 'transform' in the diagnostic, got: {}",
        combined
    );
}

/// E30B-003 acceptance #3: Error variant with declare-only function field
/// whose return type is opaque → `[E1410]` fires.
#[test]
fn e30b_003_error_opaque_return_emits_e1410() {
    let source = r#"Error => MyErr = @(
  msg: Str,
  recovery: Unit => :Opaque
)
"#;
    let (combined, _ok) = run_check(source, "error_opaque");
    assert!(
        combined.contains("[E1410]"),
        "expected [E1410] for Error opaque return, got: {}",
        combined
    );
    assert!(
        combined.contains("'recovery'"),
        "expected the field name 'recovery' in the diagnostic, got: {}",
        combined
    );
}

/// E30B-003 acceptance #4: declare-only function field with generatable
/// return type (Str = primitive) → 0 errors. Pin Phase 4 acceptance
/// regression: declare-only fn fields with auto-generatable returns must
/// continue to be accepted in all class-like variants.
#[test]
fn e30b_003_generatable_return_does_not_emit_e1410() {
    let source = r#"Pilot = @(
  name: Str,
  greet: Str => :Str
)
p <= Pilot(name <= "Rei")
stdout(p.name)
"#;
    let (combined, ok) = run_check(source, "generatable");
    assert!(
        ok,
        "taida way check should succeed for generatable declare-only fn field, got: {}",
        combined
    );
    assert!(
        !combined.contains("[E1410]"),
        "[E1410] must NOT fire for generatable return (Str), got: {}",
        combined
    );
    assert!(
        combined.contains("errors=0"),
        "expected errors=0 in the check report, got: {}",
        combined
    );
}

/// E30B-003 acceptance #5: A method body (full method definition, not
/// declare-only) with opaque return type must NOT fire `[E1410]`. The
/// `is_declare_only_fn_field` predicate requires `is_method == false`,
/// so a method-with-body bypasses the Phase 5 reject path. (Note: the
/// method body would still need to construct a value of the opaque
/// return type via other means, which is checked by other diagnostics
/// like `[E1601]` return type mismatch — that is out of scope for this
/// `[E1410]` test.)
///
/// This test pins the boundary: only declare-only function fields (no
/// body, no default) trigger the Phase 5 check; methods-with-body are
/// untouched.
#[test]
fn e30b_003_method_with_body_bypasses_e1410_check() {
    // `compute` is a method-with-body returning Str (generatable). The
    // Phase 5 check should not even consider this field, because
    // `is_declare_only_fn_field()` returns false when `is_method == true`.
    let source = r#"Pilot = @(
  name: Str
  compute =
    name
  => :Str
)
p <= Pilot(name <= "Rei")
stdout(p.compute())
"#;
    let (combined, ok) = run_check(source, "method_with_body");
    assert!(
        ok,
        "taida way check should succeed for method with body, got: {}",
        combined
    );
    assert!(
        !combined.contains("[E1410]"),
        "[E1410] must NOT fire for method-with-body (is_method=true), got: {}",
        combined
    );
}

/// E30B-003 acceptance #6: Phase 4 (E30B-002) regression guard — the four
/// existing accepted patterns must continue to type-check cleanly after
/// Phase 5 land. This test uses inline copies of the e30b_002_*_passes
/// fixtures to ensure no silent regression.
#[test]
fn e30b_003_phase4_acceptance_regression_guard() {
    // typedef variant
    let typedef_source = r#"Pilot = @(name: Str, greet: Str => :Str)
p <= Pilot(name <= "Rei")
stdout(p.name)
"#;
    let (combined, ok) = run_check(typedef_source, "phase4_typedef");
    assert!(
        ok && !combined.contains("[E1410]") && combined.contains("errors=0"),
        "Phase 4 typedef regression: {}",
        combined
    );

    // mold variant
    let mold_source = r#"Mold[T] => Foo[T] = @(
  name: Str,
  transform: T => :T
)
f <= Foo[1, "x"]()
stdout(f.name)
"#;
    let (combined, ok) = run_check(mold_source, "phase4_mold");
    assert!(
        ok && !combined.contains("[E1410]") && combined.contains("errors=0"),
        "Phase 4 mold regression: {}",
        combined
    );

    // error variant
    let error_source = r#"Error => NotFound = @(
  msg: Str,
  recovery: Unit => :Unit
)
err <= NotFound(msg <= "missing")
stdout(err.msg)
"#;
    let (combined, ok) = run_check(error_source, "phase4_error");
    assert!(
        ok && !combined.contains("[E1410]") && combined.contains("errors=0"),
        "Phase 4 error regression: {}",
        combined
    );

    // inheritance variant (Mold-derived child with declare-only fn field)
    let inheritance_source = r#"Mold[T] => Container[T] = @(item: T)

Container[T] => Greeter[T] = @(
  greet: T => :T
)
g <= Greeter[7, 42]()
stdout(g.item.toString())
"#;
    let (combined, ok) = run_check(inheritance_source, "phase4_inheritance");
    assert!(
        ok && !combined.contains("[E1410]") && combined.contains("errors=0"),
        "Phase 4 inheritance regression: {}",
        combined
    );
}
