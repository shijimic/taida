mod common;

use common::taida_bin;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

fn write_file(path: &Path, content: &str) {
    fs::write(path, content).expect("failed to write file");
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn test_taida_todo_json_reports_ids_and_stats() {
    let dir = unique_temp_dir("taida_todo_cli");
    let src = r#"
a <= TODO[Int](id <= "TASK-1", task <= "first")
b <= TODO[Int](id <= "TASK-1", task <= "second", unm <= 2)
c <= TODO[Stub["shape TBD"]](id <= "TASK-2", task <= "third")
"#;
    write_file(&dir.join("main.td"), src);

    let output = Command::new(taida_bin())
        .arg("todo")
        .arg("--format")
        .arg("json")
        .arg(&dir)
        .output()
        .expect("failed to run taida todo");

    assert!(
        output.status.success(),
        "taida todo should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("todo output should be valid JSON");
    assert_eq!(json["total"].as_u64(), Some(3));

    let by_id = json["byId"]
        .as_array()
        .expect("byId should be an array")
        .iter()
        .map(|v| {
            (
                v["id"].as_str().unwrap_or("<null>").to_string(),
                v["count"].as_u64().unwrap_or(0),
            )
        })
        .collect::<std::collections::HashMap<String, u64>>();

    assert_eq!(by_id.get("TASK-1"), Some(&2));
    assert_eq!(by_id.get("TASK-2"), Some(&1));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_native_release_blocks_todo_and_stub() {
    let dir = unique_temp_dir("taida_release_build_native");
    let src = r#"
t <= TODO[Stub["ship later"]](id <= "REL-1", task <= "replace this")
t ]=> v
stdout(typeof(v))
"#;
    let input = dir.join("main.td");
    let bin = dir.join("app_bin");
    write_file(&input, src);

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg("--release")
        .arg(&input)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to run taida build --target native --release");

    assert!(
        !output.status.success(),
        "build --target native --release should fail when TODO/Stub exists"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Release gate failed"),
        "expected release gate message, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_release_blocks_todo_and_stub() {
    let dir = unique_temp_dir("taida_release_build");
    let src_dir = dir.join("src");
    let out_dir = dir.join("dist");
    fs::create_dir_all(&src_dir).expect("failed to create src dir");
    write_file(
        &src_dir.join("main.td"),
        r#"
x <= TODO[Int](id <= "REL-2", task <= "remove before release")
stdout(x.toString())
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg("--release")
        .arg("--outdir")
        .arg(&out_dir)
        .arg(&src_dir)
        .output()
        .expect("failed to run taida build --target js --release");

    assert!(
        !output.status.success(),
        "build --target js --release should fail when TODO/Stub exists"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Release gate failed"),
        "expected release gate message, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_native_release_blocks_todo_in_imported_module() {
    let dir = unique_temp_dir("taida_release_build_native_import");
    let main_td = dir.join("main.td");
    let dep_td = dir.join("dep.td");
    let bin = dir.join("app_bin");

    write_file(
        &main_td,
        r#"
>>> ./dep => @(v)
v ]=> out
stdout(out.toString())
"#,
    );
    write_file(
        &dep_td,
        r#"
v <= TODO[Int](id <= "REL-DEP", task <= "must be removed")
<<< @(v)
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg("--release")
        .arg(&main_td)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to run taida build --target native --release");

    assert!(
        !output.status.success(),
        "build --target native --release should fail when imported module has TODO/Stub"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Release gate failed"),
        "expected release gate message, got: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_native_directory_default_entry() {
    let dir = unique_temp_dir("taida_build_native_dir_default");
    let project = dir.join("proj");
    let bin = dir.join("app_bin");
    fs::create_dir_all(&project).expect("failed to create project dir");
    write_file(
        &project.join("main.td"),
        r#"
stdout("hello native dir")
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&project)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to run taida build --target native <DIR>");

    assert!(
        output.status.success(),
        "build should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bin.exists(), "expected native output binary to exist");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_native_directory_entry_override() {
    let dir = unique_temp_dir("taida_build_native_dir_entry");
    let project = dir.join("proj");
    let bin = dir.join("app_bin");
    fs::create_dir_all(&project).expect("failed to create project dir");
    write_file(
        &project.join("main.td"),
        r#"
stdout("default entry")
"#,
    );
    write_file(
        &project.join("custom_entry.td"),
        r#"
stdout("custom entry")
"#,
    );

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&project)
        .arg("--entry")
        .arg("custom_entry")
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("failed to run taida build --target native <DIR> --entry");

    assert!(
        output.status.success(),
        "build should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(bin.exists(), "expected native output binary to exist");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_compile_command_removed() {
    let output = Command::new(taida_bin())
        .arg("compile")
        .arg("dummy.td")
        .output()
        .expect("failed to run taida compile");

    assert!(
        !output.status.success(),
        "compile should fail with non-zero exit code"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("`taida compile` has been removed"),
        "expected removal error, got: {}",
        stderr
    );
    assert!(
        stderr.contains("taida build --target native"),
        "expected migration hint, got: {}",
        stderr
    );
}

#[test]
fn test_transpile_alias_produces_js_output() {
    let dir = unique_temp_dir("taida_transpile_e2e");
    let src = dir.join("main.td");
    let js_out = dir.join("main.mjs");
    write_file(&src, "stdout(\"transpile works\")\n");

    let output = Command::new(taida_bin())
        .arg("transpile")
        .arg(&src)
        .arg("-o")
        .arg(&js_out)
        .output()
        .expect("failed to run taida transpile");

    assert!(
        output.status.success(),
        "transpile should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(js_out.exists(), "JS output should exist");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_transpile_command_help() {
    let output = Command::new(taida_bin())
        .arg("transpile")
        .arg("--help")
        .output()
        .expect("failed to run taida transpile --help");

    assert!(output.status.success(), "transpile --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("taida transpile"),
        "expected usage text, got: {}",
        stdout
    );
}

#[test]
fn test_top_level_help_prints_usage_and_commands() {
    let output = Command::new(taida_bin())
        .arg("--help")
        .output()
        .expect("failed to run taida --help");

    assert!(
        output.status.success(),
        "--help should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage:\n  taida [--no-check] <FILE>")
            && stdout.contains("Commands:")
            && stdout.contains("graph")
            && stdout.contains("Global options:"),
        "unexpected help output: {}",
        stdout
    );
}

#[test]
fn test_subcommand_help_prints_usage_and_exits_zero() {
    let workdir = unique_temp_dir("taida_subcommand_help");
    let cases = [
        (&["check", "--help"][..], "taida check [--json] <PATH>"),
        (
            &["build", "--help"][..],
            "taida build [--target js|native|wasm-min|wasm-wasi|wasm-edge|wasm-full]",
        ),
        (
            &["todo", "--help"][..],
            "taida todo [--format text|json] [PATH]",
        ),
        (
            &["graph", "--help"][..],
            "taida graph [-o OUTPUT] [--recursive] <PATH>",
        ),
        (
            &["verify", "--help"][..],
            "taida verify [--check CHECK] [--format FORMAT] <PATH>",
        ),
        (
            &["inspect", "--help"][..],
            "taida inspect [--format text|json|sarif] <PATH>",
        ),
        (&["init", "--help"][..], "taida init [DIR]"),
        (&["deps", "--help"][..], "taida deps"),
        (&["install", "--help"][..], "taida install"),
        (&["update", "--help"][..], "taida update"),
        (
            &["publish", "--help"][..],
            "taida publish [--label LABEL] [--dry-run]",
        ),
        (
            &["doc", "--help"][..],
            "taida doc generate [-o OUTPUT] <PATH>",
        ),
        (
            &["doc", "generate", "--help"][..],
            "taida doc generate [-o OUTPUT] <PATH>",
        ),
        (&["lsp", "--help"][..], "taida lsp"),
        (&["auth", "--help"][..], "taida auth <login|logout|status>"),
        (&["auth", "login", "--help"][..], "taida auth login"),
        (&["auth", "logout", "--help"][..], "taida auth logout"),
        (&["auth", "status", "--help"][..], "taida auth status"),
        (
            &["community", "--help"][..],
            "taida community <posts|post|messages|message|author>",
        ),
        (
            &["community", "posts", "--help"][..],
            "taida community posts [--tag <tag>] [--by <author>]",
        ),
        (
            &["community", "post", "--help"][..],
            "taida community post \"content\" [--tag <tag>...]",
        ),
        (
            &["community", "post", "hello", "--help"][..],
            "taida community post \"content\" [--tag <tag>...]",
        ),
        (
            &["community", "messages", "--help"][..],
            "taida community messages",
        ),
        (
            &["community", "message", "--help"][..],
            "taida community message --to <user> \"content\"",
        ),
        (
            &["community", "message", "--to", "alice", "hi", "--help"][..],
            "taida community message --to <user> \"content\"",
        ),
        (
            &["community", "author", "--help"][..],
            "taida community author [NAME]",
        ),
        (
            &["community", "author", "alice", "--help"][..],
            "taida community author [NAME]",
        ),
    ];

    for (args, expected) in cases {
        let output = Command::new(taida_bin())
            .current_dir(&workdir)
            .args(args)
            .output()
            .unwrap_or_else(|_| panic!("failed to run {}", args.join(" ")));

        assert!(
            output.status.success(),
            "{} should succeed: stderr={}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(expected),
            "unexpected help output for {}: {}",
            args.join(" "),
            stdout
        );
    }

    assert!(
        !workdir.join("--help").exists(),
        "init --help must not create a directory named --help"
    );

    let _ = fs::remove_dir_all(&workdir);
}

#[test]
fn test_graph_unknown_option_fails() {
    let output = Command::new(taida_bin())
        .arg("graph")
        .arg("--type")
        .arg("bad-view")
        .arg("examples/04_functions.td")
        .output()
        .expect("failed to run taida graph with unknown option");

    assert!(
        !output.status.success(),
        "graph should fail for unknown option --type"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown option '--type'"),
        "unexpected stderr: {}",
        stderr
    );
}

#[test]
fn test_graph_unknown_format_option_fails() {
    let output = Command::new(taida_bin())
        .arg("graph")
        .arg("--format")
        .arg("bad-format")
        .arg("examples/04_functions.td")
        .output()
        .expect("failed to run taida graph with unknown option");

    assert!(
        !output.status.success(),
        "graph should fail for unknown option --format"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown option '--format'"),
        "unexpected stderr: {}",
        stderr
    );
}

#[test]
fn test_verify_jsonl_outputs_findings_and_summary_and_sets_exit_code() {
    let dir = unique_temp_dir("taida_verify_jsonl");
    let src = dir.join("main.td");
    write_file(
        &src,
        r#"
risky x =
  Error(message <= "boom").throw()
=> :Str
"#,
    );

    let output = Command::new(taida_bin())
        .arg("verify")
        .arg("--format")
        .arg("jsonl")
        .arg(&src)
        .output()
        .expect("failed to run taida verify --format jsonl");

    assert!(
        !output.status.success(),
        "verify jsonl should exit non-zero when ERROR findings exist"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    // Verify at least one diagnostic line exists and each line has the expected JSON structure
    assert!(
        !lines.is_empty(),
        "jsonl output should contain at least one diagnostic line"
    );
    for line in &lines {
        let value: serde_json::Value =
            serde_json::from_str(line).expect("each jsonl line should be valid json");
        assert_eq!(value["schema"], "taida.diagnostic.v1");
        assert_eq!(value["stream"], "verify");
        assert!(value.get("code").is_some());
        assert!(value.get("message").is_some());
        assert!(value.get("location").is_some());
        assert!(value.get("suggestion").is_some());
    }
    let summary: serde_json::Value = serde_json::from_str(lines.last().copied().unwrap_or("{}"))
        .expect("summary line should be valid json");
    assert_eq!(summary["kind"], "summary");
    assert!(
        summary["summary"]["errors"].as_u64().unwrap_or(0) >= 1,
        "summary should include at least one error"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_diag_format_jsonl_outputs_parse_error_record() {
    let dir = unique_temp_dir("taida_build_jsonl_diag");
    let src = dir.join("broken.td");
    write_file(&src, "x <= ");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg("--diag-format")
        .arg("jsonl")
        .arg(&src)
        .output()
        .expect("failed to run taida build --diag-format jsonl");

    assert!(
        !output.status.success(),
        "build should fail for parse error in jsonl diag mode"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout
        .lines()
        .next()
        .expect("jsonl diagnostics should emit at least one line");
    let first: serde_json::Value =
        serde_json::from_str(first_line).expect("first diagnostic line should be valid json");
    assert_eq!(first["schema"], "taida.diagnostic.v1");
    assert_eq!(first["stream"], "compile");
    assert_eq!(first["kind"], "error");
    assert_eq!(first["stage"], "parse");
    assert_eq!(first["severity"], "ERROR");
    assert!(first.get("code").is_some());
    assert!(first.get("message").is_some());
    assert!(first.get("location").is_some());
    assert!(first.get("suggestion").is_some());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_outputs_machine_readable_summary() {
    let dir = unique_temp_dir("taida_check_json");
    let src = dir.join("main.td");
    write_file(
        &src,
        r#"
x <= 1
stdout(x.toString())
"#,
    );

    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&src)
        .output()
        .expect("failed to run taida check --json");

    assert!(
        output.status.success(),
        "check --json should succeed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("check --json output should be valid json");
    assert_eq!(value["schema"], "taida.check.v1");
    assert!(value["diagnostics"].is_array());
    assert_eq!(value["summary"]["files"].as_u64(), Some(1));
    assert_eq!(value["summary"]["errors"].as_u64(), Some(0));

    let _ = fs::remove_dir_all(&dir);
}

// ── C-8a: taida check --json emits E1501/E1502/E1503/E1504 ──

#[test]
fn test_check_json_e1501_same_scope_redefinition() {
    let dir = unique_temp_dir("taida_check_e1501");
    let src = dir.join("main.td");
    write_file(&src, "x <= 1\nx <= 2\n");

    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&src)
        .output()
        .expect("failed to run taida check --json");

    assert!(
        !output.status.success(),
        "check --json should fail for E1501"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("check --json output should be valid json");
    assert_eq!(value["schema"], "taida.check.v1");
    let diags = value["diagnostics"]
        .as_array()
        .expect("diagnostics should be array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1501"),
        "Expected E1501 in diagnostics, got: {:?}",
        diags
    );
    assert_eq!(value["summary"]["errors"].as_u64(), Some(1));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_e1502_old_placeholder_partial_application() {
    let dir = unique_temp_dir("taida_check_e1502");
    let src = dir.join("main.td");
    write_file(&src, "add x y = x\n=> :Int\nresult <= add(5, _)\n");

    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&src)
        .output()
        .expect("failed to run taida check --json");

    assert!(
        !output.status.success(),
        "check --json should fail for E1502"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("check --json output should be valid json");
    let diags = value["diagnostics"]
        .as_array()
        .expect("diagnostics should be array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1502"),
        "Expected E1502 in diagnostics, got: {:?}",
        diags
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_e1503_typedef_partial_application() {
    let dir = unique_temp_dir("taida_check_e1503");
    let src = dir.join("main.td");
    write_file(&src, "Point => @(x, y)\np <= Point(1, )\n");

    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&src)
        .output()
        .expect("failed to run taida check --json");

    assert!(
        !output.status.success(),
        "check --json should fail for E1503"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("check --json output should be valid json");
    let diags = value["diagnostics"]
        .as_array()
        .expect("diagnostics should be array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1503"),
        "Expected E1503 in diagnostics, got: {:?}",
        diags
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_e1504_mold_placeholder_outside_pipeline() {
    let dir = unique_temp_dir("taida_check_e1504");
    let src = dir.join("main.td");
    write_file(&src, "x <= Str[_]()\n");

    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&src)
        .output()
        .expect("failed to run taida check --json");

    assert!(
        !output.status.success(),
        "check --json should fail for E1504"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("check --json output should be valid json");
    let diags = value["diagnostics"]
        .as_array()
        .expect("diagnostics should be array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1504"),
        "Expected E1504 in diagnostics, got: {:?}",
        diags
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── C-8b: file/dir produce same format, summary, exit code ──

#[test]
fn test_check_json_file_vs_dir_format_consistency() {
    let dir = unique_temp_dir("taida_check_file_dir");
    let single_file = dir.join("single.td");
    let sub_dir = dir.join("sub");
    fs::create_dir_all(&sub_dir).expect("create sub dir");
    write_file(&single_file, "x <= 1\nx <= 2\n");
    write_file(&sub_dir.join("a.td"), "y <= 1\ny <= 2\n");

    // File input
    let file_out = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&single_file)
        .output()
        .expect("check --json file");

    // Dir input
    let dir_out = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&sub_dir)
        .output()
        .expect("check --json dir");

    // Both should fail with exit code != 0
    assert!(!file_out.status.success(), "file check should fail");
    assert!(!dir_out.status.success(), "dir check should fail");

    // Both should produce valid JSON with same schema
    let file_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&file_out.stdout))
            .expect("file output should be valid json");
    let dir_json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&dir_out.stdout))
            .expect("dir output should be valid json");

    assert_eq!(file_json["schema"], "taida.check.v1");
    assert_eq!(dir_json["schema"], "taida.check.v1");
    assert!(file_json["diagnostics"].is_array());
    assert!(dir_json["diagnostics"].is_array());
    assert!(file_json["summary"].is_object());
    assert!(dir_json["summary"].is_object());

    // Both JSON outputs should have the same field set in diagnostics
    let file_diag = &file_json["diagnostics"][0];
    let dir_diag = &dir_json["diagnostics"][0];
    for field in &[
        "stage",
        "severity",
        "code",
        "message",
        "location",
        "suggestion",
    ] {
        assert!(
            file_diag.get(*field).is_some(),
            "file diagnostic missing field: {}",
            field
        );
        assert!(
            dir_diag.get(*field).is_some(),
            "dir diagnostic missing field: {}",
            field
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_file_vs_dir_success_exit_code() {
    let dir = unique_temp_dir("taida_check_success_exit");
    let single_file = dir.join("ok.td");
    let sub_dir = dir.join("sub");
    fs::create_dir_all(&sub_dir).expect("create sub dir");
    write_file(&single_file, "x <= 1\nstdout(x.toString())\n");
    write_file(&sub_dir.join("ok.td"), "y <= 2\nstdout(y.toString())\n");

    let file_out = Command::new(taida_bin())
        .arg("check")
        .arg(&single_file)
        .output()
        .expect("check file");

    let dir_out = Command::new(taida_bin())
        .arg("check")
        .arg(&sub_dir)
        .output()
        .expect("check dir");

    assert!(file_out.status.success(), "file check should succeed");
    assert!(dir_out.status.success(), "dir check should succeed");

    let _ = fs::remove_dir_all(&dir);
}

// ── C-8c: build stops on checker failure ──

#[test]
fn test_build_stops_on_checker_error() {
    let dir = unique_temp_dir("taida_checker_stops_backend");
    let src = dir.join("main.td");
    let bin = dir.join("out_bin");
    let js_out = dir.join("out.mjs");
    write_file(&src, "x <= 1\nx <= 2\n");

    // build --target js
    let build_js = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&src)
        .arg("-o")
        .arg(&js_out)
        .output()
        .expect("build --target js");

    // build --target native
    let build_native = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build --target native");

    // Both should fail with the same checker error
    for (name, out) in &[
        ("build --target js", &build_js),
        ("build --target native", &build_native),
    ] {
        assert!(
            !out.status.success(),
            "{} should fail on checker error",
            name
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("[E1501]"),
            "{} should show E1501 error, got: {}",
            name,
            stderr
        );
    }

    // JS output file should NOT be created (backend didn't run)
    assert!(
        !js_out.exists(),
        "JS output should not exist when checker fails"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_fails_on_unresolved_package_import() {
    let dir = unique_temp_dir("taida_missing_pkg_import");
    let src = dir.join("main.td");
    let build_js_out = dir.join("build_out.mjs");

    write_file(&src, ">>> alice/missing => @(run)\nstdout(\"ok\")\n");
    write_file(&dir.join("packages.tdm"), ">>> alice/missing@a.1\n");

    let build_js = Command::new(taida_bin())
        .current_dir(&dir)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&src)
        .arg("-o")
        .arg(&build_js_out)
        .output()
        .expect("build --target js");

    assert!(
        !build_js.status.success(),
        "build --target js should fail on unresolved package import"
    );
    let stderr = String::from_utf8_lossy(&build_js.stderr);
    assert!(
        stderr.contains("Could not resolve package import 'alice/missing'"),
        "build --target js should surface the unresolved package import, got: {}",
        stderr
    );

    assert!(
        !build_js_out.exists(),
        "build output should not exist when package import resolution fails"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_resolves_package_import_from_source_root_with_custom_output() {
    if !node_available() {
        return;
    }

    let dir = unique_temp_dir("taida_pkg_import_success");
    let project = dir.join("project");
    let caller = dir.join("caller");
    let dist = dir.join("dist");
    let dep_dir = project
        .join(".taida")
        .join("deps")
        .join("alice")
        .join("pkg");
    fs::create_dir_all(&caller).expect("create caller dir");
    fs::create_dir_all(&dist).expect("create dist dir");
    fs::create_dir_all(&dep_dir).expect("create dep dir");

    write_file(&project.join("packages.tdm"), ">>> alice/pkg@a.1\n");
    write_file(
        &project.join("main.td"),
        ">>> alice/pkg => @(greet)\nstdout(greet())\n",
    );
    write_file(
        &dep_dir.join("main.td"),
        "greet =\n  \"hello from pkg\"\n=> :Str\n\n<<< @(greet)\n",
    );

    let js_out = dist.join("app.mjs");
    let build_out = Command::new(taida_bin())
        .current_dir(&caller)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(&js_out)
        .output()
        .expect("build --target js with custom output");

    assert!(
        build_out.status.success(),
        "build should succeed: {}",
        String::from_utf8_lossy(&build_out.stderr)
    );
    assert!(js_out.exists(), "expected JS output to exist");
    assert!(
        dep_dir.join("main.mjs").exists(),
        "dependency should be transpiled in-place"
    );

    let run_out = Command::new("node")
        .arg(&js_out)
        .output()
        .expect("node run");
    assert!(
        run_out.status.success(),
        "generated JS should run: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run_out.stdout).trim(),
        "hello from pkg"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_failure_does_not_leave_stale_local_module_outputs() {
    let dir = unique_temp_dir("taida_pkg_import_no_stale");
    let project = dir.join("project");
    let dist = dir.join("dist");
    fs::create_dir_all(&project).expect("create project dir");
    fs::create_dir_all(&dist).expect("create dist dir");

    write_file(&project.join("packages.tdm"), ">>> alice/missing@a.1\n");
    write_file(
        &project.join("main.td"),
        ">>> ./ok => @(value)\n>>> ./helper => @(run)\nstdout(value)\n",
    );
    write_file(&project.join("ok.td"), "value <= \"ok\"\n<<< @(value)\n");
    write_file(
        &project.join("helper.td"),
        ">>> alice/missing => @(missing)\nhelperValue =\n  \"bad\"\n=> :Str\n\n<<< @(helperValue)\n",
    );

    let build_out = Command::new(taida_bin())
        .current_dir(&project)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(dist.join("app.mjs"))
        .output()
        .expect("build --target js with unresolved package import");

    assert!(
        !build_out.status.success(),
        "build should fail on unresolved package import"
    );
    let stderr = String::from_utf8_lossy(&build_out.stderr);
    assert!(
        stderr.contains("Could not resolve package import 'alice/missing'"),
        "expected unresolved package import error, got: {}",
        stderr
    );
    assert!(
        !dist.join("app.mjs").exists(),
        "main output should not exist after failed build"
    );
    assert!(
        !dist.join("ok.mjs").exists(),
        "successfully staged earlier local module output should not leak after failed build"
    );
    assert!(
        !dist.join("helper.mjs").exists(),
        "local module output should not exist after failed build"
    );

    let emitted_mjs = fs::read_dir(&dist)
        .expect("read dist dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("mjs"))
        .count();
    assert_eq!(emitted_mjs, 0, "no final .mjs outputs should remain");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_js_failure_does_not_leave_stale_dependency_outputs() {
    let dir = unique_temp_dir("taida_pkg_import_no_stale_deps");
    let project = dir.join("project");
    let deps = project.join(".taida").join("deps").join("alice");
    fs::create_dir_all(&deps).expect("create deps root");

    write_file(
        &project.join("packages.tdm"),
        ">>> alice/good@a.1\n>>> alice/pkg@a.1\n>>> alice/missing@a.1\n",
    );
    write_file(
        &project.join("main.td"),
        ">>> alice/pkg => @(greet)\nstdout(greet())\n",
    );

    let good_dir = deps.join("good");
    let pkg_dir = deps.join("pkg");
    fs::create_dir_all(&good_dir).expect("create good dep dir");
    fs::create_dir_all(&pkg_dir).expect("create pkg dep dir");

    write_file(
        &good_dir.join("main.td"),
        "greet =\n  \"hello from good\"\n=> :Str\n\n<<< @(greet)\n",
    );
    write_file(
        &pkg_dir.join("main.td"),
        ">>> alice/good => @(greet)\n>>> alice/missing => @(missing)\n\nwelcome =\n  greet()\n=> :Str\n\n<<< @(welcome)\n",
    );

    let build_out = Command::new(taida_bin())
        .current_dir(&project)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("dist").join("app.mjs"))
        .output()
        .expect("build --target js with bad dep graph");

    assert!(
        !build_out.status.success(),
        "build should fail when a dependency import cannot be resolved"
    );
    let stderr = String::from_utf8_lossy(&build_out.stderr);
    assert!(
        stderr.contains("Could not resolve package import 'alice/missing'"),
        "expected unresolved dependency import error, got: {}",
        stderr
    );

    assert!(
        !good_dir.join("main.mjs").exists(),
        "successfully transpiled dependency output should not leak after failed build"
    );
    assert!(
        !pkg_dir.join("main.mjs").exists(),
        "failing dependency output should not exist after failed build"
    );
    assert!(
        !project.join("dist").join("app.mjs").exists(),
        "main output should not exist after failed build"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_build_diag_format_jsonl_emits_checker_error() {
    let dir = unique_temp_dir("taida_checker_jsonl");
    let src = dir.join("main.td");
    write_file(&src, "x <= 1\nx <= 2\n");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg("--diag-format")
        .arg("jsonl")
        .arg(&src)
        .output()
        .expect("build --diag-format jsonl");

    assert!(
        !output.status.success(),
        "build should fail with checker error in jsonl mode"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout
        .lines()
        .next()
        .expect("jsonl should emit at least one line");
    let diag: serde_json::Value =
        serde_json::from_str(first_line).expect("first jsonl line should be valid json");
    assert_eq!(diag["schema"], "taida.diagnostic.v1");
    assert_eq!(diag["stream"], "compile");
    assert_eq!(diag["kind"], "error");
    assert_eq!(diag["stage"], "type");
    assert_eq!(diag["code"], "E1501");

    let _ = fs::remove_dir_all(&dir);
}

// ── C-6a: same-scope duplicate 検出が CLI 経路（check/build）で一致する ──

#[test]
fn test_same_scope_duplicate_check_vs_build_consistency() {
    let dir = unique_temp_dir("taida_c6a_consistency");
    let src = dir.join("main.td");
    write_file(&src, "x <= 1\nx <= 2\nstdout(x.toString())\n");

    // taida check
    let check_out = Command::new(taida_bin())
        .arg("check")
        .arg(&src)
        .output()
        .expect("check");

    // taida build --target js
    let build_out = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&src)
        .output()
        .expect("build");

    // Both should fail
    assert!(!check_out.status.success(), "check should fail for E1501");
    assert!(!build_out.status.success(), "build should fail for E1501");

    // Both should report E1501
    let check_stderr = String::from_utf8_lossy(&check_out.stderr);
    let build_stderr = String::from_utf8_lossy(&build_out.stderr);
    assert!(
        check_stderr.contains("[E1501]")
            || String::from_utf8_lossy(&check_out.stdout).contains("E1501"),
        "check should report E1501, got stderr: {}, stdout: {}",
        check_stderr,
        String::from_utf8_lossy(&check_out.stdout)
    );
    assert!(
        build_stderr.contains("[E1501]"),
        "build should report E1501, got: {}",
        build_stderr
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── C-11c: taida check --json の回帰テスト ──

#[test]
fn test_check_json_regression_clean_file() {
    // C-11c: Clean file produces no diagnostics
    let dir = unique_temp_dir("taida_c11c_clean");
    let src = dir.join("main.td");
    write_file(&src, "x <= 42\nstdout(x.toString())\n");

    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&src)
        .output()
        .expect("check --json");

    assert!(
        output.status.success(),
        "check --json clean file should succeed"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["schema"], "taida.check.v1");
    assert_eq!(value["summary"]["errors"].as_u64(), Some(0));
    assert!(
        value["diagnostics"]
            .as_array()
            .expect("diagnostics should be a JSON array")
            .is_empty()
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_check_json_regression_multiple_errors() {
    // C-11c: Multiple errors produce correct count
    let dir = unique_temp_dir("taida_c11c_multi");
    let src = dir.join("main.td");
    write_file(&src, "x <= 1\nx <= 2\ny <= 3\ny <= 4\n");

    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(&src)
        .output()
        .expect("check --json");

    assert!(
        !output.status.success(),
        "check --json should fail with errors"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(value["schema"], "taida.check.v1");
    let diags = value["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diags.len() >= 2,
        "Expected at least 2 diagnostics, got {}",
        diags.len()
    );
    assert!(
        diags.iter().all(|d| d["code"] == "E1501"),
        "All diagnostics should be E1501"
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── C-11d: examples/quality/ の checker 用ケースを regression 化 ──

#[test]
fn test_quality_e2d_mold_partial_direct_is_rejected() {
    // C-11d: e2d_mold_partial_direct.td should be rejected by checker (E1504)
    let path = "examples/quality/e2d_mold_partial_direct.td";
    if !Path::new(path).exists() {
        return; // Skip if quality examples not present
    }
    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(path)
        .output()
        .expect("check quality file");

    assert!(
        !output.status.success(),
        "e2d should be rejected by checker"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let diags = value["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1504"),
        "Expected E1504 in e2d diagnostics, got: {:?}",
        diags
    );
}

#[test]
fn test_quality_e2f_duplicate_variable_is_rejected() {
    // C-11d: e2f_duplicate_variable_defs.td should be rejected by checker (E1501)
    let path = "examples/quality/e2f_duplicate_variable_defs.td";
    if !Path::new(path).exists() {
        return;
    }
    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(path)
        .output()
        .expect("check quality file");

    assert!(
        !output.status.success(),
        "e2f should be rejected by checker"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let diags = value["diagnostics"].as_array().expect("diagnostics array");
    assert!(
        diags.iter().any(|d| d["code"] == "E1501"),
        "Expected E1501 in e2f diagnostics, got: {:?}",
        diags
    );
}

#[test]
fn test_quality_e3a_name_collision_passes() {
    // C-11d: e3a_name_collision_check.td should PASS (demonstrates valid shadowing)
    let path = "examples/quality/e3a_name_collision_check.td";
    if !Path::new(path).exists() {
        return;
    }
    let output = Command::new(taida_bin())
        .arg("check")
        .arg("--json")
        .arg(path)
        .output()
        .expect("check quality file");

    assert!(
        output.status.success(),
        "e3a should pass checker (valid shadowing), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_graph_recursive_with_unknown_type_flag_errors() {
    // --type is no longer a valid option, should error
    let output = Command::new(taida_bin())
        .args([
            "graph",
            "--recursive",
            "--type",
            "dataflow",
            "examples/01_hello.td",
        ])
        .output()
        .expect("graph recursive with unknown type flag");

    assert!(
        !output.status.success(),
        "--type is an unknown option and should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown option '--type'"),
        "error should mention unknown option, got: {}",
        stderr
    );
}

// ── RC-5: CLI Command Quality Audit tests ───────────────

#[test]
fn test_rc5a_check_missing_path_errors() {
    let output = Command::new(taida_bin())
        .arg("check")
        .output()
        .expect("check with no path");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing <PATH>"),
        "should mention missing PATH, got: {}",
        stderr
    );
    assert!(
        stderr.contains("taida check --help"),
        "should suggest --help, got: {}",
        stderr
    );
}

#[test]
fn test_rc5b_verify_invalid_format_errors() {
    let output = Command::new(taida_bin())
        .args(["verify", "--format", "xml", "examples/01_hello.td"])
        .output()
        .expect("verify with invalid format");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown format 'xml'"),
        "should mention unknown format, got: {}",
        stderr
    );
}

#[test]
fn test_rc5b_verify_invalid_check_errors() {
    let output = Command::new(taida_bin())
        .args(["verify", "--check", "nonexistent", "examples/01_hello.td"])
        .output()
        .expect("verify with invalid check");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown check 'nonexistent'"),
        "should mention unknown check, got: {}",
        stderr
    );
    assert!(
        stderr.contains("error-coverage"),
        "should list available checks, got: {}",
        stderr
    );
}

#[test]
fn test_rc5b_verify_missing_path_errors() {
    let output = Command::new(taida_bin())
        .arg("verify")
        .output()
        .expect("verify with no path");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing <PATH>"),
        "should mention missing PATH, got: {}",
        stderr
    );
}

#[test]
fn test_rc5b_verify_valid_format_accepted() {
    for fmt in &["text", "json", "jsonl", "sarif"] {
        let output = Command::new(taida_bin())
            .args(["verify", "--format", fmt, "examples/01_hello.td"])
            .output()
            .unwrap_or_else(|_| panic!("verify --format {} should run", fmt));
        assert!(
            output.status.success(),
            "verify --format {} should succeed, stderr: {}",
            fmt,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_rc5b_verify_valid_check_accepted() {
    let output = Command::new(taida_bin())
        .args([
            "verify",
            "--check",
            "error-coverage",
            "examples/01_hello.td",
        ])
        .output()
        .expect("verify with valid check");
    assert!(
        output.status.success(),
        "verify --check error-coverage should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_rc5c_inspect_invalid_format_errors() {
    let output = Command::new(taida_bin())
        .args(["inspect", "--format", "yaml", "examples/01_hello.td"])
        .output()
        .expect("inspect with invalid format");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown format 'yaml'"),
        "should mention unknown format, got: {}",
        stderr
    );
}

#[test]
fn test_rc5c_inspect_valid_formats_accepted() {
    for fmt in &["text", "json", "sarif"] {
        let output = Command::new(taida_bin())
            .args(["inspect", "--format", fmt, "examples/01_hello.td"])
            .output()
            .unwrap_or_else(|_| panic!("inspect --format {} should run", fmt));
        assert!(
            output.status.success(),
            "inspect --format {} should succeed, stderr: {}",
            fmt,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_rc5c_inspect_missing_path_errors() {
    let output = Command::new(taida_bin())
        .arg("inspect")
        .output()
        .expect("inspect with no path");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing <PATH>"),
        "should mention missing PATH, got: {}",
        stderr
    );
}

#[test]
fn test_rc5d_todo_invalid_format_errors() {
    let output = Command::new(taida_bin())
        .args(["todo", "--format", "csv", "."])
        .output()
        .expect("todo with invalid format");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown format 'csv'"),
        "should mention unknown format, got: {}",
        stderr
    );
}

#[test]
fn test_rc5e_doc_without_generate_errors() {
    let output = Command::new(taida_bin())
        .arg("doc")
        .output()
        .expect("doc without subcommand");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("taida doc --help"),
        "should suggest --help, got: {}",
        stderr
    );
}

#[test]
fn test_rc5e_doc_invalid_subcommand_errors() {
    let output = Command::new(taida_bin())
        .args(["doc", "build"])
        .output()
        .expect("doc with invalid subcommand");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("taida doc --help"),
        "should suggest --help, got: {}",
        stderr
    );
}

#[test]
fn test_rc5_graph_missing_output_value_errors() {
    let output = Command::new(taida_bin())
        .args(["graph", "-o"])
        .output()
        .expect("graph with -o but no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for -o"),
        "should mention missing value, got: {}",
        stderr
    );
}

#[test]
fn test_rc5_graph_missing_path_errors() {
    let output = Command::new(taida_bin())
        .arg("graph")
        .output()
        .expect("graph with no path");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing <PATH>"),
        "should mention missing PATH, got: {}",
        stderr
    );
}

#[test]
fn test_rc5h_feature_gate_messages_consistent() {
    // auth and community (without feature) should mention 'community' feature.
    // With the feature enabled, they produce usage errors.
    // publish is excluded: with the feature enabled it proceeds to auth/manifest checks.
    for cmd in &["auth", "community"] {
        let output = Command::new(taida_bin())
            .arg(cmd)
            .output()
            .unwrap_or_else(|_| panic!("should run taida {}", cmd));
        assert!(
            !output.status.success(),
            "taida {} with no args should fail",
            cmd
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Without community feature: "requires the 'community' feature"
        // With community feature: usage error mentioning the subcommand
        assert!(
            stderr.contains("community") || stderr.contains("--help") || stderr.contains("Usage"),
            "taida {} stderr should mention 'community' feature or usage, got: {}",
            cmd,
            stderr
        );
    }
}

#[test]
fn test_rc5_todo_format_missing_value_errors() {
    let output = Command::new(taida_bin())
        .args(["todo", "--format"])
        .output()
        .expect("todo --format with no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for --format"),
        "should mention missing value, got: {}",
        stderr
    );
}

#[test]
fn test_rc5_verify_format_missing_value_errors() {
    let output = Command::new(taida_bin())
        .args(["verify", "--format"])
        .output()
        .expect("verify --format with no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for --format"),
        "should mention missing value, got: {}",
        stderr
    );
}

#[test]
fn test_rc5_verify_check_missing_value_errors() {
    let output = Command::new(taida_bin())
        .args(["verify", "--check"])
        .output()
        .expect("verify --check with no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for --check"),
        "should mention missing value, got: {}",
        stderr
    );
}

#[test]
fn test_rc5_inspect_format_missing_value_errors() {
    let output = Command::new(taida_bin())
        .args(["inspect", "--format"])
        .output()
        .expect("inspect --format with no value");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Missing value for --format"),
        "should mention missing value, got: {}",
        stderr
    );
}

// -----------------------------------------------------------------------
// T-1: WASM runtime cache — second build should hit cache (RC-8a)
// -----------------------------------------------------------------------

#[test]
fn test_rc8a_wasm_cache_hit_on_second_build() {
    let td = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/01_hello.td");
    if !td.exists() {
        return; // skip if examples missing
    }

    let tmp = unique_temp_dir("rc8a_cache_hit");
    let _ = fs::create_dir_all(&tmp);
    let wasm_out = tmp.join("hello.wasm");

    // First build — compiles runtime (cache miss)
    let out1 = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min", "-o"])
        .arg(&wasm_out)
        .arg(&td)
        .output()
        .expect("first wasm build");
    assert!(
        out1.status.success(),
        "first build failed: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    assert!(
        wasm_out.exists(),
        "wasm output should exist after first build"
    );

    let _ = fs::remove_file(&wasm_out);

    // Second build — should hit cache (faster, same result)
    let out2 = Command::new(taida_bin())
        .args(["build", "--target", "wasm-min", "-o"])
        .arg(&wasm_out)
        .arg(&td)
        .output()
        .expect("second wasm build");
    assert!(
        out2.status.success(),
        "second build (cache hit) failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    assert!(
        wasm_out.exists(),
        "wasm output should exist after cache hit build"
    );

    let _ = fs::remove_dir_all(&tmp);
}

// -----------------------------------------------------------------------
// T-2: `taida cache clean` (RC-8d)
// -----------------------------------------------------------------------

#[test]
fn test_rc8d_cache_clean_removes_files() {
    let cache_dir = PathBuf::from("target/wasm-rt-cache");
    let _ = fs::create_dir_all(&cache_dir);

    let fake_o = cache_dir.join("test_clean.deadbeef.o");
    let fake_tmp = cache_dir.join("test_clean.deadbeef.42.0.tmp.o");
    let _ = fs::write(&fake_o, b"fake");
    let _ = fs::write(&fake_tmp, b"fake");

    let output = Command::new(taida_bin())
        .args(["cache", "clean"])
        .output()
        .expect("cache clean");
    assert!(
        output.status.success(),
        "cache clean failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Cleaned") || stdout.contains("already clean"),
        "should report cleaning result, got: {}",
        stdout
    );

    assert!(!fake_o.exists(), "fake .o should be removed by cache clean");
    assert!(
        !fake_tmp.exists(),
        "fake .tmp.o should be removed by cache clean"
    );
}

#[test]
fn test_rc8d_cache_unknown_subcommand() {
    let output = Command::new(taida_bin())
        .args(["cache", "bogus"])
        .output()
        .expect("cache bogus");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown cache command"),
        "should reject unknown subcommand, got: {}",
        stderr
    );
}

#[test]
fn test_rc8d_cache_help() {
    let output = Command::new(taida_bin())
        .args(["cache", "--help"])
        .output()
        .expect("cache --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("clean"),
        "cache help should mention 'clean', got: {}",
        stdout
    );
}
