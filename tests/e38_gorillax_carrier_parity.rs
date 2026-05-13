mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "e38_gorillax_carrier_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("mkdir fixture");
    dir
}

fn run_command_stdout(mut cmd: Command, label: &str) -> String {
    let out = cmd.output().unwrap_or_else(|e| panic!("{label}: {e}"));
    assert!(
        out.status.success(),
        "{label} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn run_three_backends(main_path: &Path, dir: &Path) -> [(String, String); 3] {
    let mut interp_cmd = Command::new(common::taida_bin());
    interp_cmd.arg(main_path);
    let interp = run_command_stdout(interp_cmd, "interp run");

    let mjs = dir.join("main.mjs");
    let mut build_cmd = Command::new(common::taida_bin());
    build_cmd
        .args(["build", "js"])
        .arg(main_path)
        .arg("-o")
        .arg(&mjs);
    run_command_stdout(build_cmd, "js build");
    let mut run_cmd = Command::new("node");
    run_cmd.arg(&mjs);
    let js = run_command_stdout(run_cmd, "js run");

    let bin = dir.join("main.bin");
    let mut build_cmd = Command::new(common::taida_bin());
    build_cmd
        .args(["build", "native"])
        .arg(main_path)
        .arg("-o")
        .arg(&bin);
    run_command_stdout(build_cmd, "native build");
    let run_cmd = Command::new(&bin);
    let native = run_command_stdout(run_cmd, "native run");

    [
        ("interp".to_string(), interp),
        ("js".to_string(), js),
        ("native".to_string(), native),
    ]
}

#[test]
fn relaxed_gorillax_throw_uses_canonical_error_carrier() {
    let dir = fixture_dir("relaxed_throw");
    let main = dir.join("main.td");
    fs::write(
        &main,
        concat!(
            ">>> taida-lang/os => @(run)\n",
            "r <= run(\"/definitely/not/a/command\", @[])\n",
            "|== err: Error =\n",
            "  info <= err.errorInfo()\n",
            "  info ]=> e\n",
            "  encoded <= jsonEncode(err)\n",
            "  stdout(e.type)\n",
            "  stdout((e.message.contains(\"Relaxed gorilla escaped\")).toString())\n",
            "  stdout((encoded.contains(\"\\\"type\\\"\")).toString())\n",
            "  stdout((encoded.contains(\"\\\"message\\\"\")).toString())\n",
            "  stdout((encoded.contains(\"\\\"kind\\\"\")).toString())\n",
            "  stdout((encoded.contains(\"\\\"code\\\"\")).toString())\n",
            "  stdout((encoded.contains(\"\\\"cause\\\"\")).toString())\n",
            "=> :Unit\n",
            "r.relax() ]=> value\n",
        ),
    )
    .expect("write main");

    let results = run_three_backends(&main, &dir);
    for (backend, out) in results {
        assert_eq!(
            out, "RelaxedGorillaEscaped\ntrue\ntrue\ntrue\ntrue\ntrue\nfalse",
            "{} backend output mismatch",
            backend
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
