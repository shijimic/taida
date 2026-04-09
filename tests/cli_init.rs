//! Integration tests for `taida init` subcommand (RC2.6-3e).
//!
//! These tests invoke the actual `taida` binary and verify scaffold
//! output for both `--target rust-addon` and source-only (default)
//! modes. The tests are filesystem-level: they check file existence,
//! content properties, and — where possible — parse correctness.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    let p = std::env::temp_dir().join(format!(
        "taida_cli_init_{}_{}_{}",
        prefix,
        std::process::id(),
        nanos
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn taida_bin() -> String {
    env!("CARGO_BIN_EXE_taida").to_string()
}

// ── Test 1: addon scaffold creates all expected files ────────────

#[test]
fn test_init_rust_addon_creates_full_tree() {
    let root = unique_temp_dir("addon_tree");
    let project_dir = root.join("foo");

    let output = Command::new(taida_bin())
        .args(["init", "--target", "rust-addon", "foo"])
        .current_dir(&root)
        .output()
        .expect("taida init should succeed");

    assert!(
        output.status.success(),
        "taida init failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify all expected files exist.
    assert!(
        project_dir.join("packages.tdm").exists(),
        "packages.tdm missing"
    );
    assert!(
        project_dir.join("Cargo.toml").exists(),
        "Cargo.toml missing"
    );
    assert!(
        project_dir.join("src/lib.rs").exists(),
        "src/lib.rs missing"
    );
    assert!(
        project_dir.join("native/addon.toml").exists(),
        "native/addon.toml missing"
    );
    assert!(
        project_dir.join("taida/foo.td").exists(),
        "taida/foo.td missing"
    );
    assert!(
        project_dir.join(".gitignore").exists(),
        ".gitignore missing"
    );
    assert!(project_dir.join("README.md").exists(), "README.md missing");
    assert!(
        project_dir.join(".github/workflows/release.yml").exists(),
        ".github/workflows/release.yml missing"
    );

    // main.td must NOT exist for addon projects.
    assert!(
        !project_dir.join("main.td").exists(),
        "main.td must not exist for addon"
    );

    let _ = fs::remove_dir_all(&root);
}

// ── Test 2: Cargo.toml is parseable by `cargo check` ────────────
//
// We only run `cargo check` (not full build) to avoid pulling crate
// dependencies which would be slow.  However, if `cargo` is not
// available, we skip gracefully.

#[test]
fn test_init_rust_addon_cargo_toml_is_valid() {
    // Skip if cargo is not available.
    if Command::new("cargo").arg("--version").output().is_err() {
        eprintln!("cargo not found, skipping Cargo.toml validation");
        return;
    }

    let root = unique_temp_dir("addon_cargo_check");
    let project_dir = root.join("check-pkg");

    let output = Command::new(taida_bin())
        .args(["init", "--target", "rust-addon", "check-pkg"])
        .current_dir(&root)
        .output()
        .expect("taida init should succeed");
    assert!(output.status.success());

    // Run `cargo read-manifest` to verify Cargo.toml is syntactically
    // valid without needing to resolve dependencies.
    let cargo_out = Command::new("cargo")
        .args(["read-manifest", "--manifest-path"])
        .arg(project_dir.join("Cargo.toml"))
        .output()
        .expect("cargo read-manifest should run");

    assert!(
        cargo_out.status.success(),
        "Cargo.toml is not parseable by cargo:\nstderr: {}",
        String::from_utf8_lossy(&cargo_out.stderr)
    );

    let _ = fs::remove_dir_all(&root);
}

// ── Test 3: packages.tdm uses Taida version format ──────────────

#[test]
fn test_init_rust_addon_packages_tdm_taida_version() {
    let root = unique_temp_dir("addon_ver");
    let project_dir = root.join("ver-pkg");

    let output = Command::new(taida_bin())
        .args(["init", "--target", "rust-addon", "ver-pkg"])
        .current_dir(&root)
        .output()
        .expect("taida init should succeed");
    assert!(output.status.success());

    let content = fs::read_to_string(project_dir.join("packages.tdm")).unwrap();
    assert!(
        content.contains("<<<@a"),
        "packages.tdm must use Taida version format (<<<@a): {}",
        content
    );
    // Must NOT contain semver-style versions.
    assert!(
        !content.contains("0.1.0"),
        "packages.tdm must not contain semver: {}",
        content
    );

    let _ = fs::remove_dir_all(&root);
}

// ── Test 4: native/addon.toml is parseable by addon manifest parser ──
//
// We invoke `taida inspect` on the facade to trigger the parser
// chain, but that is overkill. Instead we directly read the file and
// use a simple heuristic (the file must contain the required keys).

#[test]
fn test_init_rust_addon_addon_toml_structure() {
    let root = unique_temp_dir("addon_toml");
    let project_dir = root.join("toml-pkg");

    let output = Command::new(taida_bin())
        .args(["init", "--target", "rust-addon", "toml-pkg"])
        .current_dir(&root)
        .output()
        .expect("taida init should succeed");
    assert!(output.status.success());

    let content = fs::read_to_string(project_dir.join("native/addon.toml")).unwrap();
    assert!(content.contains("abi = 1"), "addon.toml must have abi = 1");
    assert!(
        content.contains("entry = \"taida_addon_get_v1\""),
        "addon.toml must have correct entry symbol"
    );
    assert!(
        content.contains("[functions]"),
        "addon.toml must have [functions]"
    );
    assert!(content.contains("echo = 1"), "addon.toml must declare echo");
    assert!(
        content.contains("[library.prebuild]"),
        "addon.toml must have [library.prebuild]"
    );
    assert!(
        content.contains("{version}") && content.contains("{target}") && content.contains("{ext}"),
        "addon.toml URL template must use valid variables"
    );

    let _ = fs::remove_dir_all(&root);
}

// ── Test 5: unknown --target value produces clear error ─────────

#[test]
fn test_init_unknown_target_produces_error() {
    let root = unique_temp_dir("unknown_target");

    let output = Command::new(taida_bin())
        .args(["init", "--target", "js-addon", "bad-pkg"])
        .current_dir(&root)
        .output()
        .expect("taida init should run");

    assert!(!output.status.success(), "unknown target should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown init target"),
        "error message should mention unknown target: {}",
        stderr
    );

    let _ = fs::remove_dir_all(&root);
}

// ── Test 6: no name uses current directory ──────────────────────

#[test]
fn test_init_rust_addon_no_name_uses_cwd() {
    // The CWD directory name becomes the project name, so it must
    // pass name validation (lowercase + digits + hyphens only).
    let parent = unique_temp_dir("addon-cwd-parent");
    let root = parent.join("my-addon-cwd");
    fs::create_dir_all(&root).unwrap();

    let output = Command::new(taida_bin())
        .args(["init", "--target", "rust-addon"])
        .current_dir(&root)
        .output()
        .expect("taida init should succeed");

    assert!(
        output.status.success(),
        "taida init in CWD failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Files should be created in the CWD (root).
    assert!(
        root.join("packages.tdm").exists(),
        "packages.tdm missing in CWD"
    );
    assert!(
        root.join("Cargo.toml").exists(),
        "Cargo.toml missing in CWD"
    );
    assert!(
        root.join("src/lib.rs").exists(),
        "src/lib.rs missing in CWD"
    );
    assert!(
        root.join("native/addon.toml").exists(),
        "native/addon.toml missing in CWD"
    );
    assert!(
        root.join(".gitignore").exists(),
        ".gitignore missing in CWD"
    );
    assert!(root.join("README.md").exists(), "README.md missing in CWD");

    let _ = fs::remove_dir_all(&parent);
}

// ── Test 7: CI workflow template is generated for addon ─────────

#[test]
fn test_init_rust_addon_ci_workflow_exists() {
    let root = unique_temp_dir("addon_ci");
    let project_dir = root.join("ci-pkg");

    let output = Command::new(taida_bin())
        .args(["init", "--target", "rust-addon", "ci-pkg"])
        .current_dir(&root)
        .output()
        .expect("taida init should succeed");

    assert!(
        output.status.success(),
        "taida init failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let workflow_path = project_dir.join(".github/workflows/release.yml");
    assert!(
        workflow_path.exists(),
        ".github/workflows/release.yml missing"
    );

    let content = fs::read_to_string(&workflow_path).unwrap();

    // Must be valid UTF-8 and non-empty.
    assert!(!content.is_empty(), "release.yml must not be empty");

    // Placeholders must be resolved (no raw {LIBRARY_STEM} or
    // {PACKAGE_NAME} in the output).
    assert!(
        !content.contains("{LIBRARY_STEM}"),
        "raw {{LIBRARY_STEM}} placeholder must be replaced"
    );
    assert!(
        !content.contains("{PACKAGE_NAME}"),
        "raw {{PACKAGE_NAME}} placeholder must be replaced"
    );

    // The resolved library stem (ci_pkg) must appear.
    assert!(
        content.contains("ci_pkg"),
        "workflow must contain resolved library stem 'ci_pkg'"
    );

    // Must have the Taida tag trigger, not semver.
    assert!(content.contains("'*.*'"), "missing '*.*' tag trigger");

    // Must have the four matrix targets.
    assert!(
        content.contains("x86_64-unknown-linux-gnu"),
        "missing linux target"
    );
    assert!(
        content.contains("aarch64-apple-darwin"),
        "missing macOS ARM target"
    );

    // Must reference addon.lock.toml (lockfile job).
    assert!(
        content.contains("addon.lock.toml"),
        "workflow must reference addon.lock.toml"
    );

    let _ = fs::remove_dir_all(&root);
}

// ── Test 8: source-only does NOT create .github/ ────────────────

#[test]
fn test_init_source_only_no_github_dir() {
    let root = unique_temp_dir("src_no_ci");
    let project_dir = root.join("no-ci-pkg");

    let output = Command::new(taida_bin())
        .args(["init", "no-ci-pkg"])
        .current_dir(&root)
        .output()
        .expect("taida init should succeed");

    assert!(output.status.success());
    assert!(
        !project_dir.join(".github").exists(),
        "source-only projects must NOT have .github/"
    );

    let _ = fs::remove_dir_all(&root);
}

// ── Test 9: source-only init backward compat ────────────────────

#[test]
fn test_init_source_only_backward_compat() {
    let root = unique_temp_dir("src_compat");
    let project_dir = root.join("compat-pkg");

    let output = Command::new(taida_bin())
        .args(["init", "compat-pkg"])
        .current_dir(&root)
        .output()
        .expect("taida init should succeed");

    assert!(
        output.status.success(),
        "source-only init failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Source-only must create packages.tdm + main.td + .gitignore
    assert!(project_dir.join("packages.tdm").exists());
    assert!(project_dir.join("main.td").exists());
    assert!(project_dir.join(".gitignore").exists());
    // Must NOT create Cargo.toml or native/addon.toml
    assert!(!project_dir.join("Cargo.toml").exists());
    assert!(!project_dir.join("native/addon.toml").exists());

    let _ = fs::remove_dir_all(&root);
}
