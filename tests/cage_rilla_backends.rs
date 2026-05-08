mod common;

use common::{node_available, taida_bin, unique_temp_dir, write_file};
use std::process::Command;

struct RunnerCase {
    name: &'static str,
    source: &'static str,
    expected: &'static str,
    runner: &'static str,
    reject_descriptor: &'static str,
}

const RUNNER_CASES: &[RunnerCase] = &[
    RunnerCase {
        name: "jsget",
        source: r#"
>>> npm:node:os => @(constants)
sig <= Cage[constants, JSGet[@["signals", "SIGTERM"], Int]()]()
sig ]=> value
stdout(value.toString())
"#,
        expected: "15",
        runner: "JSGet",
        reject_descriptor: r#"JSGet[@[], Str]"#,
    },
    RunnerCase {
        name: "jscall",
        source: r#"
>>> npm:node:path => @(basename)
file <= Cage[basename, JSCall[@[], @["/tmp/e33-cage-rilla.txt"], Str]()]()
file ]=> value
stdout(value)
"#,
        expected: "e33-cage-rilla.txt",
        runner: "JSCall",
        reject_descriptor: r#"JSCall[@[], @[], Str]"#,
    },
    RunnerCase {
        name: "jsnew",
        source: r#"
>>> npm:node:url => @(URL)
url <= Cage[URL, JSNew[@[], @["https://example.com/a"], Molten]()]()
url ]=> obj
href <= Cage[obj, JSGet[@["href"], Str]()]()
href ]=> value
stdout(value)
"#,
        expected: "https://example.com/a",
        runner: "JSNew",
        reject_descriptor: r#"JSNew[@[], @[], Molten]"#,
    },
    RunnerCase {
        name: "jsset",
        source: r#"
>>> npm:node:process => @(env)
set <= Cage[env, JSSet[@["TAIDA_E33_TEST"], "ok"]()]()
set ]=> env2
got <= Cage[env2, JSGet[@["TAIDA_E33_TEST"], Str]()]()
got ]=> value
stdout(value)
"#,
        expected: "ok",
        runner: "JSSet",
        reject_descriptor: r#"JSSet[@["x"], 1]"#,
    },
    RunnerCase {
        name: "jsbind",
        source: r#"
>>> npm:node:url => @(URL)
url <= Cage[URL, JSNew[@[], @["https://example.com/a"], Molten]()]()
url ]=> obj
bound <= Cage[obj, JSBind[@["toString"]]()]()
bound ]=> fn
called <= Cage[fn, JSCall[@[], @[], Str]()]()
called ]=> value
stdout(value)
"#,
        expected: "https://example.com/a",
        runner: "JSBind",
        reject_descriptor: r#"JSBind[@[]]"#,
    },
    RunnerCase {
        name: "jsspread",
        source: r#"
>>> npm:node:process => @(env)
merged <= Cage[env, JSSpread[@(TAIDA_E33_SPREAD <= "yes")]()]()
merged ]=> env2
got <= Cage[env2, JSGet[@["TAIDA_E33_SPREAD"], Str]()]()
got ]=> value
stdout(value)
"#,
        expected: "yes",
        runner: "JSSpread",
        reject_descriptor: r#"JSSpread[@(x <= 1)]"#,
    },
];

fn write_source(name: &str, source: &str) -> std::path::PathBuf {
    let dir = unique_temp_dir(&format!("cage_rilla_backends_{}", name));
    let td = dir.join("main.td");
    write_file(&td, source.trim_start());
    td
}

fn non_js_reject_source(case: &RunnerCase) -> String {
    format!(
        r#"
>>> npm:node:path => @(basename)
result <= Cage[basename, {}()]()
"#,
        case.reject_descriptor
    )
}

#[test]
fn cage_rilla_js_backend_executes_runner_family() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for case in RUNNER_CASES {
        let td = write_source(case.name, case.source);
        let js = td.with_extension("mjs");
        let build = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td)
            .arg("-o")
            .arg(&js)
            .output()
            .expect("run taida build js");
        assert!(
            build.status.success(),
            "{}: taida build js failed\nstdout:\n{}\nstderr:\n{}",
            case.name,
            String::from_utf8_lossy(&build.stdout),
            String::from_utf8_lossy(&build.stderr)
        );

        let run = Command::new("node")
            .arg(&js)
            .output()
            .expect("run generated JS");
        assert!(
            run.status.success(),
            "{}: node failed\nstdout:\n{}\nstderr:\n{}",
            case.name,
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&run.stdout).trim_end(),
            case.expected
        );
    }
}

#[test]
fn cage_rilla_interpreter_rejects_js_backend_import() {
    let td = write_source("interpreter_reject", RUNNER_CASES[1].source);
    let output = Command::new(taida_bin())
        .arg(&td)
        .output()
        .expect("run taida interpreter");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "interpreter should reject JS backend import, but succeeded"
    );
    assert!(
        stderr.contains("npm imports are only available in the JS transpiler backend"),
        "interpreter error should mention JS-only npm imports, got: {}",
        stderr
    );
}

#[test]
fn cage_rilla_native_rejects_js_runner_family() {
    for case in RUNNER_CASES {
        let source = non_js_reject_source(case);
        let td = write_source(case.name, &source);
        let bin = td.with_extension("bin");
        let output = Command::new(taida_bin())
            .args(["build", "native"])
            .arg(&td)
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("run taida build native");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{}: native should reject JS runner, but succeeded",
            case.name
        );
        assert!(
            stderr.contains(&format!(
                "{} is only available in the JS transpiler backend",
                case.runner
            )),
            "{}: native error should mention JS-only runner, got: {}",
            case.name,
            stderr
        );
    }
}

#[test]
fn cage_rilla_wasm_min_rejects_js_runner_family() {
    for case in RUNNER_CASES {
        let source = non_js_reject_source(case);
        let td = write_source(case.name, &source);
        let wasm = td.with_extension("wasm");
        let output = Command::new(taida_bin())
            .args(["build", "wasm-min"])
            .arg(&td)
            .arg("-o")
            .arg(&wasm)
            .output()
            .expect("run taida build wasm-min");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{}: wasm-min should reject JS runner, but succeeded",
            case.name
        );
        assert!(
            stderr.contains(&format!(
                "{} is only available in the JS transpiler backend",
                case.runner
            )),
            "{}: wasm-min error should mention JS-only runner, got: {}",
            case.name,
            stderr
        );
    }
}

#[test]
fn cage_rilla_abstract_runner_rejects_in_js_backend() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for abstract_runner in ["CageRilla[JS, Str]()", "JSRilla[Str]()"] {
        let source = format!(
            r#"
>>> npm:node:path => @(basename)
result <= Cage[basename, {}]()
"#,
            abstract_runner
        );
        let td = write_source("abstract_reject", &source);
        let js = td.with_extension("mjs");
        let output = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(&td)
            .arg("-o")
            .arg(&js)
            .output()
            .expect("run taida build js");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{}: abstract runner should be rejected",
            abstract_runner
        );
        assert!(
            stderr.contains("abstract CageRilla descriptor"),
            "{}: error should mention abstract CageRilla descriptor, got: {}",
            abstract_runner,
            stderr
        );
    }
}

#[test]
fn cage_rilla_abstract_runner_rejects_in_native_backend() {
    for abstract_runner in ["CageRilla[JS, Str]()", "JSRilla[Str]()"] {
        let source = format!(
            r#"
>>> npm:node:path => @(basename)
result <= Cage[basename, {}]()
"#,
            abstract_runner
        );
        let td = write_source("abstract_native_reject", &source);
        let bin = td.with_extension("bin");
        let output = Command::new(taida_bin())
            .args(["build", "native"])
            .arg(&td)
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("run taida build native");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{}: native abstract runner should be rejected",
            abstract_runner
        );
        assert!(
            stderr.contains("abstract CageRilla descriptor"),
            "{}: native error should mention abstract CageRilla descriptor, got: {}",
            abstract_runner,
            stderr
        );
    }
}
