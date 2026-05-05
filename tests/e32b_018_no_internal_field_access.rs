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
