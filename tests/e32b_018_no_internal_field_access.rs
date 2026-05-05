//! E32B-018: user-facing `__*` field access is rejected.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::path::Path;
use std::process::Command;

fn stderr_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_e1960(output: &std::process::Output, label: &str) {
    assert!(
        !output.status.success(),
        "{label} should reject internal field access"
    );
    let stderr = stderr_text(output);
    assert!(
        stderr.contains("[E1960]") && stderr.contains("__value"),
        "{label} should report E1960 for __value, got: {}",
        stderr
    );
}

fn write_lax_false_fixture(dir: &Path) -> std::path::PathBuf {
    let src = dir.join("internal_value.td");
    write_file(
        &src,
        r#"
empty: @[Int] <= @[]
lax <= empty.first()
stdout(lax.__value.toString())
"#,
    );
    src
}

#[test]
fn e32b_018_interpreter_rejects_lax_false_internal_value_access() {
    let dir = unique_temp_dir("e32b_018_interp");
    let src = write_lax_false_fixture(&dir);

    let output = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run taida interpreter");
    assert_e1960(&output, "interpreter");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_018_build_backends_reject_internal_value_access() {
    let dir = unique_temp_dir("e32b_018_build");
    let src = write_lax_false_fixture(&dir);

    let cases = [
        ("js", dir.join("out.mjs")),
        ("native", dir.join("out-native")),
        ("wasm-min", dir.join("out.wasm")),
    ];
    for (target, out_path) in cases {
        let output = Command::new(taida_bin())
            .args(["build", target])
            .arg(&src)
            .arg("-o")
            .arg(&out_path)
            .output()
            .unwrap_or_else(|_| panic!("run taida build {target}"));
        assert_e1960(&output, target);
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_018_error_type_internal_access_rejected() {
    let dir = unique_temp_dir("e32b_018_type");
    let src = dir.join("internal_type.td");
    write_file(
        &src,
        r#"
Error => MyError = @(reason: Str)
err <= MyError(type <= "MyError", message <= "boom", reason <= "x")
stdout(err.__type)
"#,
    );

    let output = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run taida interpreter");
    assert!(
        !output.status.success(),
        "interpreter should reject __type access"
    );
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("[E1960]") && stderr.contains("__type"),
        "expected E1960 for __type, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

/// E32B-054: `lower_stdout_with_tag` previously bypassed
/// `lower_field_access`'s E1960 guard for FieldAccess arguments with a
/// compile-time-unknown tag, so `taida --no-check build native` would happily
/// emit `taida_pack_get(obj, "__value")` and produce a binary that prints
/// the internal field. The guard now lives in both paths; this test pins
/// the `--no-check` Native behavior.
#[test]
fn e32b_054_no_check_native_rejects_internal_field_via_stdout() {
    let dir = unique_temp_dir("e32b_054_no_check_native");
    let src = write_lax_false_fixture(&dir);
    let out_path = dir.join("out-native");

    let output = Command::new(taida_bin())
        .args(["--no-check", "build", "native"])
        .arg(&src)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("run taida --no-check build native");
    assert_e1960(&output, "--no-check native");
    assert!(
        !out_path.exists(),
        "build must not produce a Native binary when E1960 fires (bypass would have produced one)"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_054_no_check_js_rejects_internal_field_via_stdout() {
    let dir = unique_temp_dir("e32b_054_no_check_js");
    let src = write_lax_false_fixture(&dir);
    let out_path = dir.join("out.mjs");

    let output = Command::new(taida_bin())
        .args(["--no-check", "build", "js"])
        .arg(&src)
        .arg("-o")
        .arg(&out_path)
        .output()
        .expect("run taida --no-check build js");
    assert_e1960(&output, "--no-check js");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn e32b_018_unhandled_throw_output_hides_internal_fields() {
    let dir = unique_temp_dir("e32b_018_throw");
    let src = dir.join("throw.td");
    write_file(
        &src,
        r#"
Error => MyError = @(reason: Str)
MyError(type <= "MyError", message <= "boom", reason <= "x").throw()
"#,
    );

    let output = Command::new(taida_bin())
        .arg(&src)
        .output()
        .expect("run taida interpreter");
    assert!(!output.status.success(), "unhandled throw should fail");
    let stderr = stderr_text(&output);
    assert!(
        stderr.contains("Error[MyError]: boom"),
        "panic output should use sanitized error schema, got: {}",
        stderr
    );
    assert!(
        !stderr.contains("__type") && !stderr.contains("__value") && !stderr.contains("__default"),
        "panic output must not expose internal fields, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}
