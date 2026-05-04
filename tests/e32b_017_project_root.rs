//! E32B-017: `~/.taida` is user state, not a project-root marker.
//!
//! The original regression made any source under `$HOME` inherit `$HOME` as
//! project root when `$HOME/.taida` existed. That widened SEC-003's import
//! boundary and allowed a project file to import another absolute path under
//! the same home directory. This test keeps the PoC negative across the
//! interpreter, JS build, native build, and wasm-min build paths without
//! touching the broad parity test file.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_home() -> PathBuf {
    std::env::temp_dir().join(format!(
        "taida_e32b017_home_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ))
}

fn write_poc_layout() -> (PathBuf, PathBuf, PathBuf) {
    let home = unique_home();
    let project = home.join("poc_user_proj");
    let attacker = home.join("poc_attacker");
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(home.join(".taida")).expect("create home .taida");
    fs::create_dir_all(&project).expect("create project");
    fs::create_dir_all(&attacker).expect("create attacker dir");

    let secret = attacker.join("secret.td");
    fs::write(
        &secret,
        "secret n: Str =\n  \"leaked-\" + n\n=> :Str\n\n<<< @(secret)\n",
    )
    .expect("write secret module");

    let main = project.join("main.td");
    fs::write(
        &main,
        format!(
            ">>> {} => @(secret)\n\nmsg <= secret(\"x\")\nstdout(msg)\n",
            secret.display()
        ),
    )
    .expect("write main module");

    (home, main, secret)
}

fn combined_output(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_rejects_absolute_escape(label: &str, mut cmd: Command, import_token: &Path) {
    let output = cmd.output().unwrap_or_else(|err| {
        panic!("{} command failed to start: {}", label, err);
    });
    let combined = combined_output(&output);
    assert!(
        !output.status.success(),
        "{} must reject the E32B-017 absolute import PoC; combined output: {}",
        label,
        combined
    );
    assert!(
        combined.contains(&format!("Import path '{}'", import_token.display())),
        "{} rejection must include the original absolute import token; combined output: {}",
        label,
        combined
    );
    assert!(
        combined.contains("resolves outside the project root")
            && combined.contains("Path traversal beyond the project boundary is not allowed"),
        "{} rejection must use the canonical SEC-003 path-boundary diagnostic; combined output: {}",
        label,
        combined
    );
}

#[test]
fn e32b_017_home_taida_poc_rejected_by_all_build_paths() {
    let (home, main, secret) = write_poc_layout();
    let bin = common::taida_bin();

    let mut interpreter = Command::new(&bin);
    interpreter.env("HOME", &home).arg(&main);
    assert_rejects_absolute_escape("interpreter", interpreter, &secret);

    let mut js = Command::new(&bin);
    js.env("HOME", &home)
        .args(["build", "js"])
        .arg(&main)
        .arg("-o")
        .arg(home.join("out.mjs"));
    assert_rejects_absolute_escape("js", js, &secret);

    let mut native = Command::new(&bin);
    native
        .env("HOME", &home)
        .args(["build", "native"])
        .arg(&main)
        .arg("-o")
        .arg(home.join("out-native"));
    assert_rejects_absolute_escape("native", native, &secret);

    let mut wasm_min = Command::new(&bin);
    wasm_min
        .env("HOME", &home)
        .args(["build", "wasm-min"])
        .arg(&main)
        .arg("-o")
        .arg(home.join("out.wasm"));
    assert_rejects_absolute_escape("wasm-min", wasm_min, &secret);

    let _ = fs::remove_dir_all(home);
}
