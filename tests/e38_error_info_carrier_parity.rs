mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn wasmtime_available() -> bool {
    Command::new("wasmtime")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixture_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "error_info_carrier_{}_{}_{}",
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

fn run_four_backends(main_path: &Path, dir: &Path) -> [(String, String); 4] {
    assert!(
        node_available(),
        "node is required for 4-backend E38 parity"
    );
    assert!(cc_available(), "cc is required for 4-backend E38 parity");
    assert!(
        wasmtime_available(),
        "wasmtime is required for 4-backend E38 parity"
    );

    let mut interp_cmd = Command::new(taida_bin());
    interp_cmd.arg(main_path);
    let interp = run_command_stdout(interp_cmd, "interp run");

    let mjs = dir.join("main.mjs");
    let mut build_cmd = Command::new(taida_bin());
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
    let mut build_cmd = Command::new(taida_bin());
    build_cmd
        .args(["build", "native"])
        .arg(main_path)
        .arg("-o")
        .arg(&bin);
    run_command_stdout(build_cmd, "native build");
    let run_cmd = Command::new(&bin);
    let native = run_command_stdout(run_cmd, "native run");

    let wasm = dir.join("main.wasm");
    let mut build_cmd = Command::new(taida_bin());
    build_cmd
        .args(["build", "wasm-full"])
        .arg(main_path)
        .arg("-o")
        .arg(&wasm);
    run_command_stdout(build_cmd, "wasm-full build");
    let mut run_cmd = Command::new("wasmtime");
    run_cmd.arg(&wasm);
    let wasm_full = run_command_stdout(run_cmd, "wasm-full run");

    [
        ("interp".to_string(), interp),
        ("js".to_string(), js),
        ("native".to_string(), native),
        ("wasm-full".to_string(), wasm_full),
    ]
}

fn assert_four_backends_agree(results: &[(String, String); 4], expected: &str) {
    for (backend, out) in results {
        assert_eq!(out, expected, "{} backend output mismatch", backend);
    }
}

#[test]
fn json_parse_failure_carries_error_info_across_backends() {
    let dir = fixture_dir("json_parse");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "bad <= JSON[\"not valid json\", Int]()\n\
         info <= bad.errorInfo()\n\
         stdout(bad.hasValue().toString())\n\
         stdout(info.hasValue().toString())\n\
         info ]=> err\n\
         stdout(err.type)\n\
         stdout(err.kind)\n\
         stdout(err.code.toString())\n\
         stdout(err.message)\n\
         stdout(bad)\n\
         stdout(bad.toString())\n\
         stdout(jsonEncode(bad))\n\
         stdout(bad.getOrDefault(99).toString())\n\
         bad ]=> value\n\
         stdout(value.toString())\n\
         mapped <= bad.map(_ x = x)\n\
         mappedInfo <= mapped.errorInfo()\n\
         stdout(mappedInfo.hasValue().toString())\n\
         flat <= bad.flatMap(_ x = Lax[x]())\n\
         flatInfo <= flat.errorInfo()\n\
         stdout(flatInfo.hasValue().toString())\n",
    )
    .expect("write main");

    let results = run_four_backends(&main, &dir);
    assert_four_backends_agree(
        &results,
        "false\ntrue\nJsonError\nparse\n0\nJSON parse error: invalid input\n@(has_value <= false, __value <= 0, __default <= 0, __type <= \"Lax\")\nLax(default: 0)\n{\"__default\":0,\"__value\":0,\"has_value\":false}\n99\n0\ntrue\ntrue",
    );
    let _ = fs::remove_dir_all(&dir);
}
