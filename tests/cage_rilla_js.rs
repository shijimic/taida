mod common;

use common::{node_available, taida_bin, unique_temp_dir, write_file};
use std::process::Command;

fn build_and_run_js(source: &str) -> String {
    let dir = unique_temp_dir("cage_rilla_js");
    let td = dir.join("main.td");
    let js = dir.join("main.mjs");
    write_file(&td, source.trim_start());

    let build = Command::new(taida_bin())
        .args(["build", "js"])
        .arg(&td)
        .arg("-o")
        .arg(&js)
        .output()
        .expect("run taida build js");
    assert!(
        build.status.success(),
        "taida build js failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );

    let run = Command::new("node")
        .arg(&js)
        .output()
        .expect("run generated JS");
    assert!(
        run.status.success(),
        "node failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );

    String::from_utf8_lossy(&run.stdout)
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn cage_rilla_js_call_and_get_execute_through_cage() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let stdout = build_and_run_js(
        r#"
>>> npm:node:path => @(basename)
>>> npm:node:os => @(constants)

file <= Cage[basename, JSCall[@[], @["/tmp/e33-cage-rilla.txt"], Str]()]()
file ]=> fileName
stdout(fileName)

sig <= Cage[constants, JSGet[@["signals", "SIGTERM"], Int]()]()
sig ]=> sigterm
stdout(sigterm.toString())
"#,
    );

    assert_eq!(stdout, "e33-cage-rilla.txt\n15");
}

#[test]
fn cage_rilla_js_error_info_and_typename_are_available() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let stdout = build_and_run_js(
        r#"
>>> npm:node:path => @(basename)

Thing = @(name: Str)
Enum => Status = :Ok :Fail

stdout(TypeName[Thing(name <= "box")]())
stdout(TypeName[Status:Fail()]())

bad <= Cage[basename, JSCall[@["missing"], @[], Str]()]()
bad.errorInfo() ]=> info
stdout(info.type)
"#,
    );

    assert_eq!(stdout, "Thing\nFail\nJSError");
}
