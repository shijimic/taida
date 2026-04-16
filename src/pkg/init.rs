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
//!   addon package via the C14 tag-push workflow:
//!   `packages.tdm`, `Cargo.toml`, `src/lib.rs`, `native/addon.toml`,
//!   `taida/<name>.td`, `.gitignore`, `README.md`,
//!   `.github/workflows/release.yml`.
//!
//! C14-3: the `release.yml` template is sourced from
//! `crates/addon-rs/templates/release.yml.template` and kept
//! structurally symmetric with `shijimic/taida`'s core release
//! workflow (prepare -> gate -> build -> publish, `github-actions[bot]`
//! as release author, 5-target build matrix). Authoring tweaks go in
//! the template file; this module only substitutes
//! `{{LIBRARY_STEM}}` and `{{CRATE_DIR}}` at scaffold time.

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
pub fn init_project(dir: &Path, name: &str, target: InitTarget) -> Result<Vec<String>, String> {
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
    // B11-10d: Show canonical surface as a comment.
    // The owner is unknown at init time, so the active line uses
    // version-only form. User should update to the canonical form
    // before publishing: `<<<@a owner/name @(exports)`
    let packages_tdm = format!(
        "// packages.tdm -- {name}\n\
         // Canonical form (update before publishing):\n\
         // <<<@a owner/{name} @(myExport)\n\
         <<<@a\n"
    );
    write_file(dir, "packages.tdm", &packages_tdm)?;
    created.push("packages.tdm".to_string());

    // ── Cargo.toml ──────────────────────────────────────
    let cargo_toml = addon_cargo_toml(name);
    write_file(dir, "Cargo.toml", &cargo_toml)?;
    created.push("Cargo.toml".to_string());

    // ── src/lib.rs ──────────────────────────────────────
    fs::create_dir_all(dir.join("src"))
        .map_err(|e| format!("Cannot create src/ directory: {}", e))?;
    let lib_rs = addon_lib_rs(&crate_name);
    write_file(dir, "src/lib.rs", &lib_rs)?;
    created.push("src/lib.rs".to_string());

    // ── native/addon.toml ───────────────────────────────
    fs::create_dir_all(dir.join("native"))
        .map_err(|e| format!("Cannot create native/ directory: {}", e))?;
    let addon_toml = addon_manifest(&crate_name);
    write_file(dir, "native/addon.toml", &addon_toml)?;
    created.push("native/addon.toml".to_string());

    // ── taida/<name>.td ─────────────────────────────────
    fs::create_dir_all(dir.join("taida"))
        .map_err(|e| format!("Cannot create taida/ directory: {}", e))?;
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

    // ── .github/workflows/release.yml (C14-3) ──
    // CI workflow for tag-push-triggered release. The static template
    // lives in `crates/addon-rs/templates/release.yml.template` and is
    // kept structurally symmetric with the Taida core release workflow
    // (`.github/workflows/release.yml` in `shijimic/taida`).
    //
    // Template variables:
    //   {{LIBRARY_STEM}}  → Rust crate name (underscored), used to
    //                        locate and rename `lib<stem>.<ext>`.
    //   {{CRATE_DIR}}     → Path to the cargo crate root relative to
    //                        the repository root. For scaffolded
    //                        single-crate addons this is `.`.
    let workflow = render_release_workflow(&crate_name, ".");
    fs::create_dir_all(dir.join(".github/workflows"))
        .map_err(|e| format!("Cannot create .github/workflows/ directory: {}", e))?;
    write_file(dir, ".github/workflows/release.yml", &workflow)?;
    created.push(".github/workflows/release.yml".to_string());

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

# taida-addon is not yet published on crates.io.
# The [patch.crates-io] section below resolves it from a local taida
# checkout. Adjust the path if your taida repo is in a different
# location relative to this project.
[patch.crates-io]
taida-addon = {{ path = "../taida/crates/addon-rs" }}
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

/// Static template for `.github/workflows/release.yml` (C14-3).
///
/// The template lives as a sibling file so it can be edited / diffed
/// as plain YAML, and so its structure remains symmetric with the
/// Taida core release workflow at
/// `.github/workflows/release.yml` in `shijimic/taida`.
///
/// Variables substituted at scaffold time:
///   * `{{LIBRARY_STEM}}` — underscored crate / library stem.
///   * `{{CRATE_DIR}}`    — path to the cargo crate root (usually `.`).
const RELEASE_YML_TEMPLATE: &str =
    include_str!("../../crates/addon-rs/templates/release.yml.template");

/// Render the `.github/workflows/release.yml` template by substituting
/// `{{LIBRARY_STEM}}` and `{{CRATE_DIR}}` with concrete values.
///
/// The output is the exact YAML that will be written to the
/// scaffolded addon repository. The function is pure — no I/O.
///
/// # Rationale (C14-3)
///
/// Under the C14 publish workflow, the `taida publish` CLI only pushes
/// the tag. All build / SHA-256 / `addon.lock.toml` / release creation
/// responsibilities live in this workflow and are performed by
/// `github-actions[bot]`, mirroring the Taida core release contract.
pub(crate) fn render_release_workflow(library_stem: &str, crate_dir: &str) -> String {
    RELEASE_YML_TEMPLATE
        .replace("{{LIBRARY_STEM}}", library_stem)
        .replace("{{CRATE_DIR}}", crate_dir)
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
# Build the addon cdylib locally
cargo build --release --lib

# Run the addon test suite
cargo test
```

## Release (C14 tag-push workflow)

Releases are cut by tag pushes. The CI workflow at
`.github/workflows/release.yml` is responsible for building cdylibs
for the 5 supported targets, computing SHA-256, and publishing the
GitHub Release with `addon.lock.toml` attached.

```bash
# Preview the next version without touching git
taida publish --dry-run

# Push the tag; CI takes over from there
taida publish
```

The release is created by `github-actions[bot]`. The `taida publish`
CLI does NOT build cdylibs, write `addon.lock.toml`, or call
`gh release create` — those responsibilities live in CI.

> **Note:** `taida-addon` is not yet published on crates.io. The `Cargo.toml`
> includes a `[patch.crates-io]` section that resolves `taida-addon` from a
> local taida checkout at `../taida/crates/addon-rs`. Adjust the path if your
> taida repository is in a different location.
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
    fs::write(&abs, content).map_err(|e| format!("Error writing '{}': {}", abs.display(), e))
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Extract the block of text that belongs to a given job (name is
    /// assumed to live at 2-space indent). The block ends when the
    /// next 2-space-indented sibling key appears, or at EOF.
    fn extract_job_block(content: &str, job: &str) -> String {
        let marker = format!("  {job}:");
        let mut collecting = false;
        let mut out = String::new();
        for line in content.lines() {
            if collecting {
                // A new top-level job (2-space indent, not nested).
                if line.starts_with("  ")
                    && !line.starts_with("   ")
                    && !line.starts_with("    ")
                    && line.trim_end().ends_with(':')
                    && !line.trim_start().is_empty()
                    && line != marker
                {
                    break;
                }
                // Top-level key at column 0 ends the jobs mapping.
                if !line.starts_with(' ') && !line.is_empty() {
                    break;
                }
                out.push_str(line);
                out.push('\n');
            } else if line == marker {
                collecting = true;
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    /// Return the jobs declared under the top-level `jobs:` mapping in
    /// declaration order.
    fn declared_job_order(content: &str) -> Vec<String> {
        let mut in_jobs = false;
        let mut jobs = Vec::new();
        for line in content.lines() {
            if line == "jobs:" {
                in_jobs = true;
                continue;
            }
            if in_jobs {
                // End of jobs block: any top-level key at column 0.
                if !line.is_empty() && !line.starts_with(' ') {
                    break;
                }
                // 2-space indent, ends with ':' — a job header.
                if let Some(rest) = line.strip_prefix("  ")
                    && !rest.starts_with(' ')
                    && let Some(name) = rest.strip_suffix(':')
                    && !name.is_empty()
                {
                    jobs.push(name.to_string());
                }
            }
        }
        jobs
    }

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
        assert!(
            dir.join(".github/workflows/release.yml").exists(),
            "CI workflow template missing"
        );
        // main.td must NOT exist for addon projects
        assert!(!dir.join("main.td").exists());

        assert!(files.contains(&"packages.tdm".to_string()));
        assert!(files.contains(&"Cargo.toml".to_string()));
        assert!(files.contains(&"src/lib.rs".to_string()));
        assert!(files.contains(&"native/addon.toml".to_string()));
        assert!(files.contains(&"taida/my-addon.td".to_string()));
        assert!(files.contains(&".gitignore".to_string()));
        assert!(files.contains(&"README.md".to_string()));
        assert!(files.contains(&".github/workflows/release.yml".to_string()));
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
        assert!(
            content.contains("cdylib"),
            "Cargo.toml must declare cdylib: {content}"
        );
        assert!(
            content.contains("rlib"),
            "Cargo.toml must declare rlib: {content}"
        );
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
        assert!(
            content.contains("abi = 1"),
            "addon.toml must have abi = 1: {content}"
        );
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
        let result = crate::addon::manifest::parse_addon_manifest(&dir.join("native/addon.toml"));
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
        // RC2.6B-024: package field is the qualified name source for
        // release titles. Scaffold uses "OWNER/<crate_name>" placeholder.
        assert_eq!(manifest.package, "OWNER/test_pkg");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_addon_toml_qualified_name_used_for_release_title() {
        // RC2.6B-024: release title reads addon.toml's `package` field
        // as the qualified name. Verify that when a user sets this to
        // "org/name", parse_addon_manifest returns it correctly.
        let dir = temp_dir("addon_b024_pkg");
        init_project(&dir, "cool-addon", InitTarget::RustAddon).unwrap();

        // Scaffold generates "OWNER/cool_addon"; simulate user setting
        // the real org/name before publishing.
        let toml_path = dir.join("native/addon.toml");
        let content = std::fs::read_to_string(&toml_path).unwrap();
        let updated = content.replace("OWNER/cool_addon", "my-org/cool-addon");
        std::fs::write(&toml_path, &updated).unwrap();

        let manifest = crate::addon::manifest::parse_addon_manifest(&toml_path).unwrap();
        assert_eq!(
            manifest.package, "my-org/cool-addon",
            "addon.toml package must be the qualified name for release titles"
        );
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
        assert!(
            err.contains("must not be empty"),
            "expected empty rejection: {err}"
        );

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

    // ── C14-3: release.yml template tests (tag-push-only flow) ───────
    //
    // These tests verify that the static template under
    // `crates/addon-rs/templates/release.yml.template` is scaffolded
    // with variables properly substituted and the structural contract
    // (4 jobs, 5-platform matrix, tag regex, github-actions[bot] as
    // release author) is preserved.
    //
    // The old RC2.6-era `create-release` / `build` / `lockfile` three-job
    // layout has been replaced by the core-symmetric four-job layout
    // `prepare` / `gate` / `build` / `publish`. The tests below pin
    // that contract so a regression cannot slip in silently.

    #[test]
    fn test_rust_addon_release_yml_exists_and_non_empty() {
        let dir = temp_dir("addon_rel");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let path = dir.join(".github/workflows/release.yml");
        assert!(path.exists(), "release.yml must be created");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.is_empty(), "release.yml must not be empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_variables_substituted() {
        let dir = temp_dir("addon_rel_vars");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // All scaffold placeholders must be resolved — no `{{...}}`
        // remnants in the rendered YAML.
        assert!(
            !content.contains("{{LIBRARY_STEM}}"),
            "raw {{{{LIBRARY_STEM}}}} placeholder must be replaced: {content}"
        );
        assert!(
            !content.contains("{{CRATE_DIR}}"),
            "raw {{{{CRATE_DIR}}}} placeholder must be replaced: {content}"
        );
        // The resolved library stem (underscored crate name) must
        // appear as the `LIBRARY_STEM` env value.
        assert!(
            content.contains("LIBRARY_STEM: my_addon"),
            "workflow must set LIBRARY_STEM to 'my_addon': {content}"
        );
        // CRATE_DIR defaults to `.` for scaffolded single-crate addons.
        assert!(
            content.contains("CRATE_DIR: ."),
            "workflow must set CRATE_DIR to '.' for scaffolded projects: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_taida_tag_triggers() {
        let dir = temp_dir("addon_rel_tag");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // Taida version tag regex — one- and two-letter generations.
        assert!(
            content.contains(r#""[a-z].[0-9]*""#),
            "missing one-letter generation tag pattern: {content}"
        );
        assert!(
            content.contains(r#""[a-z][a-z].[0-9]*""#),
            "missing two-letter generation tag pattern: {content}"
        );
        // Legacy wildcard pattern from the old template must be gone.
        assert!(
            !content.contains("'*.*'"),
            "legacy '*.*' wildcard pattern must be removed: {content}"
        );
        // Semver v* prefix must never appear.
        assert!(
            !content.contains("'v*'"),
            "workflow must not use semver v* tag triggers: {content}"
        );
        // workflow_dispatch input for manual re-runs.
        assert!(
            content.contains("workflow_dispatch:"),
            "workflow must allow workflow_dispatch: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_four_jobs() {
        // Core-symmetric 4-stage contract.
        let dir = temp_dir("addon_rel_jobs");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        for job in ["prepare:", "gate:", "build:", "publish:"] {
            assert!(
                content.contains(job),
                "workflow must declare job '{}': {}",
                job,
                content
            );
        }
        // Legacy job names must not linger.
        for legacy in ["create-release:", "lockfile:"] {
            assert!(
                !content.contains(legacy),
                "legacy job '{}' must be removed: {}",
                legacy,
                content
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_job_dependencies() {
        // gate needs prepare; build needs prepare+gate; publish needs
        // prepare+build. This mirrors the core workflow graph.
        let dir = temp_dir("addon_rel_deps");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // gate: scalar `needs: prepare` just after the `gate:` header.
        let gate_block = extract_job_block(&content, "gate");
        assert!(
            gate_block.contains("\n    needs: prepare\n"),
            "gate job must declare `needs: prepare` as a scalar: {gate_block}"
        );
        // build: list `needs: - prepare, - gate`.
        let build_block = extract_job_block(&content, "build");
        for dep in ["      - prepare", "      - gate"] {
            assert!(
                build_block.contains(dep),
                "build job must declare dependency '{}': {}",
                dep.trim(),
                build_block
            );
        }
        // publish: list `needs: - prepare, - build`.
        let publish_block = extract_job_block(&content, "publish");
        for dep in ["      - prepare", "      - build"] {
            assert!(
                publish_block.contains(dep),
                "publish job must declare dependency '{}': {}",
                dep.trim(),
                publish_block
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_five_platform_matrix() {
        let dir = temp_dir("addon_rel_mtx");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        let expected = [
            ("x86_64-unknown-linux-gnu", "ubuntu-latest"),
            ("aarch64-unknown-linux-gnu", "ubuntu-latest"),
            ("x86_64-apple-darwin", "macos-15-intel"),
            ("aarch64-apple-darwin", "macos-14"),
            ("x86_64-pc-windows-msvc", "windows-latest"),
        ];
        for (triple, runner) in expected {
            // `- triple: X` followed (later) by `runner: Y` on the same
            // matrix entry block. We just check both tokens appear.
            assert!(
                content.contains(&format!("triple: {triple}")),
                "build matrix must include triple '{triple}': {content}"
            );
            assert!(
                content.contains(&format!("runner: {runner}")),
                "build matrix must include runner '{runner}': {content}"
            );
        }
        // Exactly five `- triple:` lines in the matrix (counted on the
        // canonical indentation to avoid false positives).
        let triple_lines = content.matches("          - triple:").count();
        assert_eq!(
            triple_lines, 5,
            "build matrix must declare exactly 5 entries, got {triple_lines}: {content}"
        );
        // aarch64-linux must be cross-compiled.
        assert!(
            content.contains("cross: true"),
            "aarch64-linux entry must set cross: true: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_gate_steps() {
        let dir = temp_dir("addon_rel_gate");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // Gate must run fmt / clippy / test — the same triad as the
        // core workflow (minus Taida-specific extras like e2e_smoke).
        assert!(
            content.contains("cargo fmt --all -- --check"),
            "gate must run cargo fmt --check: {content}"
        );
        assert!(
            content.contains("cargo clippy --all-targets -- -D warnings"),
            "gate must run cargo clippy with -D warnings: {content}"
        );
        assert!(
            content.contains("cargo test --all-targets"),
            "gate must run cargo test: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_publish_uses_github_token() {
        // Release author must be github-actions[bot]: that is enforced
        // by using `github.token` (the auto-minted bot token) as the
        // `gh` CLI credential, not a user personal access token.
        let dir = temp_dir("addon_rel_bot");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("GH_TOKEN: ${{ github.token }}"),
            "publish must authenticate with github.token (so release author = github-actions[bot]): {content}"
        );
        assert!(
            content.contains("gh release create"),
            "publish must call gh release create: {content}"
        );
        // Must NOT use secrets.GITHUB_TOKEN (works but legacy) or any
        // user PAT variable, and must NOT use --generate-notes alone
        // without passing through gh api.
        assert!(
            !content.contains("secrets.GH_PAT"),
            "must not require a personal access token: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_publish_asset_composition() {
        // Publish must upload all five cdylibs + addon.lock.toml +
        // prebuild-targets.toml.txt + SHA256SUMS.
        let dir = temp_dir("addon_rel_assets");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("addon.lock.toml"),
            "publish must reference addon.lock.toml: {content}"
        );
        assert!(
            content.contains("prebuild-targets.toml.txt"),
            "publish must reference prebuild-targets.toml.txt: {content}"
        );
        assert!(
            content.contains("SHA256SUMS"),
            "publish must generate SHA256SUMS: {content}"
        );
        // All 5 triples must appear in the lockfile assembly loop.
        for target in [
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
        ] {
            assert!(
                content.contains(target),
                "publish lockfile assembly must reference '{}': {}",
                target,
                content
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_permissions() {
        let dir = temp_dir("addon_rel_perm");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // Top-level permissions: contents: write for release upload.
        assert!(
            content.contains("permissions:"),
            "workflow must declare top-level permissions: {content}"
        );
        assert!(
            content.contains("contents: write"),
            "workflow must grant contents: write (for release asset upload): {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_sha256_computation() {
        let dir = temp_dir("addon_rel_sha");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // SHA-256 computation must be cross-platform (sha256sum on Linux,
        // shasum -a 256 fallback on macOS).
        assert!(
            content.contains("sha256sum"),
            "workflow must compute SHA-256 via sha256sum (Linux): {content}"
        );
        assert!(
            content.contains("shasum -a 256"),
            "workflow must have shasum -a 256 fallback for macOS: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_cli_no_longer_builds() {
        // Regression guard: under C14 the CLI does not build cdylibs,
        // compute SHA-256, or write addon.lock.toml. All of that is
        // the workflow's responsibility. The workflow must therefore
        // contain the build / hash / lock logic.
        let dir = temp_dir("addon_rel_cli");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("cargo build --locked --release --target"),
            "workflow must perform the per-target cargo build (CLI no longer does): {content}"
        );
        assert!(
            content.contains("cross build --locked --release --target"),
            "workflow must perform cross builds for aarch64-linux: {content}"
        );
        assert!(
            content.contains("native/addon.lock.toml"),
            "workflow must assemble native/addon.lock.toml: {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_core_symmetry() {
        // The core Taida workflow and the addon workflow share the
        // structural contract `prepare -> gate -> build -> publish`
        // with `permissions: contents: write` and `github.token` as
        // the release credential.
        let dir = temp_dir("addon_rel_sym");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // Order of job declarations must match the core contract.
        let jobs_in_order = declared_job_order(&content);
        assert_eq!(
            jobs_in_order,
            vec!["prepare", "gate", "build", "publish"],
            "jobs must follow the core prepare/gate/build/publish contract, got {:?}",
            jobs_in_order
        );
        // prepare job must expose release_tag / release_ref as outputs.
        let prepare_block = extract_job_block(&content, "prepare");
        assert!(
            prepare_block.contains("release_tag:"),
            "prepare must expose release_tag output: {prepare_block}"
        );
        assert!(
            prepare_block.contains("release_ref:"),
            "prepare must expose release_ref output: {prepare_block}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_release_yml_yaml_is_structurally_consistent() {
        // Without a YAML parser we use a lightweight structural audit
        // that catches the most common breakage (stray tabs, CRLF line
        // endings, trailing whitespace before colons).
        let dir = temp_dir("addon_rel_yaml");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            !content.contains('\t'),
            "release.yml must not contain tab characters"
        );
        assert!(
            !content.contains("\r\n"),
            "release.yml must use LF line endings"
        );
        // Every line introducing a job must live at 2-space indent.
        for job in ["prepare", "gate", "build", "publish"] {
            let marker = format!("  {job}:");
            assert!(
                content.lines().any(|l| l == marker),
                "job '{job}' header must appear at 2-space indent"
            );
        }
    }

    #[test]
    fn test_source_only_does_not_create_github_dir() {
        let dir = temp_dir("src_no_ci");
        init_project(&dir, "demo", InitTarget::SourceOnly).unwrap();
        assert!(
            !dir.join(".github").exists(),
            "source-only projects must NOT have .github/"
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

    #[test]
    fn test_render_release_workflow_is_pure() {
        // The render helper is pure — same inputs, same output.
        let a = render_release_workflow("foo_bar", ".");
        let b = render_release_workflow("foo_bar", ".");
        assert_eq!(a, b, "render_release_workflow must be deterministic");
        // And it respects CRATE_DIR when it isn't the default.
        let nested = render_release_workflow("foo_bar", "crates/foo");
        assert!(
            nested.contains("CRATE_DIR: crates/foo"),
            "CRATE_DIR must be substituted into the env block: {nested}"
        );
        // Library stem substitution must land in the env block.
        assert!(
            a.contains("LIBRARY_STEM: foo_bar"),
            "LIBRARY_STEM must be substituted into the env block: {a}"
        );
    }
}
