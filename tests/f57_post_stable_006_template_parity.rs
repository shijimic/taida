// F56-FB-006 follow-up (F57 Phase 1c): `${...}` interpolation parity for
// bodies the interpreter renders verbatim.
//
// When a `${...}` body does not parse as an expression — e.g. a positional-
// field pack literal `@(a: @(b <= 42))` rejected by `[E1521]` — the
// interpreter (`eval_template_string`) emits the raw body text. The native
// backend previously treated the body as a bare variable name, lowering to an
// undefined-variable read that rendered as `0`. This pins the native fallback
// against the interpreter, and also pins a *valid* nested pack literal that
// both backends lower and render identically.
//
// Scope: interpreter ⇔ native only. The JS backend lowers Taida templates to
// JS template literals (pass-through), so a `@(...)` pack literal inside `${}`
// is a JS `SyntaxError` regardless of validity. Bringing the JS template path
// to the parse-based interp/native model is a structural rewrite tracked
// separately (F57B-002); it is out of scope for this fallback-parity fix.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::process::Command;

fn interp_out(label: &str, src: &str) -> String {
    let dir = unique_temp_dir(label);
    let f = dir.join("main.td");
    write_file(&f, src);
    let out = Command::new(taida_bin())
        .arg(&f)
        .output()
        .expect("run interpreter");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "interp failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn native_out(label: &str, src: &str) -> String {
    let dir = unique_temp_dir(label);
    let f = dir.join("main.td");
    write_file(&f, src);
    let bin = dir.join("out.bin");
    let comp = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&f)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("native build");
    assert!(
        comp.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&comp.stderr)
    );
    let run = Command::new(&bin).output().expect("native run");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        run.status.success(),
        "native run failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    String::from_utf8_lossy(&run.stdout).trim_end().to_string()
}

/// A `${...}` body that fails to parse as an expression (`@(a: @(b <= 42))`
/// uses `:` = a positional field, rejected by `[E1521]`) must be emitted
/// verbatim on native, exactly as the interpreter does — not lowered to a
/// bare-variable read that renders as `0`.
#[test]
fn nonexpr_pack_interpolation_native_matches_interp() {
    let src = "x <= `${@(a: @(b <= 42))}`\nstdout(x)\n";
    let i = interp_out("f57_006fb_interp_a", src);
    let n = native_out("f57_006fb_native_a", src);
    assert_eq!(
        n, i,
        "native must match the interpreter for a non-expression body"
    );
    assert!(i.contains("@(a: @(b <= 42))"), "interp baseline: {i}");
}

/// A *valid* nested pack literal interpolation (`@(a <= @(b <= 42))`) is parsed
/// and lowered by both backends and must render the same display string.
#[test]
fn nested_pack_interpolation_native_matches_interp() {
    let src = "x <= `${@(a <= @(b <= 42))}`\nstdout(x)\n";
    let i = interp_out("f57_006fb_interp_b", src);
    let n = native_out("f57_006fb_native_b", src);
    assert_eq!(
        n, i,
        "native must match the interpreter for a nested pack body"
    );
    assert!(i.contains("@(a <= @(b <= 42))"), "interp baseline: {i}");
}

/// Whitespace-padded non-expression body: the raw text is emitted with its
/// *leading* spaces intact, because both backends use the untrimmed `${...}`
/// body (`expr_str`, not `expr_str_trimmed`). This pins that choice so a future
/// edit cannot silently switch native to the trimmed form and re-diverge from
/// the interpreter. (The helpers `trim_end` only the program's trailing
/// newline, so the assertion checks the leading-space prefix.)
#[test]
fn padded_nonexpr_interpolation_preserves_leading_space_on_both() {
    let src = "x <= `${  @(a: @(b <= 42))  }`\nstdout(x)\n";
    let i = interp_out("f57_006fb_interp_pad", src);
    let n = native_out("f57_006fb_native_pad", src);
    assert_eq!(
        n, i,
        "native must match the interpreter for a padded non-expression body"
    );
    assert!(
        i.starts_with("  @(a: @(b <= 42))"),
        "interp must keep the leading spaces of the untrimmed body: {i:?}"
    );
}

/// a parsed-but-non-expression body (`${x <= 1}` parses as an
/// assignment) is rejected with `[E1702]` instead of silently emitting
/// nothing — the old silent-skip behaviour this test used to pin let a
/// mistyped body vanish from the output with no diagnostic. Both backends
/// must reject it: the interpreter at runtime, the native/wasm lowering at
/// build time (they share `lower_template_lit`, so one check covers both).
#[test]
fn nonexpr_statement_interpolation_rejected_on_both() {
    let src = "y <= `[${x <= 1}]`\nstdout(y)\n";

    let dir = unique_temp_dir("f57_006fb_interp_assign");
    let f = dir.join("main.td");
    write_file(&f, src);
    let out = Command::new(taida_bin())
        .arg(&f)
        .output()
        .expect("run interpreter");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        !out.status.success(),
        "interp must reject a statement body, got stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("[E1702]"),
        "interp rejection must cite [E1702], got: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dir = unique_temp_dir("f57_006fb_native_assign");
    let f = dir.join("main.td");
    write_file(&f, src);
    let bin = dir.join("out.bin");
    let comp = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg(&f)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("native build");
    let _ = fs::remove_dir_all(&dir);
    assert!(
        !comp.status.success(),
        "native build must reject a statement body"
    );
    assert!(
        String::from_utf8_lossy(&comp.stderr).contains("[E1702]"),
        "native rejection must cite [E1702], got: {}",
        String::from_utf8_lossy(&comp.stderr)
    );
}
