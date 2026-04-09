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

    // ── .github/workflows/release.yml (RC2.6-4a, 4b) ──
    // CI workflow template for cross-platform prebuild release.
    // Placeholders {LIBRARY_STEM} and {PACKAGE_NAME} are replaced
    // with values derived from the project name.
    let workflow = ci_release_workflow(&crate_name, name);
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

/// Generate the `.github/workflows/release.yml` CI template (RC2.6-4a).
///
/// This workflow is triggered by tag pushes matching Taida version
/// patterns (`a.1`, `b.3`, `aa.5.rc`, etc.). It creates a GitHub
/// Release, builds the addon cdylib on four platforms via cross-compile,
/// computes SHA-256 for each, uploads them as release assets, then
/// generates and uploads `addon.lock.toml`.
///
/// Design note (B-005): `addon.lock.toml` is **release-asset only** --
/// it is NOT committed back into the repo after the tag. This avoids
/// the exact-tag metadata ordering problem where `taida install` reads
/// the tarball at the tagged commit, which would not include post-tag
/// commits.
fn ci_release_workflow(library_stem: &str, _package_name: &str) -> String {
    // The template is a single YAML string with the library stem baked
    // in at scaffold time. All occurrences of the library stem are
    // directly substituted — there are no residual placeholders like
    // `{LIBRARY_STEM}` or env-var indirections in the final output.
    //
    // GitHub Actions expressions use `${{ ... }}` which collides with
    // Rust format's `{...}`. We escape them as `${{{{ ... }}}}` in the
    // raw string so they render as `${{ ... }}` in the output.
    //
    // Architecture (B-005):
    //   1. `create-release` job: creates the GitHub Release from the tag.
    //   2. `build` matrix: 4 platforms build cdylib with `--target`,
    //      compute SHA-256 (cross-platform), rename to canonical name,
    //      upload cdylib to release, and upload a sha256-<target>.txt
    //      artifact with the hash.
    //   3. `lockfile` job: downloads all sha256 artifacts, assembles
    //      `addon.lock.toml`, and uploads it as a release asset.
    //
    // We use artifacts (not matrix job outputs) to pass SHA-256 values
    // between jobs because GitHub Actions matrix outputs from parallel
    // legs race and only the last writer wins.
    format!(
        r##"# .github/workflows/release.yml
#
# Release addon prebuild binaries for Taida.
# Generated by `taida init --target rust-addon`.
#
# Triggered by Taida version tags (e.g., `a.1`, `b.3`).
# Each matrix job builds the cdylib, computes SHA-256, and uploads the
# binary to the GitHub Release. A final job collects all SHA-256 values
# and uploads `addon.lock.toml` as a release asset.
#
# See https://github.com/taida-lang/taida for documentation.

name: Release addon prebuild

on:
  push:
    # Taida version tags: a.1, b.3, aa.5, a.1.rc, etc.
    # '*.*' matches any tag containing a dot, which covers all Taida
    # generation-based versions. Semver v* tags are not used in Taida.
    tags: ['*.*']

permissions:
  contents: write

env:
  LIBRARY_STEM: {library_stem}

jobs:
  create-release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Create GitHub Release (idempotent)
        run: |
          if gh release view "${{{{ github.ref_name }}}}" &>/dev/null; then
            echo "Release ${{{{ github.ref_name }}}} already exists, skipping creation"
          else
            gh release create "${{{{ github.ref_name }}}}" --generate-notes
          fi
        env:
          GH_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}

  build:
    needs: create-release
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            ext: so
          - os: macos-latest
            target: x86_64-apple-darwin
            ext: dylib
          - os: macos-14
            target: aarch64-apple-darwin
            ext: dylib
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            ext: dll
    runs-on: ${{{{ matrix.os }}}}
    steps:
      - uses: actions/checkout@v4

      # taida-addon crate is resolved via [patch.crates-io] from a
      # local taida checkout. Clone it into the workspace and rewrite
      # the patch path so cargo can find it on CI runners.
      # (actions/checkout@v4 rejects paths outside $GITHUB_WORKSPACE,
      # so we use git clone directly.)
      - name: Clone taida for taida-addon crate
        shell: bash
        run: git clone --depth 1 https://github.com/taida-lang/taida.git taida-deps

      - name: Rewrite patch path for CI
        shell: bash
        run: |
          sed -i.bak 's|path = "../taida/crates/addon-rs"|path = "taida-deps/crates/addon-rs"|' Cargo.toml
          rm -f Cargo.toml.bak

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{{{ matrix.target }}}}

      - name: Build cdylib
        run: cargo build --release --lib --target ${{{{ matrix.target }}}}

      - name: Compute SHA-256
        id: sha
        shell: bash
        run: |
          if [[ "${{{{ matrix.os }}}}" == "windows-latest" ]]; then
            CDYLIB="{library_stem}.${{{{ matrix.ext }}}}"
          else
            CDYLIB="lib{library_stem}.${{{{ matrix.ext }}}}"
          fi
          FILE="target/${{{{ matrix.target }}}}/release/$CDYLIB"
          if command -v sha256sum &>/dev/null; then
            SHA=$(sha256sum "$FILE" | awk '{{print $1}}')
          else
            SHA=$(shasum -a 256 "$FILE" | awk '{{print $1}}')
          fi
          echo "sha256=$SHA" >> "$GITHUB_OUTPUT"
          echo "cdylib=$CDYLIB" >> "$GITHUB_OUTPUT"
          echo "$SHA" > "sha256-${{{{ matrix.target }}}}.txt"

      - name: Rename cdylib to canonical name
        shell: bash
        run: |
          cp "target/${{{{ matrix.target }}}}/release/$CDYLIB" "$CANONICAL"
        env:
          CDYLIB: ${{{{ steps.sha.outputs.cdylib }}}}
          CANONICAL: lib${{{{ env.LIBRARY_STEM }}}}-${{{{ matrix.target }}}}.${{{{ matrix.ext }}}}

      - name: Upload cdylib to release
        shell: bash
        run: |
          # RC2.6B-021: If the asset already exists (e.g. from local
          # publish), keep it and recompute sha256-*.txt from the
          # existing asset so the lockfile job stays consistent.
          if gh release view "${{{{ github.ref_name }}}}" --json assets -q '.assets[].name' | grep -qx "$CANONICAL"; then
            echo "Asset $CANONICAL already exists on release, recomputing SHA from existing asset"
            gh release download "${{{{ github.ref_name }}}}" -p "$CANONICAL" -D /tmp/existing-asset --clobber
            if command -v sha256sum &>/dev/null; then
              SHA=$(sha256sum "/tmp/existing-asset/$CANONICAL" | awk '{{print $1}}')
            else
              SHA=$(shasum -a 256 "/tmp/existing-asset/$CANONICAL" | awk '{{print $1}}')
            fi
            echo "$SHA" > "sha256-${{{{ matrix.target }}}}.txt"
            echo "Overwrote sha256-${{{{ matrix.target }}}}.txt with existing asset SHA: $SHA"
          else
            gh release upload "${{{{ github.ref_name }}}}" "$CANONICAL"
          fi
        env:
          CANONICAL: lib${{{{ env.LIBRARY_STEM }}}}-${{{{ matrix.target }}}}.${{{{ matrix.ext }}}}
          GH_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}

      - name: Upload SHA-256 artifact
        uses: actions/upload-artifact@v4
        with:
          name: sha256-${{{{ matrix.target }}}}
          path: sha256-${{{{ matrix.target }}}}.txt
          retention-days: 1

  lockfile:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Download all SHA-256 artifacts
        uses: actions/download-artifact@v4
        with:
          pattern: sha256-*
          merge-multiple: true

      - name: Generate addon.lock.toml
        shell: bash
        run: |
          mkdir -p native
          printf '# native/addon.lock.toml\n' > native/addon.lock.toml
          printf '# Generated by CI. Do NOT edit manually.\n\n' >> native/addon.lock.toml
          printf '[targets]\n' >> native/addon.lock.toml
          for target in \
            x86_64-unknown-linux-gnu \
            x86_64-apple-darwin \
            aarch64-apple-darwin \
            x86_64-pc-windows-msvc; do
            SHA=$(cat "sha256-$target.txt" 2>/dev/null || echo "MISSING")
            printf '"%s" = "sha256:%s"\n' "$target" "$SHA" >> native/addon.lock.toml
          done

      - name: Upload lockfile to release
        run: gh release upload "${{{{ github.ref_name }}}}" native/addon.lock.toml --clobber
        env:
          GH_TOKEN: ${{{{ secrets.GITHUB_TOKEN }}}}
"##
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

    // ── CI workflow template tests (RC2.6-4a, 4b) ──────────

    #[test]
    fn test_rust_addon_ci_workflow_exists_and_non_empty() {
        let dir = temp_dir("addon_ci");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let path = dir.join(".github/workflows/release.yml");
        assert!(path.exists(), "release.yml must be created");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.is_empty(), "release.yml must not be empty");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_no_raw_placeholders() {
        let dir = temp_dir("addon_ci_ph");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // {LIBRARY_STEM} and {PACKAGE_NAME} must be resolved
        assert!(
            !content.contains("{LIBRARY_STEM}"),
            "raw {{LIBRARY_STEM}} placeholder must be replaced: {content}"
        );
        assert!(
            !content.contains("{PACKAGE_NAME}"),
            "raw {{PACKAGE_NAME}} placeholder must be replaced: {content}"
        );
        // The resolved library stem should appear
        assert!(
            content.contains("my_addon"),
            "workflow must contain resolved library stem 'my_addon': {content}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_has_matrix_targets() {
        let dir = temp_dir("addon_ci_mx");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("x86_64-unknown-linux-gnu"),
            "missing linux target"
        );
        assert!(
            content.contains("x86_64-apple-darwin"),
            "missing macOS x86 target"
        );
        assert!(
            content.contains("aarch64-apple-darwin"),
            "missing macOS ARM target"
        );
        assert!(
            content.contains("x86_64-pc-windows-msvc"),
            "missing Windows target"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_has_taida_tag_triggers() {
        let dir = temp_dir("addon_ci_tag");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // Taida version tags: '*.*' covers all generation-based versions
        assert!(content.contains("'*.*'"), "missing '*.*' tag trigger");
        // Must NOT contain semver-style v* prefix
        assert!(
            !content.contains("'v*'"),
            "workflow must not use semver v* tag triggers"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_has_lockfile_job() {
        let dir = temp_dir("addon_ci_lock");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("lockfile:"),
            "workflow must have lockfile job"
        );
        assert!(
            content.contains("addon.lock.toml"),
            "workflow must reference addon.lock.toml"
        );
        assert!(
            content.contains("needs: build"),
            "lockfile job must depend on build"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_sha256_computation() {
        let dir = temp_dir("addon_ci_sha");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("sha256sum"),
            "workflow must compute SHA-256 (Linux)"
        );
        assert!(
            content.contains("shasum -a 256"),
            "workflow must have macOS fallback (shasum -a 256)"
        );
        assert!(
            content.contains("Compute SHA-256"),
            "workflow must have SHA computation step"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_has_create_release_job() {
        let dir = temp_dir("addon_ci_rel");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("create-release:"),
            "workflow must have create-release job"
        );
        assert!(
            content.contains("gh release create"),
            "create-release job must run gh release create"
        );
        assert!(
            content.contains("needs: create-release"),
            "build job must depend on create-release"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_has_permissions() {
        let dir = temp_dir("addon_ci_perm");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("permissions:"),
            "workflow must declare permissions"
        );
        assert!(
            content.contains("contents: write"),
            "workflow must have contents: write permission"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_cross_compile() {
        let dir = temp_dir("addon_ci_cross");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("--target ${{ matrix.target }}"),
            "cargo build must use --target flag for cross-compile"
        );
        // Output path must use target-specific directory
        assert!(
            content.contains("target/${{ matrix.target }}/release/"),
            "output path must include target-specific directory"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_canonical_rename() {
        let dir = temp_dir("addon_ci_rename");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        assert!(
            content.contains("Rename cdylib to canonical name"),
            "workflow must have rename step"
        );
        // The gh release upload line must NOT use '#' display-label syntax
        // for asset renaming (that only sets the label, not the filename).
        for line in content.lines() {
            if line.contains("gh release upload") {
                assert!(
                    !line.contains('#'),
                    "gh release upload must not use '#' display-label syntax: {line}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
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

    // ── RC2.6B-021: CI template cdylib upload must not use --clobber ──

    #[test]
    fn test_rust_addon_ci_workflow_no_clobber_on_cdylib_upload() {
        let dir = temp_dir("addon_ci_no_clobber");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // The cdylib upload step must NOT use --clobber so that a
        // local-publish cdylib is not overwritten by CI.
        // However, the lockfile upload MAY use --clobber (it's a
        // complete replacement generated by CI).
        let mut in_cdylib_upload = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- name: Upload cdylib") {
                in_cdylib_upload = true;
                continue;
            }
            if trimmed.starts_with("- name:") && in_cdylib_upload {
                in_cdylib_upload = false;
            }
            if in_cdylib_upload && trimmed.contains("gh release upload") {
                assert!(
                    !trimmed.contains("--clobber"),
                    "cdylib upload step must NOT use --clobber (RC2.6B-021): {trimmed}"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rust_addon_ci_workflow_recomputes_sha_for_existing_asset() {
        // RC2.6B-021: when cdylib upload is skipped because the asset
        // already exists, the CI must download the existing asset,
        // recompute its SHA, and overwrite sha256-*.txt so the lockfile
        // job uses the correct hash.
        let dir = temp_dir("addon_ci_sha_recompute");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        let mut in_cdylib_upload = false;
        let mut found_download = false;
        let mut found_sha_overwrite = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- name: Upload cdylib") {
                in_cdylib_upload = true;
                continue;
            }
            if trimmed.starts_with("- name:") && in_cdylib_upload {
                break;
            }
            if in_cdylib_upload {
                if trimmed.contains("gh release download") {
                    found_download = true;
                }
                if trimmed.contains("sha256-")
                    && trimmed.contains(".txt")
                    && trimmed.contains("> \"")
                {
                    found_sha_overwrite = true;
                }
            }
        }
        assert!(
            found_download,
            "cdylib upload step must download existing asset when it already exists"
        );
        assert!(
            found_sha_overwrite,
            "cdylib upload step must overwrite sha256-*.txt with existing asset's SHA"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── RC2.6B-023: CI template steps must use shell: bash ──

    #[test]
    fn test_rust_addon_ci_workflow_upload_uses_bash_shell() {
        let dir = temp_dir("addon_ci_bash");
        init_project(&dir, "my-addon", InitTarget::RustAddon).unwrap();
        let content = std::fs::read_to_string(dir.join(".github/workflows/release.yml")).unwrap();
        // The upload step and rename step must use `shell: bash` so
        // that $CANONICAL env var expansion works on Windows (B-023).
        let lines: Vec<&str> = content.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed == "- name: Upload cdylib to release"
                || trimmed == "- name: Rename cdylib to canonical name"
            {
                // Look ahead for `shell: bash` before the next step
                let mut found_shell_bash = false;
                for next_line in &lines[(i + 1)..] {
                    let next = next_line.trim();
                    if next.starts_with("- name:") || next.starts_with("- uses:") {
                        break;
                    }
                    if next == "shell: bash" {
                        found_shell_bash = true;
                        break;
                    }
                }
                assert!(
                    found_shell_bash,
                    "step '{}' must use `shell: bash` (RC2.6B-023)",
                    trimmed
                );
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
