//! E32 build descriptor driver regression tests.

mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::path::Path;
use std::process::Command;

fn project(label: &str) -> std::path::PathBuf {
    let dir = unique_temp_dir(label);
    write_file(&dir.join("packages.tdm"), "");
    dir
}

fn run_taida_build(project: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(taida_bin());
    cmd.current_dir(project).arg("build").args(args);
    cmd.output().expect("taida build descriptor")
}

fn stderr_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout_text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn write_basic_entries(dir: &Path) {
    write_file(
        &dir.join("shared.td"),
        "sharedValue <= \"shared\"\n<<< sharedValue\n",
    );
    write_file(
        &dir.join("server.td"),
        ">>> ./shared.td => @(sharedValue)\nstdout(sharedValue)\n",
    );
    write_file(
        &dir.join("frontend.td"),
        ">>> ./shared.td => @(sharedValue)\nstdout(sharedValue)\n",
    );
}

#[test]
fn e32_descriptor_native_server_builds_wasm_dependency_first() {
    let dir = project("e32_descriptor_native_wasm_route");
    write_basic_entries(&dir);
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
>>> ./frontend.td => @(frontendMain)

frontendA <= BuildUnit(
  name <= "frontend-a",
  target <= "wasm-min",
  entry <= frontendMain
)

serverX <= BuildUnit(
  name <= "server-x",
  target <= "native",
  entry <= serverMain,
  assets <= @[
    RouteAsset(path <= "/app.wasm", unit <= frontendA)
  ]
)

<<< serverX
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--unit", "server-x"]);
    assert!(
        output.status.success(),
        "descriptor build failed\nstdout={}\nstderr={}",
        stdout_text(&output),
        stderr_text(&output)
    );

    assert!(
        dir.join(".taida/build/wasm-min/frontend-a/frontend.wasm")
            .exists(),
        "wasm dependency should be built before server"
    );
    assert!(
        dir.join(".taida/build/native/server-x/server-x").exists(),
        "native server artifact should be committed"
    );
    let map = fs::read_to_string(dir.join(".taida/build/artifact-map.json")).unwrap();
    assert!(map.contains("\"artifact_graph_version\": 1"));
    assert!(map.contains("\"dependencies\": [\n        \"frontend-a\""));
    assert!(map.contains("\"output\": \"wasm-min/frontend-a/frontend.wasm\""));
    let wasm_tx =
        fs::read_to_string(dir.join(".taida/build/wasm-min/frontend-a/.transaction-id")).unwrap();
    let native_tx =
        fs::read_to_string(dir.join(".taida/build/native/server-x/.transaction-id")).unwrap();
    assert_eq!(wasm_tx, native_tx);
    assert!(map.contains(&format!("\"transaction_id\": \"{}\"", wasm_tx)));
}

#[test]
fn e32_descriptor_asset_bundle_copies_bytes_and_map() {
    let dir = project("e32_descriptor_asset_copy");
    fs::create_dir_all(dir.join("nextjs-app/out/sub")).unwrap();
    write_file(&dir.join("nextjs-app/out/index.html"), "<h1>Taida</h1>\n");
    write_file(&dir.join("nextjs-app/out/sub/app.css"), "body{color:red}\n");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)

frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "nextjs-app/out",
  files <= @["**/*"],
  output <= "assets/frontend"
)

serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= frontendAssets)]
)

<<< serverX
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--unit", "server-x"]);
    assert!(
        output.status.success(),
        "descriptor build failed\nstdout={}\nstderr={}",
        stdout_text(&output),
        stderr_text(&output)
    );
    assert_eq!(
        fs::read_to_string(dir.join(".taida/build/assets/frontend/index.html")).unwrap(),
        "<h1>Taida</h1>\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join(".taida/build/assets/frontend/sub/app.css")).unwrap(),
        "body{color:red}\n"
    );
    let map = fs::read_to_string(dir.join(".taida/build/artifact-map.json")).unwrap();
    assert!(map.contains("\"output\": \"assets/frontend/index.html\""));
    assert!(map.contains("\"path\": \"/\""));
    let asset_tx =
        fs::read_to_string(dir.join(".taida/build/assets/frontend/.transaction-id")).unwrap();
    assert!(map.contains(&format!("\"transaction_id\": \"{}\"", asset_tx)));
}

#[test]
fn e32_descriptor_asset_glob_escape_rejects_with_build_context() {
    let dir = project("e32_descriptor_asset_escape");
    fs::create_dir_all(dir.join("public")).unwrap();
    write_file(&dir.join("public/index.html"), "ok");
    write_file(&dir.join("secret.txt"), "secret");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "public",
  files <= @["../secret.txt"],
  output <= "assets/frontend"
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= frontendAssets)]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("\"code\":\"E1911\""), "stdout={stdout}");
    assert!(stdout.contains("\"unit\":\"server-x\""), "stdout={stdout}");
    assert!(
        stdout.contains("\"edge_kind\":\"AssetDependency\""),
        "stdout={stdout}"
    );
    assert!(stdout.contains("AssetBundle.files glob"), "stdout={stdout}");
}

#[test]
fn e32_descriptor_asset_absolute_root_rejects() {
    let dir = project("e32_descriptor_asset_absolute_root");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "/tmp",
  files <= @["**/*"]
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= frontendAssets)]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("\"code\":\"E1910\""), "stdout={stdout}");
    assert!(stdout.contains("AssetBundle.root"), "stdout={stdout}");
    assert!(stdout.contains("\"unit\":\"server-x\""), "stdout={stdout}");
}

#[cfg(unix)]
#[test]
fn e32_descriptor_asset_symlink_rejects() {
    use std::os::unix::fs::symlink;

    let dir = project("e32_descriptor_asset_symlink");
    fs::create_dir_all(dir.join("public")).unwrap();
    write_file(&dir.join("public/index.html"), "ok");
    write_file(&dir.join("secret.txt"), "secret");
    symlink(dir.join("secret.txt"), dir.join("public/leak.txt")).unwrap();
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "public",
  files <= @["**/*"]
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= frontendAssets)]
)
<<< serverX
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--unit", "server-x"]);
    assert!(!output.status.success());
    let stderr = stderr_text(&output);
    assert!(stderr.contains("[E1913]"), "stderr={stderr}");
    assert!(stderr.contains("symlink"), "stderr={stderr}");
}

#[test]
fn e32_descriptor_duplicate_route_path_rejects_with_context() {
    let dir = project("e32_descriptor_duplicate_route_path");
    fs::create_dir_all(dir.join("public")).unwrap();
    write_file(&dir.join("public/index.html"), "ok");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "public",
  files <= @["**/*"]
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[
    RouteAsset(path <= "/", asset <= frontendAssets),
    RouteAsset(path <= "/", asset <= frontendAssets)
  ]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("\"code\":\"E1915\""), "stdout={stdout}");
    assert!(
        stdout.contains("\"edge_kind\":\"AssetDependency\""),
        "stdout={stdout}"
    );
    assert!(stdout.contains("\"unit\":\"server-x\""), "stdout={stdout}");
}

#[test]
fn e32_descriptor_asset_output_collision_rejects() {
    let dir = project("e32_descriptor_asset_output_collision");
    fs::create_dir_all(dir.join("public-a")).unwrap();
    fs::create_dir_all(dir.join("public-b")).unwrap();
    write_file(&dir.join("public-a/index.html"), "a");
    write_file(&dir.join("public-b/index.html"), "b");
    write_file(
        &dir.join("main.td"),
        r#"
frontendA <= AssetBundle(
  name <= "frontend-a",
  root <= "public-a",
  files <= @["**/*"],
  output <= "assets/frontend"
)
frontendB <= AssetBundle(
  name <= "frontend-b",
  root <= "public-b",
  files <= @["**/*"],
  output <= "assets/frontend"
)
plan <= BuildPlan(
  name <= "web-release",
  units <= @[],
  assets <= @[frontendA, frontendB]
)
<<< plan
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--plan", "web-release", "--diag-format", "jsonl"],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("\"code\":\"E1914\""), "stdout={stdout}");
    assert!(stdout.contains("frontend-a"), "stdout={stdout}");
    assert!(stdout.contains("frontend-b"), "stdout={stdout}");
}

#[test]
fn e32_descriptor_failed_transaction_preserves_previous_output() {
    let dir = project("e32_descriptor_transaction");
    fs::create_dir_all(dir.join("public")).unwrap();
    write_file(&dir.join("public/index.html"), "v1");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    let descriptor = |glob: &str| {
        format!(
            r#"
>>> ./server.td => @(serverMain)
frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "public",
  files <= @["{}"],
  output <= "assets/frontend"
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= frontendAssets)]
)
<<< serverX
"#,
            glob
        )
    };
    write_file(&dir.join("main.td"), &descriptor("**/*"));
    let first = run_taida_build(&dir, &["main.td", "--unit", "server-x"]);
    assert!(first.status.success(), "stderr={}", stderr_text(&first));
    let artifact_map_before =
        fs::read_to_string(dir.join(".taida/build/artifact-map.json")).unwrap();

    write_file(&dir.join("main.td"), &descriptor("../secret.txt"));
    let second = run_taida_build(&dir, &["main.td", "--unit", "server-x"]);
    assert!(!second.status.success());
    assert_eq!(
        fs::read_to_string(dir.join(".taida/build/assets/frontend/index.html")).unwrap(),
        "v1"
    );
    assert_eq!(
        fs::read_to_string(dir.join(".taida/build/artifact-map.json")).unwrap(),
        artifact_map_before,
        "failed descriptor transaction must preserve committed artifact map"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn e32_descriptor_cleans_dead_pid_staging_before_commit() {
    let dir = project("e32_descriptor_stale_cleanup");
    let stale = dir.join(".taida/build/.tmp-stale");
    fs::create_dir_all(&stale).unwrap();
    write_file(
        &stale.join("transaction.json"),
        r#"{"transaction_id":"stale","pid":999999999}"#,
    );
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain
)
<<< serverX
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--unit", "server-x"]);
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        stdout_text(&output),
        stderr_text(&output)
    );
    assert!(
        !stale.exists(),
        "dead-pid staging directory should be removed"
    );
    let cleanup_log = fs::read_to_string(dir.join(".taida/build/.cleanup.log")).unwrap();
    assert!(cleanup_log.contains(".tmp-stale"), "log={cleanup_log}");
    assert!(cleanup_log.contains("dead-pid"), "log={cleanup_log}");
}

#[test]
fn e32_descriptor_wasm_closure_rejects_native_only_import_with_context() {
    let dir = project("e32_descriptor_target_closure");
    write_file(
        &dir.join("shared.td"),
        r#"
>>> taida-lang/os => @(readFile)
helper <= 1
<<< helper
"#,
    );
    write_file(
        &dir.join("frontend.td"),
        r#"
>>> ./shared.td => @(helper)
stdout(helper)
"#,
    );
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./frontend.td => @(frontendMain)
frontendA <= BuildUnit(
  name <= "frontend-a",
  target <= "wasm-min",
  entry <= frontendMain
)
<<< frontendA
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "frontend-a", "--diag-format", "jsonl"],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("\"code\":\"E1941\""), "stdout={stdout}");
    assert!(
        stdout.contains("\"target\":\"wasm-min\""),
        "stdout={stdout}"
    );
    assert!(
        stdout.contains("\"edge_kind\":\"NormalImport\""),
        "stdout={stdout}"
    );
    assert!(stdout.contains("taida-lang/os"), "stdout={stdout}");
}

#[test]
fn e32_descriptor_artifact_cycle_reports_dependency_path() {
    let dir = project("e32_descriptor_artifact_cycle");
    write_file(&dir.join("a.td"), "stdout(\"a\")\n");
    write_file(&dir.join("b.td"), "stdout(\"b\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./a.td => @(aMain)
>>> ./b.td => @(bMain)
aUnit <= BuildUnit(
  name <= "a",
  target <= "js",
  entry <= aMain,
  assets <= @[RouteAsset(path <= "/b", unit <= bUnit)]
)
bUnit <= BuildUnit(
  name <= "b",
  target <= "js",
  entry <= bMain,
  assets <= @[RouteAsset(path <= "/a", unit <= aUnit)]
)
<<< aUnit
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--unit", "a", "--diag-format", "jsonl"]);
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("\"code\":\"E1940\""), "stdout={stdout}");
    assert!(
        stdout.contains("\"edge_kind\":\"ArtifactDependency\""),
        "stdout={stdout}"
    );
    assert!(stdout.contains("a"), "stdout={stdout}");
    assert!(stdout.contains("b"), "stdout={stdout}");
}

#[test]
fn e32_descriptor_build_hook_requires_opt_in_and_then_runs() {
    let dir = project("e32_descriptor_hook");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)

genAssets <= BuildHook(
  name <= "gen-assets",
  command <= "mkdir -p generated && printf ${TAIDA_HOOK_MESSAGE} > generated/app.txt",
  cwd <= ".",
  env <= @[@(name <= "TAIDA_HOOK_MESSAGE", value <= "hook-output")]
)

frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "generated",
  files <= @["**/*"],
  output <= "assets/frontend",
  before <= @[genAssets]
)

serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= frontendAssets)]
)

<<< serverX
"#,
    );

    let disabled = run_taida_build(&dir, &["main.td", "--unit", "server-x"]);
    assert!(!disabled.status.success());
    assert!(
        stderr_text(&disabled).contains("[E1951]"),
        "stderr={}",
        stderr_text(&disabled)
    );

    let enabled = run_taida_build(&dir, &["main.td", "--unit", "server-x", "--run-hooks"]);
    assert!(
        enabled.status.success(),
        "stdout={}\nstderr={}",
        stdout_text(&enabled),
        stderr_text(&enabled)
    );
    assert_eq!(
        fs::read_to_string(dir.join(".taida/build/assets/frontend/app.txt")).unwrap(),
        "hook-output"
    );
    let hooks = fs::read_dir(dir.join(".taida/build/hooks/gen-assets"))
        .unwrap()
        .count();
    assert!(hooks >= 1, "hook log should be committed");
}

#[test]
fn e32_descriptor_build_hook_failure_reports_context() {
    let dir = project("e32_descriptor_hook_failure");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
failHook <= BuildHook(
  name <= "fail-hook",
  command <= "exit 7",
  cwd <= "."
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  before <= @[failHook]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &[
            "main.td",
            "--unit",
            "server-x",
            "--run-hooks",
            "--diag-format",
            "jsonl",
        ],
    );
    assert!(!output.status.success());
    let stdout = stdout_text(&output);
    assert!(stdout.contains("\"code\":\"E1952\""), "stdout={stdout}");
    assert!(
        stdout.contains("\"hook_name\":\"fail-hook\""),
        "stdout={stdout}"
    );
    assert!(stdout.contains("\"exit_code\":7"), "stdout={stdout}");
}

// =============================================================================
// E32B-036: descriptor `name` path traversal rejection ([E1916])
// =============================================================================
//
// `BuildUnit` / `BuildPlan` / `AssetBundle` / `BuildHook` の `name` は staging
// path / artifact-map key / hook log directory に直接使われる。攻撃者が
// `name <= "../../../../tmp/pwn"` のような traversal を埋め込むと、commit
// 時に project root の外へ書き出される。`parse_build_*` 直後の
// `validate_descriptor_name` で `[E1916]` を hard-fail させ、4 種類すべての
// descriptor で同じ policy を pin する。

fn assert_e1916(output: &std::process::Output, label: &str) {
    assert!(
        !output.status.success(),
        "{label} should reject descriptor name traversal"
    );
    let stdout = stdout_text(output);
    let stderr = stderr_text(output);
    assert!(
        stdout.contains("\"code\":\"E1916\"") || stderr.contains("[E1916]"),
        "{label} should report E1916, stdout={stdout} stderr={stderr}"
    );
}

#[test]
fn e32b_036_build_unit_name_path_traversal_rejected() {
    let dir = project("e32b_036_unit_traversal");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
serverX <= BuildUnit(
  name <= "../../../../tmp/pwn",
  target <= "js",
  entry <= serverMain
)
<<< serverX
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--all-units", "--diag-format", "jsonl"]);
    assert_e1916(&output, "BuildUnit.name traversal");

    // Defense-in-depth: confirm no project-external file was created.
    let escaped = std::path::Path::new("/tmp/pwn");
    assert!(
        !escaped.is_dir() || !escaped.join("server.mjs").exists(),
        "build must not have created a project-external artifact directory"
    );
}

#[test]
fn e32b_036_asset_bundle_name_path_traversal_rejected() {
    let dir = project("e32b_036_asset_traversal");
    fs::create_dir_all(dir.join("public")).unwrap();
    write_file(&dir.join("public/index.html"), "ok");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
frontendAssets <= AssetBundle(
  name <= "../../../../tmp/pwn-assets",
  root <= "public",
  files <= @["**/*"]
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= frontendAssets)]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert_e1916(&output, "AssetBundle.name traversal");
}

#[test]
fn e32b_036_build_hook_name_path_traversal_rejected() {
    let dir = project("e32b_036_hook_traversal");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
escapeHook <= BuildHook(
  name <= "../../../../tmp/pwn-hook",
  command <= "echo ok",
  cwd <= "."
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  before <= @[escapeHook]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert_e1916(&output, "BuildHook.name traversal");
}

#[test]
fn e32b_036_build_plan_name_path_traversal_rejected() {
    let dir = project("e32b_036_plan_traversal");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain
)
plan <= BuildPlan(
  name <= "../../../../tmp/pwn-plan",
  units <= @[serverX]
)
<<< plan
<<< serverX
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--all-units", "--diag-format", "jsonl"]);
    assert_e1916(&output, "BuildPlan.name traversal");
}

#[test]
fn e32b_036_build_unit_name_leading_dot_rejected() {
    let dir = project("e32b_036_unit_hidden");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
serverX <= BuildUnit(
  name <= ".hidden",
  target <= "js",
  entry <= serverMain
)
<<< serverX
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--all-units", "--diag-format", "jsonl"]);
    assert_e1916(&output, "BuildUnit.name leading-dot");
}

#[test]
fn e32b_036_build_unit_name_empty_rejected() {
    let dir = project("e32b_036_unit_empty");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
serverX <= BuildUnit(
  name <= "",
  target <= "js",
  entry <= serverMain
)
<<< serverX
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--all-units", "--diag-format", "jsonl"]);
    assert_e1916(&output, "BuildUnit.name empty");
}

// =============================================================================
// Descriptor `name` 重複 reject ([E1902])
// =============================================================================
//
// `BuildUnit` / `BuildPlan` / `AssetBundle` / `BuildHook` の `name` は CLI
// (`taida build --unit X` / `--plan Y`) や artifact map / hook log / docs lookup
// の鍵になる。同 name を異なる symbol で 2 つ定義すると、後勝ちで silent に
// 上書きされ、ユーザーがどちらが選ばれているか判別できない (silent foot-gun)。
// `build_descriptor_model` で `*_symbol_by_name.insert` の戻り値を見て、
// 4 種類すべての descriptor で `[E1902]` で hard-fail させる。

fn assert_e1902_duplicate_name(
    output: &std::process::Output,
    descriptor: &str,
    duplicate_name: &str,
) {
    assert!(
        !output.status.success(),
        "{descriptor} duplicate name '{duplicate_name}' must hard-fail"
    );
    let stdout = stdout_text(output);
    let stderr = stderr_text(output);
    assert!(
        stdout.contains("\"code\":\"E1902\"") || stderr.contains("[E1902]"),
        "{descriptor} duplicate '{duplicate_name}' must report E1902; stdout={stdout} stderr={stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains(duplicate_name),
        "{descriptor} duplicate diagnostic must mention '{duplicate_name}'; combined={combined}"
    );
    assert!(
        combined.contains("declared more than once"),
        "{descriptor} duplicate diagnostic must explain the conflict; combined={combined}"
    );
}

fn assert_e1902_duplicate_symbol(
    output: &std::process::Output,
    descriptor: &str,
    duplicate_symbol: &str,
) {
    assert!(
        !output.status.success(),
        "{descriptor} duplicate symbol '{duplicate_symbol}' must hard-fail"
    );
    let stdout = stdout_text(output);
    let stderr = stderr_text(output);
    assert!(
        stdout.contains("\"code\":\"E1902\"") || stderr.contains("[E1902]"),
        "{descriptor} duplicate symbol '{duplicate_symbol}' must report E1902; stdout={stdout} stderr={stderr}"
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains(duplicate_symbol),
        "{descriptor} duplicate-symbol diagnostic must mention '{duplicate_symbol}'; combined={combined}"
    );
    assert!(
        combined.contains("bound more than once"),
        "{descriptor} duplicate-symbol diagnostic must explain the conflict; combined={combined}"
    );
}

#[test]
fn e32b_056_build_unit_duplicate_name_rejected() {
    let dir = project("e32b_056_unit_dup");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(&dir.join("frontend.td"), "stdout(\"frontend\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
>>> ./frontend.td => @(frontendMain)
serverA <= BuildUnit(
  name <= "duplicate-unit",
  target <= "js",
  entry <= serverMain
)
serverB <= BuildUnit(
  name <= "duplicate-unit",
  target <= "js",
  entry <= frontendMain
)
<<< serverA
<<< serverB
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--all-units", "--diag-format", "jsonl"]);
    assert_e1902_duplicate_name(&output, "BuildUnit", "duplicate-unit");
}

#[test]
fn e32b_056_build_plan_duplicate_name_rejected() {
    let dir = project("e32b_056_plan_dup");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain
)
planA <= BuildPlan(
  name <= "duplicate-plan",
  units <= @[serverX]
)
planB <= BuildPlan(
  name <= "duplicate-plan",
  units <= @[serverX]
)
<<< serverX
<<< planA
<<< planB
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--all-units", "--diag-format", "jsonl"]);
    assert_e1902_duplicate_name(&output, "BuildPlan", "duplicate-plan");
}

#[test]
fn e32b_056_asset_bundle_duplicate_name_rejected() {
    let dir = project("e32b_056_asset_dup");
    fs::create_dir_all(dir.join("public")).unwrap();
    write_file(&dir.join("public/index.html"), "ok");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
assetsA <= AssetBundle(
  name <= "duplicate-assets",
  root <= "public",
  files <= @["**/*"]
)
assetsB <= AssetBundle(
  name <= "duplicate-assets",
  root <= "public",
  files <= @["**/*"]
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= assetsA)]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert_e1902_duplicate_name(&output, "AssetBundle", "duplicate-assets");
}

#[test]
fn e32b_056_build_hook_duplicate_name_rejected() {
    let dir = project("e32b_056_hook_dup");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
hookA <= BuildHook(
  name <= "duplicate-hook",
  command <= "echo a",
  cwd <= "."
)
hookB <= BuildHook(
  name <= "duplicate-hook",
  command <= "echo b",
  cwd <= "."
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  before <= @[hookA]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert_e1902_duplicate_name(&output, "BuildHook", "duplicate-hook");
}

// =============================================================================
// 同 symbol への二重定義 reject ([E1902])
// =============================================================================
//
// `unitName <= BuildUnit(name <= "x", ...)` を同じ `unitName` で 2 度定義すると
// `units_by_symbol[symbol]` が **silent overwrite** され、`unit_symbol_by_name`
// に旧 name の stale alias が残る (例: `unit_symbol_by_name["x"] -> unitName`
// が残ったまま `units_by_symbol[unitName]` は name="y" に書き換わる)。`taida
// build --unit x` は stale alias 経由で後勝ち unit (name="y") を build する
// silent foot-gun になる。E32B-056 の name-collision reject と対称的に、
// symbol-collision も `[E1902]` で hard-fail させる。
#[test]
fn e32b_087_build_unit_duplicate_symbol_rejected() {
    let dir = project("e32b_087_unit_sym_dup");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(&dir.join("frontend.td"), "stdout(\"frontend\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
>>> ./frontend.td => @(frontendMain)
aUnit <= BuildUnit(
  name <= "x",
  target <= "js",
  entry <= serverMain
)
aUnit <= BuildUnit(
  name <= "y",
  target <= "js",
  entry <= frontendMain
)
<<< aUnit
"#,
    );

    let output = run_taida_build(&dir, &["main.td", "--unit", "x", "--diag-format", "jsonl"]);
    assert_e1902_duplicate_symbol(&output, "BuildUnit", "aUnit");
}

#[test]
fn e32b_087_build_plan_duplicate_symbol_rejected() {
    let dir = project("e32b_087_plan_sym_dup");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain
)
aPlan <= BuildPlan(
  name <= "plan-a",
  units <= @[serverX]
)
aPlan <= BuildPlan(
  name <= "plan-b",
  units <= @[serverX]
)
<<< serverX
<<< aPlan
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--plan", "plan-a", "--diag-format", "jsonl"],
    );
    assert_e1902_duplicate_symbol(&output, "BuildPlan", "aPlan");
}

#[test]
fn e32b_087_asset_bundle_duplicate_symbol_rejected() {
    let dir = project("e32b_087_asset_sym_dup");
    fs::create_dir_all(dir.join("public")).unwrap();
    write_file(&dir.join("public/index.html"), "ok");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
aAssets <= AssetBundle(
  name <= "assets-a",
  root <= "public",
  files <= @["**/*"]
)
aAssets <= AssetBundle(
  name <= "assets-b",
  root <= "public",
  files <= @["**/*"]
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= aAssets)]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert_e1902_duplicate_symbol(&output, "AssetBundle", "aAssets");
}

#[test]
fn e32b_087_build_hook_duplicate_symbol_rejected() {
    let dir = project("e32b_087_hook_sym_dup");
    write_file(&dir.join("server.td"), "stdout(\"server\")\n");
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./server.td => @(serverMain)
aHook <= BuildHook(
  name <= "hook-a",
  command <= "echo a",
  cwd <= "."
)
aHook <= BuildHook(
  name <= "hook-b",
  command <= "echo b",
  cwd <= "."
)
serverX <= BuildUnit(
  name <= "server-x",
  target <= "js",
  entry <= serverMain,
  before <= @[aHook]
)
<<< serverX
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "server-x", "--diag-format", "jsonl"],
    );
    assert_e1902_duplicate_symbol(&output, "BuildHook", "aHook");
}

// =============================================================================
// `validate_target_closure` parse error は silent skip ではなく `[E1941]` で hard-fail
// =============================================================================
//
// build descriptor driver は target × closure の互換違反 (`taida-lang/net` を
// wasm-min closure に含める等) を `[E1941]` で reject する責務を負う。closure
// module に parse error が混じると、validation を silent skip して進ませると
// 「対象 API が closure に入っているか」を確認できなくなる。`collect_local_modules`
// 段階で parse error 全般を `[E1941]` 化済だが、`validate_target_closure` の
// 内側の再 parse でも `continue` で握り潰さず TOCTOU race window 含めて hard-fail
// するよう defence-in-depth を入れる。
#[test]
fn e32b_055_closure_module_parse_error_rejected() {
    let dir = project("e32b_055_closure_parse_error");
    write_file(
        &dir.join("net_helper.td"),
        ">>> taida-lang/net@a.1 => @(httpServe)\nlet bad = (\n",
    );
    write_file(
        &dir.join("frontend.td"),
        ">>> ./net_helper.td => @(httpServe)\nstdout(\"frontend\")\n",
    );
    write_file(
        &dir.join("main.td"),
        r#"
>>> ./frontend.td => @(frontendMain)
frontendA <= BuildUnit(
  name <= "frontend-a",
  target <= "wasm-min",
  entry <= frontendMain
)
<<< frontendA
"#,
    );

    let output = run_taida_build(
        &dir,
        &["main.td", "--unit", "frontend-a", "--diag-format", "jsonl"],
    );
    assert!(
        !output.status.success(),
        "closure module with parse errors must hard-fail; stdout={} stderr={}",
        stdout_text(&output),
        stderr_text(&output)
    );
    let stdout = stdout_text(&output);
    let stderr = stderr_text(&output);
    assert!(
        stdout.contains("\"code\":\"E1941\"") || stderr.contains("[E1941]"),
        "parse-error closure must report E1941; stdout={stdout} stderr={stderr}"
    );
    let combined = format!("{stdout}{stderr}").to_ascii_lowercase();
    assert!(
        combined.contains("net_helper.td") && combined.contains("parse error"),
        "diagnostic must mention the offending module and parse error context; combined={combined}"
    );
}
