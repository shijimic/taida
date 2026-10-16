//! Regressions for package management and distribution: `taida init`,
//! project-root discovery, package import resolution, `ingot install`
//! (store refresh, prebuild placement, lockfile pins, release-age policy),
//! the `ingot publish` API diff, and `install.sh`.

mod common;
mod mock;

use common::{taida_bin, unique_temp_dir};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Run `taida <args>` in `dir` with `HOME` pointed at a private directory.
fn taida(dir: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(taida_bin())
        .args(args)
        .current_dir(dir)
        .env("HOME", home)
        .output()
        .expect("run taida")
}

fn sha256_hex(data: &[u8]) -> String {
    taida::crypto::sha256_hex_bytes(data)
}

// ── taida init --target rust-addon ──────────────────────────────────

#[test]
fn init_rust_addon_refuses_to_overwrite_an_existing_crate() {
    let dir = unique_temp_dir("f65_init_existing");
    let home = dir.join("home");
    let crate_dir = dir.join("existing-crate");
    fs::create_dir_all(crate_dir.join("src")).unwrap();
    fs::create_dir_all(crate_dir.join(".github/workflows")).unwrap();
    let files = [
        (
            "Cargo.toml",
            "[package]\nname = \"precious\"\nversion = \"9.9.9\"\n",
        ),
        (
            "src/lib.rs",
            "// my precious library code\npub fn precious() {}\n",
        ),
        ("README.md", "# Precious README\n"),
        (".gitignore", "secret.key\n"),
        (".github/workflows/release.yml", "name: my-own-release\n"),
    ];
    for (rel, content) in files {
        fs::write(crate_dir.join(rel), content).unwrap();
    }

    let out = taida(&crate_dir, &home, &["init", "--target", "rust-addon", "."]);
    assert!(
        !out.status.success(),
        "init must refuse: {}",
        stdout_of(&out)
    );
    let err = stderr_of(&out);
    assert!(err.contains("refusing to overwrite"), "{err}");
    assert!(
        err.contains("Cargo.toml") && err.contains("src/lib.rs"),
        "{err}"
    );
    for (rel, content) in files {
        assert_eq!(
            fs::read_to_string(crate_dir.join(rel)).unwrap(),
            content,
            "{rel} must be untouched"
        );
    }
    assert!(
        !crate_dir.join("packages.tdm").exists(),
        "nothing is written"
    );
    assert!(!crate_dir.join("native").exists(), "nothing is written");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn init_rust_addon_keeps_an_existing_readme_and_gitignore() {
    let dir = unique_temp_dir("f65_init_keep");
    let home = dir.join("home");
    let project = dir.join("kept");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("README.md"), "# Mine\n").unwrap();
    fs::write(project.join(".gitignore"), "mine\n").unwrap();

    let out = taida(&project, &home, &["init", "--target", "rust-addon", "."]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_eq!(
        fs::read_to_string(project.join("README.md")).unwrap(),
        "# Mine\n"
    );
    assert_eq!(
        fs::read_to_string(project.join(".gitignore")).unwrap(),
        "mine\n"
    );
    assert!(project.join("Cargo.toml").exists());
    assert!(project.join("taida/kept.td").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn init_never_writes_through_a_symlink() {
    let dir = unique_temp_dir("f65_init_symlink");
    let home = dir.join("home");
    let victim = dir.join("victim.txt");

    // rust-addon: a (dangling) symlink at a scaffold path aborts the init.
    let addon = dir.join("linked");
    fs::create_dir_all(addon.join("src")).unwrap();
    std::os::unix::fs::symlink(&victim, addon.join("src/lib.rs")).unwrap();
    let out = taida(&addon, &home, &["init", "--target", "rust-addon", "."]);
    assert!(!out.status.success());
    assert!(
        stderr_of(&out).contains("src/lib.rs"),
        "{}",
        stderr_of(&out)
    );
    assert!(!victim.exists(), "the link target must not be created");

    // Source-only: an optional file behind a symlink is skipped, not followed.
    let plain = dir.join("plain");
    fs::create_dir_all(&plain).unwrap();
    std::os::unix::fs::symlink(&victim, plain.join("main.td")).unwrap();
    let out = taida(&plain, &home, &["init", "."]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(!victim.exists(), "the link target must not be created");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn init_rust_addon_scaffold_is_consistent() {
    let dir = unique_temp_dir("f65_init_scaffold");
    let home = dir.join("home");
    let out = taida(&dir, &home, &["init", "--target", "rust-addon", "my-addon"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let project = dir.join("my-addon");

    // The manifest's package id names the facade file the loader reads.
    let manifest =
        taida::addon::manifest::parse_addon_manifest(&project.join("native/addon.toml")).unwrap();
    assert_eq!(manifest.package, "OWNER/my-addon");
    assert_eq!(manifest.library, "my_addon");
    let facade = fs::read_to_string(project.join("taida/my-addon.td")).unwrap();
    assert!(
        facade.contains("echo <= RustAddon[\"echo\"](arity <= 1)"),
        "the facade binds the addon function explicitly: {facade}"
    );
    assert!(facade.contains("<<< @(Echo)"), "{facade}");

    // The generated `echo` builds its result through the host instead of
    // handing the borrowed argument back (which the host would free twice).
    let lib = fs::read_to_string(project.join("src/lib.rs")).unwrap();
    assert!(!lib.contains("args_ptr as *mut"), "{lib}");
    assert!(lib.contains("HostValueBuilder::from_raw"), "{lib}");
    let readme = fs::read_to_string(project.join("README.md")).unwrap();
    assert!(
        readme.contains(">>> OWNER/my-addon@a.1 => @(Echo)"),
        "{readme}"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ── Project root in a shared temporary directory ────────────────────

/// Plant a project marker and a shadowing `taida-lang/os` package in
/// `shared`, as another local user could in `/tmp`.
fn plant_shared_marker(shared: &Path, marker: &str) {
    fs::write(shared.join(marker), "").unwrap();
    let evil = shared.join(".taida/deps/taida-lang/os");
    fs::create_dir_all(&evil).unwrap();
    fs::write(
        evil.join("main.td"),
        "stdout(\"PLANTED MODULE RAN\")\nEnvVar name: Str = \"spoofed\" => :Str\n<<< @(EnvVar)\n",
    )
    .unwrap();
}

const VICTIM_SCRIPT: &str = ">>> taida-lang/os => @(EnvVar)\nstdout(\"victim script running\")\n";

#[cfg(unix)]
#[test]
fn marker_in_a_sticky_world_writable_dir_is_not_a_project_root() {
    use std::os::unix::fs::PermissionsExt;
    let dir = unique_temp_dir("f65_shared_sticky");
    let home = dir.join("home");
    let shared = dir.join("shared");
    fs::create_dir_all(shared.join("victim")).unwrap();
    for marker in ["packages.tdm", "taida.toml"] {
        let _ = fs::remove_file(shared.join("packages.tdm"));
        let _ = fs::remove_file(shared.join("taida.toml"));
        plant_shared_marker(&shared, marker);
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
        fs::write(shared.join("victim/script.td"), VICTIM_SCRIPT).unwrap();
        fs::write(shared.join("direct.td"), VICTIM_SCRIPT).unwrap();

        for (cwd, script) in [
            (shared.join("victim"), "script.td"),
            (shared.clone(), "direct.td"),
        ] {
            let out = taida(&cwd, &home, &[script]);
            let stdout = stdout_of(&out);
            assert!(
                out.status.success() && !stdout.contains("PLANTED"),
                "{marker} / {script}: {stdout}{}",
                stderr_of(&out)
            );
            assert!(stdout.contains("victim script running"), "{stdout}");
        }

        // `ingot` commands do not adopt the shared manifest either.
        let out = taida(&shared.join("victim"), &home, &["ingot", "install"]);
        assert!(!out.status.success());
        assert!(
            stderr_of(&out).contains("No packages.tdm found"),
            "{}",
            stderr_of(&out)
        );
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn marker_in_the_system_temp_dir_is_not_a_project_root() {
    let dir = unique_temp_dir("f65_shared_tmpdir");
    let home = dir.join("home");
    let shared = dir.join("tmp");
    fs::create_dir_all(shared.join("victim")).unwrap();
    plant_shared_marker(&shared, "packages.tdm");
    fs::write(shared.join("victim/script.td"), VICTIM_SCRIPT).unwrap();

    let out = Command::new(taida_bin())
        .arg("script.td")
        .current_dir(shared.join("victim"))
        .env("HOME", &home)
        .env("TMPDIR", &shared)
        .output()
        .unwrap();
    let stdout = stdout_of(&out);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(!stdout.contains("PLANTED"), "{stdout}");

    // A normal project keeps working: its own packages.tdm is the root.
    let project = dir.join("proj");
    fs::create_dir_all(project.join("sub")).unwrap();
    fs::create_dir_all(project.join(".taida/deps/me/lib")).unwrap();
    fs::write(project.join("packages.tdm"), "<<<@a.1 me/proj\n").unwrap();
    fs::write(
        project.join(".taida/deps/me/lib/main.td"),
        "hi x: Str = \"hi \" + x => :Str\n<<< @(hi)\n",
    )
    .unwrap();
    fs::write(
        project.join("sub/app.td"),
        ">>> me/lib => @(hi)\nstdout(hi(\"there\"))\n",
    )
    .unwrap();
    let out = taida(&project.join("sub"), &home, &["app.td"]);
    assert_eq!(stdout_of(&out).trim(), "hi there", "{}", stderr_of(&out));
    let _ = fs::remove_dir_all(&dir);
}

// ── Package imports stay inside the installed package ──────────────

const OUTSIDE_MODULE: &str =
    "stdout(\"OUTSIDE MODULE RAN\")\nhello name: Str = `hello, ${name}` => :Str\n<<< @(hello)\n";

#[test]
fn package_import_with_dot_segments_does_not_leave_deps() {
    let dir = unique_temp_dir("f65_pkg_dotdot");
    let home = dir.join("home");
    fs::create_dir_all(dir.join("outside")).unwrap();
    fs::write(dir.join("outside/evil.td"), OUTSIDE_MODULE).unwrap();
    let app = dir.join("app");
    fs::create_dir_all(app.join(".taida/deps/x")).unwrap();
    fs::write(app.join("packages.tdm"), "").unwrap();
    fs::write(
        app.join("pkg.td"),
        // deps/x -> deps -> .taida -> app -> the temp dir holding `outside/`.
        ">>> x/../../../../outside/evil => @(hello)\nstdout(hello(\"pkg\"))\n",
    )
    .unwrap();

    let out = taida(&app, &home, &["pkg.td"]);
    assert!(!out.status.success());
    assert!(!stdout_of(&out).contains("OUTSIDE"), "{}", stdout_of(&out));
    assert!(stderr_of(&out).contains("not found"), "{}", stderr_of(&out));

    if cfg!(feature = "native") {
        let out = taida(&app, &home, &["build", "native", "pkg.td", "-o", "pkg.bin"]);
        assert!(!out.status.success(), "native build must not resolve it");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_dependency_imports_its_own_path_dependency_from_the_program_deps() {
    // `app` depends on `a`, which depends on `b` by path. `ingot install`
    // puts both in `app/.taida/deps`; `a`'s `>>> b` resolves there, not in
    // `a`'s own project root.
    let dir = unique_temp_dir("f65_pkg_transitive_path");
    let home = dir.join("home");
    let app = dir.join("app");
    for sub in ["app", "libs/a", "libs/b"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(
        app.join("packages.tdm"),
        "name <= \"app\"\nversion <= \"0.1.0\"\ndeps <= @(\n  a <= @(path <= \"../libs/a\")\n)\n",
    )
    .unwrap();
    fs::write(
        app.join("main.td"),
        ">>> a => @(outer)\nstdout(outer(\"w\"))\n",
    )
    .unwrap();
    fs::write(
        dir.join("libs/a/packages.tdm"),
        "name <= \"a\"\nversion <= \"0.1.0\"\ndeps <= @(\n  b <= @(path <= \"../b\")\n)\n",
    )
    .unwrap();
    fs::write(
        dir.join("libs/a/main.td"),
        ">>> b => @(inner)\nouter x: Str = inner(x) => :Str\n<<< @(outer)\n",
    )
    .unwrap();
    fs::write(
        dir.join("libs/b/main.td"),
        "inner x: Str = `inner(${x})` => :Str\n<<< @(inner)\n",
    )
    .unwrap();

    let install = taida(&app, &home, &["ingot", "install"]);
    assert!(install.status.success(), "{}", stderr_of(&install));
    let run = taida(&app, &home, &["main.td"]);
    assert!(run.status.success(), "{}", stderr_of(&run));
    assert_eq!(stdout_of(&run).trim_end(), "inner(w)");
    let check = taida(&app, &home, &["way", "check", "main.td"]);
    assert!(
        check.status.success(),
        "{}{}",
        stdout_of(&check),
        stderr_of(&check)
    );
    if cfg!(feature = "native") {
        let build = taida(
            &app,
            &home,
            &["build", "native", "main.td", "-o", "main.bin"],
        );
        assert!(build.status.success(), "{}", stderr_of(&build));
        let out = Command::new(app.join("main.bin")).output().unwrap();
        assert_eq!(stdout_of(&out).trim_end(), "inner(w)");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn dependency_entry_outside_its_package_is_not_loaded() {
    let dir = unique_temp_dir("f65_pkg_entry");
    let home = dir.join("home");
    fs::create_dir_all(dir.join("outside")).unwrap();
    fs::write(dir.join("outside/evil.td"), OUTSIDE_MODULE).unwrap();
    fs::create_dir_all(dir.join("libs/util")).unwrap();
    fs::write(
        dir.join("libs/util/packages.tdm"),
        format!(
            "name <= \"util\"\nversion <= \"0.1.0\"\nentry <= \"{}\"\n",
            dir.join("outside/evil.td").display()
        ),
    )
    .unwrap();
    let app = dir.join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("packages.tdm"),
        "name <= \"app\"\nversion <= \"0.1.0\"\ndeps <= @(\n  util <= @(path <= \"../libs/util\")\n)\n",
    )
    .unwrap();
    fs::write(
        app.join("main.td"),
        ">>> util => @(hello)\nstdout(hello(\"entry\"))\n",
    )
    .unwrap();

    let out = taida(&app, &home, &["ingot", "install"]);
    assert!(
        stderr_of(&out).contains("must be a relative path inside the package"),
        "install reports the bad entry: {}",
        stderr_of(&out)
    );
    let out = taida(&app, &home, &["main.td"]);
    assert!(!out.status.success());
    assert!(!stdout_of(&out).contains("OUTSIDE"), "{}", stdout_of(&out));
    let _ = fs::remove_dir_all(&dir);
}

// ── ingot install never writes through a `.taida` symlink ───────────

#[cfg(unix)]
#[test]
fn install_refuses_a_symlinked_dot_taida() {
    let dir = unique_temp_dir("f65_dot_taida_link");
    let home = dir.join("home");
    fs::create_dir_all(dir.join("victim/deps/notes")).unwrap();
    fs::write(dir.join("victim/deps/notes/diary.txt"), "precious").unwrap();
    let repo = dir.join("repo");
    fs::create_dir_all(repo.join("libs/util")).unwrap();
    std::os::unix::fs::symlink("../victim", repo.join(".taida")).unwrap();
    fs::write(
        repo.join("packages.tdm"),
        "name <= \"app\"\nversion <= \"0.1.0\"\ndeps <= @(\n  util <= @(path <= \"./libs/util\")\n)\n",
    )
    .unwrap();
    fs::write(
        repo.join("libs/util/main.td"),
        "hello name: Str = name => :Str\n<<< @(hello)\n",
    )
    .unwrap();

    let out = taida(&repo, &home, &["ingot", "install"]);
    assert!(!out.status.success());
    assert!(
        stderr_of(&out).contains("symbolic link"),
        "{}",
        stderr_of(&out)
    );
    assert_eq!(
        fs::read_to_string(dir.join("victim/deps/notes/diary.txt")).unwrap(),
        "precious"
    );
    assert!(!dir.join("victim/taida.lock").exists());
    let _ = fs::remove_dir_all(&dir);
}

// ── Release-age policy: invalid configuration fails closed ──────────

#[test]
fn invalid_min_release_age_in_config_stops_the_install() {
    let dir = unique_temp_dir("f65_release_age");
    let home = dir.join("home");
    fs::create_dir_all(home.join(".taida")).unwrap();
    let consumer = dir.join("consumer");
    let addon = dir.join("addon");
    fs::create_dir_all(consumer.join("prebuilt")).unwrap();
    fs::create_dir_all(addon.join("native")).unwrap();
    fs::create_dir_all(addon.join("taida")).unwrap();
    fs::write(
        consumer.join("packages.tdm"),
        "name <= \"consumer\"\nversion <= \"0.1.0\"\ndeps <= @(\n  demo <= @(path <= \"../addon\")\n)\n",
    )
    .unwrap();
    fs::write(
        addon.join("packages.tdm"),
        ">>> ./taida/addon.td\n<<<@a.1 local/demo @(greet)\n",
    )
    .unwrap();
    fs::write(
        addon.join("taida/addon.td"),
        "greet <= RustAddon[\"greet\"](arity <= 1)\n<<< @(greet)\n",
    )
    .unwrap();
    fs::write(consumer.join("prebuilt/libdemo.so"), "FAKE\n").unwrap();
    let sha = sha256_hex(b"FAKE\n");
    let Ok(host) = taida::addon::host_target::detect_host_target() else {
        return;
    };
    fs::write(
        addon.join("native/addon.toml"),
        format!(
            "abi = 1\nentry = \"taida_addon_get_v1\"\npackage = \"local/demo\"\nlibrary = \"demo\"\n\n[functions]\ngreet = 1\n\n[library.prebuild]\nurl = \"file://prebuilt/libdemo.so\"\n\n[library.prebuild.targets]\n\"{}\" = \"sha256:{sha}\"\n",
            host.as_triple()
        ),
    )
    .unwrap();

    for bad in ["1 month", "30 d"] {
        fs::write(
            home.join(".taida/config.toml"),
            format!("[security]\nmin_release_age = \"{bad}\"\n"),
        )
        .unwrap();
        let _ = fs::remove_dir_all(consumer.join(".taida"));
        let out = taida(&consumer, &home, &["ingot", "install"]);
        let err = stderr_of(&out);
        assert!(!out.status.success(), "{err}");
        assert!(
            err.contains("min_release_age") && err.contains("invalid release age duration"),
            "{bad}: {err}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}

// ── ingot publish: public API diff ──────────────────────────────────

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {}", stderr_of(&out));
}

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

#[test]
fn api_diff_sees_the_package_facade_of_a_source_package() {
    if !git_available() {
        return;
    }
    use taida::pkg::api_diff::{ApiDiff, detect, snapshot_at_tag, snapshot_head};
    let dir = unique_temp_dir("f65_api_diff_source");
    git(&dir, &["init", "-q"]);
    fs::write(
        dir.join("packages.tdm"),
        ">>> ./main.td\n<<<@a.1 me/p1 @(hello, greet)\n",
    )
    .unwrap();
    fs::write(
        dir.join("main.td"),
        "hello name: Str = `hello, ${name}` => :Str\ngreet name: Str = `greet, ${name}` => :Str\n<<< @(hello, greet)\n",
    )
    .unwrap();
    git(&dir, &["add", "packages.tdm", "main.td"]);
    git(&dir, &["commit", "-qm", "a.1"]);
    git(&dir, &["tag", "a.1"]);
    fs::write(
        dir.join("packages.tdm"),
        ">>> ./main.td\n<<<@a.2 me/p1 @(hello)\n",
    )
    .unwrap();
    fs::write(
        dir.join("main.td"),
        "hello name: Str = `hello, ${name}` => :Str\n<<< @(hello)\n",
    )
    .unwrap();

    let prev = snapshot_at_tag(&dir, "a.1").unwrap();
    let next = snapshot_head(&dir).unwrap();
    assert_eq!(
        detect(&prev, &next),
        ApiDiff::Breaking {
            removed: vec!["greet".to_string()],
            changed: Vec::new(),
        }
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn api_diff_ignores_files_below_taida_subdirectories() {
    if !git_available() {
        return;
    }
    use taida::pkg::api_diff::{ApiDiff, detect, snapshot_at_tag, snapshot_head};
    let dir = unique_temp_dir("f65_api_diff_internal");
    fs::create_dir_all(dir.join("taida/internal")).unwrap();
    git(&dir, &["init", "-q"]);
    fs::write(
        dir.join("packages.tdm"),
        ">>> ./taida/p2.td\n<<<@a.1 me/p2 @(hello)\n",
    )
    .unwrap();
    fs::write(
        dir.join("taida/p2.td"),
        "hello name: Str = `hello, ${name}` => :Str\n<<< @(hello)\n",
    )
    .unwrap();
    fs::write(
        dir.join("taida/internal/helper.td"),
        "helper x: Int = x + 1 => :Int\n<<< @(helper)\n",
    )
    .unwrap();
    git(&dir, &["add", "packages.tdm", "taida"]);
    git(&dir, &["commit", "-qm", "a.1"]);
    git(&dir, &["tag", "a.1"]);
    fs::write(
        dir.join("taida/internal/helper.td"),
        "helper x: Int = x + 2 => :Int\n<<< @(helper)\n",
    )
    .unwrap();

    let prev = snapshot_at_tag(&dir, "a.1").unwrap();
    let next = snapshot_head(&dir).unwrap();
    assert_eq!(
        prev.exports.iter().cloned().collect::<Vec<_>>(),
        vec!["hello".to_string()]
    );
    assert_eq!(detect(&prev, &next), ApiDiff::None);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn api_diff_of_an_addon_package_reads_its_facade_and_functions() {
    use taida::pkg::api_diff::snapshot_head;
    let dir = unique_temp_dir("f65_api_diff_addon");
    fs::create_dir_all(dir.join("native")).unwrap();
    fs::create_dir_all(dir.join("taida")).unwrap();
    fs::write(dir.join("packages.tdm"), "<<<@a.1\n").unwrap();
    fs::write(
        dir.join("native/addon.toml"),
        "abi = 1\nentry = \"taida_addon_get_v1\"\npackage = \"me/my-addon\"\nlibrary = \"my_addon\"\n\n[functions]\necho = 1\n",
    )
    .unwrap();
    fs::write(
        dir.join("taida/my-addon.td"),
        "echo <= RustAddon[\"echo\"](arity <= 1)\nEcho <= echo\n<<< @(Echo)\n",
    )
    .unwrap();
    // Another top-level file is not part of an addon package's surface.
    fs::write(dir.join("taida/scratch.td"), "tmp <= 1\n<<< @(tmp)\n").unwrap();
    let snap = snapshot_head(&dir).unwrap();
    assert_eq!(
        snap.exports.iter().cloned().collect::<Vec<_>>(),
        vec!["Echo".to_string(), "echo".to_string()]
    );
    let _ = fs::remove_dir_all(&dir);
}

// ── install.sh ──────────────────────────────────────────────────────

fn install_sh() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("install.sh")
}

/// A fake `curl` that answers `-o FILE URL` from `$FAKE_FIXTURES/<basename>`
/// and the GitHub "latest release" API from `$FAKE_LATEST` (fails when the
/// file does not exist). Nothing leaves the machine.
fn write_fake_curl(bin: &Path) {
    fs::create_dir_all(bin).unwrap();
    let script = r#"#!/bin/sh
out=""; url=""; prev=""
for a in "$@"; do
  if [ "$prev" = "-o" ]; then out="$a"; fi
  prev="$a"; url="$a"
done
case "$url" in
  *releases/latest) [ -f "$FAKE_LATEST" ] || { echo "curl: (22) 403" >&2; exit 22; }; cat "$FAKE_LATEST"; exit 0 ;;
esac
f="$FAKE_FIXTURES/$(basename "$url")"
[ -f "$f" ] || exit 22
if [ -n "$out" ]; then cp "$f" "$out"; else cat "$f"; fi
"#;
    let path = bin.join("curl");
    fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// Release fixtures for tag `@t.1` on this host (archive, SHA256SUMS, and
/// cosign bundles), or `None` on a host `install.sh` does not support.
fn write_release_fixtures(fixtures: &Path) -> Option<()> {
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => return None,
    };
    let stem = format!("taida-@t.1-{triple}");
    let build = fixtures.join("build").join(&stem);
    fs::create_dir_all(&build).unwrap();
    fs::write(build.join("taida"), "#!/bin/sh\necho fake taida\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(build.join("taida"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let asset = format!("{stem}.tar.gz");
    let status = Command::new("tar")
        .arg("-czf")
        .arg(fixtures.join(&asset))
        .arg("-C")
        .arg(fixtures.join("build"))
        .arg(&stem)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let sum = sha256_hex(&fs::read(fixtures.join(&asset)).unwrap());
    fs::write(fixtures.join("SHA256SUMS"), format!("{sum}  {asset}\n")).unwrap();
    fs::write(fixtures.join(format!("{asset}.cosign.bundle")), "bundle\n").unwrap();
    fs::write(fixtures.join("SHA256SUMS.cosign.bundle"), "bundle\n").unwrap();
    Some(())
}

fn run_install_sh(
    cwd: &Path,
    home: &Path,
    bin: &Path,
    fixtures: &Path,
    env: &[(&str, &str)],
    args: &[&str],
) -> Output {
    let mut cmd = Command::new("bash");
    cmd.arg(install_sh())
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("FAKE_FIXTURES", fixtures)
        .env("FAKE_LATEST", fixtures.join("latest.json"))
        .env_remove("TAIDA_VERIFY_SIGNATURES")
        .env_remove("TAIDA_INSTALL_PREFIX")
        .env_remove("TAIDA_VERSION");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("run install.sh")
}

#[cfg(unix)]
#[test]
fn install_sh_rejects_unknown_verify_signature_values() {
    let dir = unique_temp_dir("f65_install_sh_verify");
    let (bin, fixtures, home) = (dir.join("bin"), dir.join("fixtures"), dir.join("home"));
    fs::create_dir_all(&home).unwrap();
    write_fake_curl(&bin);
    if write_release_fixtures(&fixtures).is_none() {
        return;
    }
    let prefix = dir.join("prefix");
    let prefix_arg = prefix.to_string_lossy().into_owned();
    for value in ["enforced", "yes please", "bestEffort"] {
        let out = run_install_sh(
            &dir,
            &home,
            &bin,
            &fixtures,
            &[("TAIDA_VERIFY_SIGNATURES", value)],
            &["--version", "@t.1", "--prefix", &prefix_arg],
        );
        assert_eq!(out.status.code(), Some(1), "{value}: {}", stderr_of(&out));
        assert!(
            stderr_of(&out).contains("unsupported TAIDA_VERIFY_SIGNATURES"),
            "{}",
            stderr_of(&out)
        );
        assert!(!prefix.join("bin/taida").exists());
    }
    // Values that read as "verify" select `required`: without cosign on
    // PATH the install stops (exit 4) instead of downgrading.
    let cosign_on_path = Command::new("sh")
        .args(["-c", "PATH=/usr/bin:/bin command -v cosign"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !cosign_on_path {
        for value in ["true", "Required", "1"] {
            let out = run_install_sh(
                &dir,
                &home,
                &bin,
                &fixtures,
                &[("TAIDA_VERIFY_SIGNATURES", value)],
                &["--version", "@t.1", "--prefix", &prefix_arg],
            );
            assert_eq!(out.status.code(), Some(4), "{value}: {}", stderr_of(&out));
            assert!(!prefix.join("bin/taida").exists());
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn install_sh_resolves_a_relative_prefix_against_the_caller() {
    let dir = unique_temp_dir("f65_install_sh_prefix");
    let (bin, fixtures, home) = (dir.join("bin"), dir.join("fixtures"), dir.join("home"));
    let user = dir.join("user");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&user).unwrap();
    write_fake_curl(&bin);
    if write_release_fixtures(&fixtures).is_none() {
        return;
    }
    let out = run_install_sh(
        &user,
        &home,
        &bin,
        &fixtures,
        &[("TAIDA_VERIFY_SIGNATURES", "off")],
        &["--version", "@t.1", "--prefix", "./tools"],
    );
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(user.join("tools/bin/taida").exists(), "{}", stdout_of(&out));
    assert!(stdout_of(&out).contains(&user.join("tools/bin").display().to_string()));
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn install_sh_reports_an_unresolvable_latest_tag_as_a_network_failure() {
    let dir = unique_temp_dir("f65_install_sh_latest");
    let (bin, fixtures, home) = (dir.join("bin"), dir.join("fixtures"), dir.join("home"));
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&fixtures).unwrap();
    write_fake_curl(&bin);
    // A: the API request fails (e.g. rate limit).
    let out = run_install_sh(
        &dir,
        &home,
        &bin,
        &fixtures,
        &[("TAIDA_VERIFY_SIGNATURES", "off")],
        &[],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr_of(&out));
    assert!(
        stderr_of(&out).contains("cannot resolve latest release tag"),
        "{}",
        stderr_of(&out)
    );
    // B: the API answers without `tag_name`.
    fs::write(
        fixtures.join("latest.json"),
        "{\"message\": \"nothing here\"}\n",
    )
    .unwrap();
    let out = run_install_sh(
        &dir,
        &home,
        &bin,
        &fixtures,
        &[("TAIDA_VERIFY_SIGNATURES", "off")],
        &[],
    );
    assert_eq!(out.status.code(), Some(2), "{}", stderr_of(&out));
    assert!(
        stderr_of(&out).contains("cannot resolve latest release tag"),
        "{}",
        stderr_of(&out)
    );
    let _ = fs::remove_dir_all(&dir);
}

// ── Store packages with an addon prebuild (loopback mock) ───────────

/// Loopback stand-in for GitHub: the package archive, its tag ref and tag
/// list (see `mock::MockServer`), plus the release metadata endpoint that
/// addon prebuild installs read. Answers every request with a fresh copy
/// of `state` so tests can change it between installs.
struct ReleaseMock {
    addr: std::net::SocketAddr,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl ReleaseMock {
    fn start(state: Arc<Mutex<mock::TagState>>) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_flag.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/");
                let path = path.split('?').next().unwrap_or(path).to_string();
                let s = state.lock().unwrap().clone();
                let (status, body): (&str, Vec<u8>) = if path
                    == format!(
                        "/{}/{}/archive/refs/tags/{}.tar.gz",
                        s.org, s.name, s.version
                    ) {
                    ("200 OK", s.tarball.clone())
                } else if path == format!("/repos/{}/{}/git/refs/tags/{}", s.org, s.name, s.version)
                {
                    (
                            "200 OK",
                            format!(
                                "{{\"ref\":\"refs/tags/{}\",\"object\":{{\"sha\":\"{}\",\"type\":\"commit\"}}}}",
                                s.version, s.commit_sha
                            )
                            .into_bytes(),
                        )
                } else if path.contains("/releases/tags/") {
                    (
                            "200 OK",
                            b"{\"published_at\":\"2020-01-01T00:00:00Z\",\"author\":{\"login\":\"taida-lang\"}}".to_vec(),
                        )
                } else if path == format!("/repos/{}/{}/tags", s.org, s.name) {
                    (
                        "200 OK",
                        format!("[{{\"name\":\"{}\"}}]", s.version).into_bytes(),
                    )
                } else {
                    ("404 Not Found", b"not found".to_vec())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        ReleaseMock { addr, stop }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for ReleaseMock {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = std::net::TcpStream::connect(self.addr);
    }
}

/// The workspace sample addon's cdylib, when it has been built.
fn sample_cdylib() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let name = if cfg!(target_os = "macos") {
        "libtaida_addon_sample.dylib"
    } else {
        "libtaida_addon_sample.so"
    };
    ["release", "debug"]
        .iter()
        .map(|profile| root.join("target").join(profile).join(name))
        .find(|p| p.exists())
}

struct StoreAddonFixture {
    dir: PathBuf,
    app: PathBuf,
    state: Arc<Mutex<mock::TagState>>,
    so_sha: String,
}

/// App depending on store package `taida-lang/x@a.1`, an addon whose
/// prebuild is the sample cdylib served from the app via `file://`.
fn store_addon_fixture(prefix: &str) -> Option<StoreAddonFixture> {
    let so = sample_cdylib()?;
    let host = taida::addon::host_target::detect_host_target().ok()?;
    let dir = unique_temp_dir(prefix);
    let app = dir.join("app");
    fs::create_dir_all(app.join("prebuild")).unwrap();
    let so_name = format!("libtaida_addon_sample.{}", host.cdylib_ext());
    fs::copy(&so, app.join("prebuild").join(&so_name)).unwrap();
    let so_sha = sha256_hex(&fs::read(&so).unwrap());
    let addon_toml = format!(
        "abi = 1\nentry = \"taida_addon_get_v1\"\npackage = \"taida-lang/x\"\nlibrary = \"taida_addon_sample\"\n\n[functions]\nnoop = 0\necho = 1\n\n[library.prebuild]\nurl = \"file://prebuild/lib{{name}}.{{ext}}\"\n\n[library.prebuild.targets]\n\"{}\" = \"sha256:{so_sha}\"\n",
        host.as_triple()
    );
    let tarball = mock::make_tarball(&[
        ("packages.tdm", b"<<<@a.1 taida-lang/x\n" as &[u8]),
        ("native/addon.toml", addon_toml.as_bytes()),
    ]);
    let integrity = format!("sha256:{}", sha256_hex(&tarball));
    fs::write(
        app.join("packages.tdm"),
        format!("[packages.\"taida-lang/x\"]\nversion = \"a.1\"\nintegrity = \"{integrity}\"\n\n<<<@a.1 test/app\n"),
    )
    .unwrap();
    fs::write(
        app.join("main.td"),
        ">>> taida-lang/x => @(echo)\nstdout(echo(\"store addon\"))\n",
    )
    .unwrap();
    let state = Arc::new(Mutex::new(mock::TagState {
        org: "taida-lang".into(),
        name: "x".into(),
        version: "a.1".into(),
        commit_sha: "7777777777777777777777777777777777777777".into(),
        tarball,
    }));
    Some(StoreAddonFixture {
        dir,
        app,
        state,
        so_sha,
    })
}

fn store_install(app: &Path, home: &Path, base: &str, extra: &[&str]) -> Output {
    Command::new(taida_bin())
        .arg("ingot")
        .arg("install")
        .args(extra)
        .current_dir(app)
        .env("HOME", home)
        .env("TAIDA_GITHUB_BASE_URL", base)
        .env("TAIDA_GITHUB_API_URL", base)
        .env("TAIDA_E32_ALLOW_MOCK_GITHUB_BASE_URL", "1")
        .env_remove("TAIDA_VERIFY_SIGNATURES")
        .env_remove("TAIDA_MIN_RELEASE_AGE")
        .output()
        .expect("run taida ingot install")
}

fn lock_integrity(app: &Path) -> String {
    let lock = fs::read_to_string(app.join(".taida/taida.lock")).unwrap();
    lock.lines()
        .find(|l| l.starts_with("integrity = "))
        .unwrap_or_default()
        .to_string()
}

fn files_named(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if path.to_string_lossy().ends_with(suffix) {
                out.push(path);
            }
        }
    }
    out
}

#[test]
fn addon_prebuild_stays_in_the_project_and_the_lock_is_stable() {
    if !common::taida_mock_github_base_url_supported() {
        return;
    }
    let Some(fx) = store_addon_fixture("f65_store_prebuild") else {
        return;
    };
    let server = ReleaseMock::start(fx.state.clone());
    let home = fx.dir.join("home");

    // 1. Fresh store, 2. refresh (the first install records no commit sha).
    let out = store_install(&fx.app, &home, &server.url(), &[]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let first = lock_integrity(&fx.app);
    let out = store_install(&fx.app, &home, &server.url(), &[]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_eq!(lock_integrity(&fx.app), first);

    // The binary lives in this project only; the shared store is unchanged.
    assert!(
        files_named(&home.join(".taida/store"), ".so").is_empty()
            && files_named(&home.join(".taida/store"), ".dylib").is_empty(),
        "no prebuild inside the store"
    );
    let placed = files_named(&fx.app.join(".taida/deps"), "taida_addon_sample.so")
        .into_iter()
        .chain(files_named(
            &fx.app.join(".taida/deps"),
            "taida_addon_sample.dylib",
        ))
        .next()
        .expect("prebuild placed under .taida/deps");
    assert!(
        fs::symlink_metadata(&placed).unwrap().is_file(),
        "a real file, not a link"
    );
    assert_eq!(sha256_hex(&fs::read(&placed).unwrap()), fx.so_sha);

    // 3. --frozen on the same machine, 4. --frozen with an empty store (CI).
    let out = store_install(&fx.app, &home, &server.url(), &["--frozen"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let out = store_install(
        &fx.app,
        &fx.dir.join("home-ci"),
        &server.url(),
        &["--frozen"],
    );
    assert!(out.status.success(), "{}", stderr_of(&out));

    // The installed addon still loads.
    let out = Command::new(taida_bin())
        .arg("main.td")
        .current_dir(&fx.app)
        .env("HOME", &home)
        .output()
        .unwrap();
    assert_eq!(stdout_of(&out).trim(), "store addon", "{}", stderr_of(&out));
    let _ = fs::remove_dir_all(&fx.dir);
}

#[cfg(unix)]
#[test]
fn reinstall_replaces_the_prebuild_instead_of_rewriting_it() {
    if !common::taida_mock_github_base_url_supported() {
        return;
    }
    let Some(fx) = store_addon_fixture("f65_store_replace") else {
        return;
    };
    let server = ReleaseMock::start(fx.state.clone());
    let home = fx.dir.join("home");
    let out = store_install(&fx.app, &home, &server.url(), &[]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let placed = files_named(&fx.app.join(".taida/deps"), "taida_addon_sample.so")
        .into_iter()
        .next()
        .expect("prebuild placed");
    // Hold the file open like a running program that loaded it.
    use std::os::unix::fs::MetadataExt;
    let mut held = fs::File::open(&placed).unwrap();
    let out = store_install(&fx.app, &home, &server.url(), &[]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_ne!(
        held.metadata().unwrap().ino(),
        fs::metadata(&placed).unwrap().ino(),
        "a new file replaces the old one; the old one is not rewritten in place"
    );
    let mut bytes = Vec::new();
    held.read_to_end(&mut bytes).unwrap();
    assert_eq!(
        sha256_hex(&bytes),
        fx.so_sha,
        "the file a running program has open keeps its contents"
    );
    let _ = fs::remove_dir_all(&fx.dir);
}

#[test]
fn lock_pin_mismatch_explains_how_to_resolve_it() {
    if !common::taida_mock_github_base_url_supported() {
        return;
    }
    let Some(fx) = store_addon_fixture("f65_store_pin") else {
        return;
    };
    let server = ReleaseMock::start(fx.state.clone());
    let home = fx.dir.join("home");
    let out = store_install(&fx.app, &home, &server.url(), &[]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let lock_path = fx.app.join(".taida/taida.lock");
    let lock = fs::read_to_string(&lock_path).unwrap();
    let tampered_pin = format!("00{}", &fx.so_sha[2..]);
    fs::write(&lock_path, lock.replace(&fx.so_sha, &tampered_pin)).unwrap();

    for extra in [&[][..], &["--force-refresh"][..]] {
        let out = store_install(&fx.app, &home, &server.url(), extra);
        let err = stderr_of(&out);
        assert!(!out.status.success());
        assert!(
            err.contains(&format!("pins {tampered_pin}"))
                && err.contains(&format!("release declares {}", fx.so_sha))
                && err.contains("[[package.addon]]"),
            "{err}"
        );
        assert!(
            !err.contains("Re-run `taida ingot install --force-refresh`"),
            "{err}"
        );
    }
    let _ = fs::remove_dir_all(&fx.dir);
}

#[test]
fn interrupted_refresh_is_recovered_on_the_next_install() {
    if !common::taida_mock_github_base_url_supported() {
        return;
    }
    let dir = unique_temp_dir("f65_store_interrupted");
    let home = dir.join("home");
    let app = dir.join("app");
    fs::create_dir_all(&app).unwrap();
    let tarball = mock::make_tarball(&[
        ("packages.tdm", b"<<<@a.1 taida-lang/y\n" as &[u8]),
        ("main.td", b"hi <= 1\n<<< @(hi)\n"),
    ]);
    let integrity = format!("sha256:{}", sha256_hex(&tarball));
    fs::write(
        app.join("packages.tdm"),
        format!("[packages.\"taida-lang/y\"]\nversion = \"a.1\"\nintegrity = \"{integrity}\"\n\n<<<@a.1 test/app\n"),
    )
    .unwrap();
    fs::write(
        app.join("main.td"),
        ">>> taida-lang/y => @(hi)\nstdout(hi)\n",
    )
    .unwrap();
    let state = Arc::new(Mutex::new(mock::TagState {
        org: "taida-lang".into(),
        name: "y".into(),
        version: "a.1".into(),
        commit_sha: "1111111111111111111111111111111111111111".into(),
        tarball,
    }));
    {
        let server = ReleaseMock::start(state.clone());
        let out = store_install(&app, &home, &server.url(), &[]);
        assert!(out.status.success(), "{}", stderr_of(&out));
    }

    // What an install killed mid-refresh by an earlier version left: the
    // working copy moved aside and an empty entry in its place.
    let store = home.join(".taida/store/taida-lang/y");
    fs::rename(store.join("a.1"), store.join("a.1.refresh-staging-1-2")).unwrap();
    fs::create_dir_all(store.join("a.1")).unwrap();
    fs::create_dir_all(store.join(".tmp-a.1-extract-3-4")).unwrap();

    // The next install repairs it even while offline.
    let out = store_install(&app, &home, "http://127.0.0.1:9", &[]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(
        stderr_of(&out).contains("restored the previous install"),
        "{}",
        stderr_of(&out)
    );
    assert!(store.join("a.1/.taida_installed").exists());
    let leftovers: Vec<_> = fs::read_dir(&store)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("refresh-staging") || n.starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    let out = Command::new(taida_bin())
        .arg("main.td")
        .current_dir(&app)
        .env("HOME", &home)
        .output()
        .unwrap();
    assert_eq!(stdout_of(&out).trim(), "1", "{}", stderr_of(&out));
    let _ = fs::remove_dir_all(&dir);
}
