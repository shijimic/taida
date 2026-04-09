//! Project initialisation logic for `taida init`.
//!
//! RC2.6-3a: extracted from `src/main.rs::run_init` so the CLI layer
//! remains a thin wrapper and the core logic is testable / reusable.
//!
//! The module exposes a single entry point — [`init_project`] — that
//! writes a complete project skeleton to `dir`. The skeleton varies
//! by [`InitTarget`]:
//!
//! * **SourceOnly** — the pre-RC2.6 behaviour: `packages.tdm`,
//!   `main.td`, `.taida/`, `.gitignore`.
//! * **RustAddon** — everything needed to build and publish a Rust
//!   addon package via `taida publish --target rust-addon`:
//!   `packages.tdm`, `Cargo.toml`, `src/lib.rs`, `native/addon.toml`,
//!   `taida/<name>.td`, `.gitignore`, `README.md`.

use std::fs;
use std::path::Path;

use super::manifest::Manifest;

// ── InitTarget ──────────────────────────────────────────────────

/// The kind of project skeleton to generate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitTarget {
    /// Traditional Taida source-only project (pre-RC2.6 default).
    SourceOnly,
    /// Rust addon project with cdylib + facade + addon manifest.
    RustAddon,
}

// ── Public entry point ──────────────────────────────────────────

/// Write a complete project skeleton into `dir`.
///
/// `name` is the human/package name (derived from the directory name
/// by the CLI layer). `target` selects which template set to emit.
///
/// Returns `Ok(files)` with a list of relative paths that were
/// created, or `Err(msg)` on the first I/O failure.
///
/// # Errors
///
/// * `packages.tdm` already exists in `dir` (conflict guard).
/// * Any filesystem write fails.
pub fn init_project(
    dir: &Path,
    name: &str,
    target: InitTarget,
) -> Result<Vec<String>, String> {
    // ── Name validation ─────────────────────────────────
    validate_project_name(name)?;

    // ── Conflict check (both flows) ─────────────────────
    let manifest_path = dir.join("packages.tdm");
    if manifest_path.exists() {
        return Err(format!(
            "packages.tdm already exists in '{}'",
            dir.display()
        ));
    }

    match target {
        InitTarget::SourceOnly => init_source_only(dir, name),
        InitTarget::RustAddon => init_rust_addon(dir, name),
    }
}

// ── SourceOnly flow ─────────────────────────────────────────────

fn init_source_only(dir: &Path, name: &str) -> Result<Vec<String>, String> {
    let mut created: Vec<String> = Vec::new();

    // packages.tdm
    let manifest_content = Manifest::default_template(name);
    write_file(dir, "packages.tdm", &manifest_content)?;
    created.push("packages.tdm".to_string());

    // main.td (only if it does not already exist)
    let main_path = dir.join("main.td");
    if !main_path.exists() {
        let main_content = Manifest::default_main();
        write_file(dir, "main.td", main_content)?;
        created.push("main.td".to_string());
    }

    // .taida/ directory (warning on failure, not fatal)
    let taida_dir = dir.join(".taida");
    if let Err(e) = fs::create_dir_all(&taida_dir) {
        eprintln!(
            "Warning: could not create .taida directory '{}': {}",
            taida_dir.display(),
            e
        );
    }

    // .gitignore (only if it does not already exist)
    let gitignore_path = dir.join(".gitignore");
    if !gitignore_path.exists() {
        write_file(dir, ".gitignore", GITIGNORE_SOURCE_ONLY)?;
        created.push(".gitignore".to_string());
    }

    Ok(created)
}

// ── RustAddon flow (RC2.6-3b, 3d) ──────────────────────────────

fn init_rust_addon(dir: &Path, name: &str) -> Result<Vec<String>, String> {
    let mut created: Vec<String> = Vec::new();

    // Sanitise the name for use as a Rust crate identifier:
    // replace hyphens with underscores.
    let crate_name = name.replace('-', "_");

    // ── packages.tdm ────────────────────────────────────
    let packages_tdm = format!(
        "// packages.tdm -- {name}\n\
         <<<@a\n"
    );
    write_file(dir, "packages.tdm", &packages_tdm)?;
    created.push("packages.tdm".to_string());

    // ── Cargo.toml ──────────────────────────────────────
    let cargo_toml = addon_cargo_toml(name);
    write_file(dir, "Cargo.toml", &cargo_toml)?;
    created.push("Cargo.toml".to_string());

    // ── src/lib.rs ──────────────────────────────────────
    fs::create_dir_all(dir.join("src")).map_err(|e| {
        format!("Cannot create src/ directory: {}", e)
    })?;
    let lib_rs = addon_lib_rs(&crate_name);
    write_file(dir, "src/lib.rs", &lib_rs)?;
    created.push("src/lib.rs".to_string());

    // ── native/addon.toml ───────────────────────────────
    fs::create_dir_all(dir.join("native")).map_err(|e| {
        format!("Cannot create native/ directory: {}", e)
    })?;
    let addon_toml = addon_manifest(&crate_name);
    write_file(dir, "native/addon.toml", &addon_toml)?;
    created.push("native/addon.toml".to_string());

    // ── taida/<name>.td ─────────────────────────────────
    fs::create_dir_all(dir.join("taida")).map_err(|e| {
        format!("Cannot create taida/ directory: {}", e)
    })?;
    let facade = addon_facade(name);
    let facade_path = format!("taida/{name}.td");
    write_file(dir, &facade_path, &facade)?;
    created.push(facade_path);

    // ── .gitignore ──────────────────────────────────────
    // RC2.6-3d: addon projects additionally ignore target/ (cargo
    // build output). Cargo.lock is NOT ignored (binary crate
    // convention: commit the lockfile for reproducibility).
    write_file(dir, ".gitignore", GITIGNORE_RUST_ADDON)?;
    created.push(".gitignore".to_string());

    // ── .taida/ directory (same as source-only) ─────────
    let taida_dir = dir.join(".taida");
    if let Err(e) = fs::create_dir_all(&taida_dir) {
        eprintln!(
            "Warning: could not create .taida directory '{}': {}",
            taida_dir.display(),
            e
        );
    }

    // ── README.md ───────────────────────────────────────
    let readme = addon_readme(name);
    write_file(dir, "README.md", &readme)?;
    created.push("README.md".to_string());

    // RC2.6-3d: addon projects do NOT create main.td (the
    // facade in taida/<name>.td replaces it).

    Ok(created)
}

// ── Template generators (RC2.6-3b) ─────────────────────────────

/// `.gitignore` for source-only projects (pre-RC2.6 content).
const GITIGNORE_SOURCE_ONLY: &str = "\
# Taida build artifacts (regeneratable)
.taida/deps/
.taida/build/
.taida/graph/
# .taida/taida.lock is tracked (not inside ignored dirs)
";

/// `.gitignore` for Rust addon projects.
///
/// Includes `target/` for cargo build output. `Cargo.lock` is NOT
/// ignored: it should be committed for reproducible addon builds.
const GITIGNORE_RUST_ADDON: &str = "\
# Rust build output
target/

# Taida build artifacts (regeneratable)
.taida/deps/
.taida/build/
.taida/graph/
";

fn addon_cargo_toml(name: &str) -> String {
    // Crate name uses underscores (Rust convention).
    let crate_name = name.replace('-', "_");
    format!(
        r#"[package]
name = "{crate_name}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["rlib", "cdylib"]

[dependencies]
taida-addon = "2.0"

# taida-addon is not yet published on crates.io. For local development,
# uncomment the [patch.crates-io] section below and adjust the path to
# point to your local taida checkout.
#
# [patch.crates-io]
# taida-addon = {{ path = "../taida/crates/addon-rs" }}
"#
    )
}

fn addon_lib_rs(crate_name: &str) -> String {
    format!(
        r#"//! `{crate_name}` — Taida Rust addon.
//!
//! Created with `taida init --target rust-addon`.
//! See https://github.com/taida-lang/taida for documentation.

use core::ffi::c_char;
use taida_addon::{{
    TaidaAddonErrorV1, TaidaAddonFunctionV1, TaidaAddonStatus,
    TaidaAddonValueV1, TaidaHostV1,
}};
use taida_addon::bridge::HostValueBuilder;
use core::sync::atomic::{{AtomicPtr, Ordering}};

/// Host callback table, populated by `addon_init`.
static HOST_PTR: AtomicPtr<TaidaHostV1> = AtomicPtr::new(core::ptr::null_mut());

extern "C" fn addon_init(host: *const TaidaHostV1) -> TaidaAddonStatus {{
    if host.is_null() {{
        return TaidaAddonStatus::NullPointer;
    }}
    HOST_PTR.store(host as *mut _, Ordering::Release);
    TaidaAddonStatus::Ok
}}

// ── echo ────────────────────────────────────────────────────────
//
// Identity function: returns its single argument unchanged.
// Replace or extend this with your addon's real functions.

extern "C" fn echo(
    args_ptr: *const TaidaAddonValueV1,
    args_len: u32,
    out_value: *mut *mut TaidaAddonValueV1,
    _out_error: *mut *mut TaidaAddonErrorV1,
) -> TaidaAddonStatus {{
    if args_len != 1 {{
        return TaidaAddonStatus::ArityMismatch;
    }}
    // Return the argument as-is (identity).
    if !out_value.is_null() && !args_ptr.is_null() {{
        unsafe {{ *out_value = args_ptr as *mut TaidaAddonValueV1 }};
    }}
    TaidaAddonStatus::Ok
}}

/// Function table for this addon.
static FUNCTIONS: &[TaidaAddonFunctionV1] = &[
    TaidaAddonFunctionV1 {{
        name: c"echo".as_ptr() as *const c_char,
        arity: 1,
        call: echo,
    }},
];

taida_addon::declare_addon! {{
    name: "{crate_name}",
    functions: FUNCTIONS,
    init: addon_init,
}}
"#
    )
}

fn addon_manifest(crate_name: &str) -> String {
    // The URL template uses only the four allowed variables:
    // {version}, {target}, {ext}, {name}. The GitHub org/repo prefix
    // is a placeholder ("OWNER/<crate_name>") that the user must set
    // manually before publishing.
    format!(
        r#"# native/addon.toml — ABI v1 manifest
#
# Generated by `taida init --target rust-addon`.
# See https://github.com/taida-lang/taida for schema documentation.
#
# URL template for prebuild binary downloads.
# Before publishing, replace OWNER and NAME with your GitHub org/repo.
# Example: https://github.com/my-org/my-addon/releases/download/{{version}}/lib{{name}}-{{target}}.{{ext}}

abi = 1
entry = "taida_addon_get_v1"
package = "OWNER/{crate_name}"
library = "{crate_name}"

[functions]
echo = 1

[library.prebuild]
url = "https://github.com/OWNER/{crate_name}/releases/download/{{version}}/lib{{name}}-{{target}}.{{ext}}"

[library.prebuild.targets]
"#
    )
}

fn addon_facade(name: &str) -> String {
    format!(
        r#"// {name}.td — Taida facade for the {name} addon.
//
// Generated by `taida init --target rust-addon`.
// Export the addon's functions so downstream packages can import them.

Echo <= echo

<<< @(Echo)
"#
    )
}

fn addon_readme(name: &str) -> String {
    format!(
        r#"# {name}

Taida Rust addon package.

Created with `taida init --target rust-addon`.

## Usage

```taida
>>> {name}@a.1 => @(Echo)

result <= Echo("hello")
stdout(result)
```

## Development

```bash
# Build the addon cdylib
cargo build --release --lib

# Publish (requires `gh` CLI for GitHub Release)
taida publish --target rust-addon
```

> **Note:** `taida-addon` is not yet published on crates.io. Before building,
> uncomment the `[patch.crates-io]` section in `Cargo.toml` and point it to
> your local taida checkout.
"#
    )
}

// ── Name validation ─────────────────────────────────────────────

/// Validate a project name for `taida init`.
///
/// Rules (same as the bare-name component in `publish::validate_package_name`):
/// - Must not be empty.
/// - ASCII lowercase, digits, and hyphens only (`[a-z0-9-]+`).
/// - Must not start or end with a hyphen.
/// - Must not contain `..` (path traversal guard).
fn validate_project_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Project name must not be empty.".to_string());
    }
    if name.contains("..") {
        return Err(format!(
            "Invalid project name '{}'. Name must not contain '..'.",
            name
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(format!(
            "Invalid project name '{}'. Name must not start or end with '-'.",
            name
        ));
    }
    if !name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(format!(
            "Invalid project name '{}'. Name must contain only lowercase letters, digits, and hyphens.",
            name
        ));
    }
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────

fn write_file(dir: &Path, rel_path: &str, content: &str) -> Result<(), String> {
    let abs = dir.join(rel_path);
    fs::write(&abs, content).map_err(|e| {
        format!("Error writing '{}': {}", abs.display(), e)
    })
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(suffix: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "taida_init_{}_{}_{}",
            suffix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn test_source_only_creates_expected_files() {
        let dir = temp_dir("src_only");
        let files = init_project(&dir, "demo", InitTarget::SourceOnly).unwrap();
        assert!(dir.join("packages.tdm").exists());
        assert!(dir.join("main.td").exists());
        assert!(dir.join(".gitignore").exists());
        assert!(files.contains(&"packages.tdm".to_string()));
        assert!(files.contains(&"main.td".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_source_only_conflict_guard() {
        let dir = temp_dir("src_conflict");
        std::fs::write(dir.join("packages.tdm"), "existing").unwrap();
        let err = init_project(&dir, "demo", InitTarget::SourceOnly).unwrap_err();
        assert!(err.contains("already exists"), "expected conflict: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_creates_expected_files() {
        let dir = temp_dir("addon");
        let files = init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        assert!(dir.join("packages.tdm").exists());
        assert!(dir.join("Cargo.toml").exists());
        assert!(dir.join("src/lib.rs").exists());
        assert!(dir.join("native/addon.toml").exists());
        assert!(dir.join("taida/my-addon.td").exists());
        assert!(dir.join(".gitignore").exists());
        assert!(dir.join("README.md").exists());
        // main.td must NOT exist for addon projects
        assert!(!dir.join("main.td").exists());

        assert!(files.contains(&"packages.tdm".to_string()));
        assert!(files.contains(&"Cargo.toml".to_string()));
        assert!(files.contains(&"src/lib.rs".to_string()));
        assert!(files.contains(&"native/addon.toml".to_string()));
        assert!(files.contains(&"taida/my-addon.td".to_string()));
        assert!(files.contains(&".gitignore".to_string()));
        assert!(files.contains(&"README.md".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_packages_tdm_uses_taida_version_format() {
        let dir = temp_dir("addon_ver");
        init_project(&dir, "test-pkg", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join("packages.tdm")).unwrap();
        assert!(
            content.contains("<<<@a"),
            "packages.tdm must use Taida version format: {content}"
        );
        // Must NOT contain semver
        assert!(
            !content.contains("0.1.0"),
            "packages.tdm must not contain semver: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_cargo_toml_has_cdylib() {
        let dir = temp_dir("addon_cargo");
        init_project(&dir, "test-pkg", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(content.contains("cdylib"), "Cargo.toml must declare cdylib: {content}");
        assert!(content.contains("rlib"), "Cargo.toml must declare rlib: {content}");
        assert!(
            content.contains("taida-addon"),
            "Cargo.toml must depend on taida-addon: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_addon_toml_has_abi_v1() {
        let dir = temp_dir("addon_abi");
        init_project(&dir, "test-pkg", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join("native/addon.toml")).unwrap();
        assert!(content.contains("abi = 1"), "addon.toml must have abi = 1: {content}");
        assert!(
            content.contains("entry = \"taida_addon_get_v1\""),
            "addon.toml must have correct entry: {content}"
        );
        assert!(
            content.contains("[functions]"),
            "addon.toml must have [functions]: {content}"
        );
        assert!(
            content.contains("echo = 1"),
            "addon.toml must declare echo function: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_gitignore_includes_target() {
        let dir = temp_dir("addon_gi");
        init_project(&dir, "test-pkg", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(
            content.contains("target/"),
            ".gitignore must include target/: {content}"
        );
        // Cargo.lock must NOT be in .gitignore
        assert!(
            !content.contains("Cargo.lock"),
            ".gitignore must not ignore Cargo.lock: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_conflict_guard() {
        let dir = temp_dir("addon_conflict");
        std::fs::write(dir.join("packages.tdm"), "existing").unwrap();
        let err = init_project(&dir, "demo", InitTarget::RustAddon).unwrap_err();
        assert!(err.contains("already exists"), "expected conflict: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_addon_toml_parseable_by_manifest_parser() {
        let dir = temp_dir("addon_parse");
        init_project(&dir, "test-pkg", InitTarget::RustAddon).unwrap();
        // The generated addon.toml must be parseable by the real
        // addon manifest parser (RC2.6-3e requirement).
        let result = crate::addon::manifest::parse_addon_manifest(
            &dir.join("native/addon.toml"),
        );
        assert!(
            result.is_ok(),
            "addon.toml must be parseable: {:?}",
            result.err()
        );
        let manifest = result.unwrap();
        assert_eq!(manifest.abi, 1);
        assert_eq!(manifest.entry, "taida_addon_get_v1");
        assert_eq!(manifest.library, "test_pkg");
        assert!(manifest.functions.contains_key("echo"));
        assert_eq!(manifest.functions["echo"], 1);
        assert!(manifest.prebuild.has_prebuild());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_facade_has_export() {
        let dir = temp_dir("addon_facade");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join("taida/my-addon.td")).unwrap();
        assert!(
            content.contains("<<< @(Echo)"),
            "facade must export: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Name validation tests ──────────────────────────────

    #[test]
    fn test_validate_project_name_valid() {
        assert!(validate_project_name("my-addon").is_ok());
        assert!(validate_project_name("foo").is_ok());
        assert!(validate_project_name("a1b2").is_ok());
    }

    #[test]
    fn test_validate_project_name_empty_rejected() {
        assert!(validate_project_name("").is_err());
    }

    #[test]
    fn test_validate_project_name_whitespace_rejected() {
        assert!(validate_project_name("foo bar").is_err());
    }

    #[test]
    fn test_validate_project_name_special_chars_rejected() {
        assert!(validate_project_name("foo\"bar").is_err());
        assert!(validate_project_name("foo/bar").is_err());
    }

    #[test]
    fn test_validate_project_name_dotdot_rejected() {
        assert!(validate_project_name("..").is_err());
        assert!(validate_project_name("a..b").is_err());
    }

    #[test]
    fn test_validate_project_name_leading_trailing_hyphen_rejected() {
        assert!(validate_project_name("-pkg").is_err());
        assert!(validate_project_name("pkg-").is_err());
    }

    #[test]
    fn test_validate_project_name_uppercase_rejected() {
        assert!(validate_project_name("MyAddon").is_err());
    }

    #[test]
    fn test_init_rejects_invalid_name() {
        let dir = temp_dir("bad_name");
        let err = init_project(&dir, "", InitTarget::SourceOnly).unwrap_err();
        assert!(err.contains("must not be empty"), "expected empty rejection: {err}");

        let err = init_project(&dir, "foo bar", InitTarget::RustAddon).unwrap_err();
        assert!(err.contains("lowercase"), "expected char rejection: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cargo_toml_has_patch_hint() {
        let dir = temp_dir("patch_hint");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(
            content.contains("[patch.crates-io]"),
            "Cargo.toml must have patch hint: {content}"
        );
        assert!(
            content.contains("taida-addon is not yet published"),
            "Cargo.toml must explain crates.io status: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_addon_toml_comment_no_auto_rewrite() {
        let dir = temp_dir("toml_comment");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join("native/addon.toml")).unwrap();
        assert!(
            !content.contains("automatically"),
            "addon.toml must not promise auto-rewrite: {content}"
        );
        assert!(
            content.contains("Before publishing, replace OWNER"),
            "addon.toml must instruct manual replacement: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
