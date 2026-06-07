//! help — split out of src/main.rs (pure move).
//! Behaviour unchanged; imports added per cargo check.

use taida::version::taida_version;

pub(crate) fn print_cli_version() {
    println!("Taida Lang {}", taida_version());
}

pub(crate) fn print_cli_help() {
    println!(
        "\
Taida Lang {}

Usage:
  taida [--no-check] <FILE>
  taida [--no-check]
  taida <COMMAND> [OPTIONS]

Commands:
  build       Build Native or WASM output
  way         Quality hub: check, lint, verify, todo
  graph       AI-oriented structural JSON for codebase comprehension
  doc         Generate docs from doc comments
  ingot       Package/dependency hub: deps, install, update, publish, cache
  init        Initialize a Taida project
  lsp         Run the language server over stdio
  auth        Manage authentication state
  community   Access community features
  upgrade     Upgrade taida to a newer version

Global options:
  --help, -h     Show this help
  --version, -V  Show version
  --no-check     Skip type checking where supported

Use `taida <COMMAND> --help` for command-specific usage.",
        taida_version()
    );
}

pub(crate) fn print_graph_help() {
    println!(
        "\
Usage:
  taida graph [-o OUTPUT] [--recursive] <PATH>
  taida graph summary [--format text|json|sarif] <PATH>

Options:
  --recursive, -r   Follow imports recursively and produce unified multi-module JSON
  --output, -o      Output path (bare filename writes into .taida/graph/)
  --format, -f      Summary output format: text | json | sarif

Output:
  AI-oriented unified JSON — types, functions, flow, imports, exports

Examples:
  taida graph examples/04_functions.td
  taida graph summary --format json examples/04_functions.td
  taida graph --recursive examples/complex/inventory/main.td
  taida graph -o snapshot.json examples/04_functions.td"
    );
}

pub(crate) fn print_graph_summary_help() {
    println!(
        "\
Usage:
  taida graph summary [--format text|json|sarif] <PATH>

Options:
  --format, -f    text | json | sarif

Examples:
  taida graph summary main.td
  taida graph summary --format sarif main.td"
    );
}

pub(crate) fn print_way_help() {
    println!(
        "\
Usage:
  taida way <PATH>
  taida way check <PATH>
  taida way lint <PATH>
  taida way verify <PATH>
  taida way todo [PATH]

Commands:
  check    Parse + type front gate
  lint     Naming-convention lint
  verify   Structural verification checks
  todo     Scan TODO/Stub molds

Notes:
  `taida way <PATH>` is the full quality gate. It runs check, lint, and verify.
  `--no-check` is not accepted under `taida way`."
    );
}

pub(crate) fn print_ingot_help() {
    println!(
        "\
Usage:
  taida ingot [--help]
  taida ingot deps
  taida ingot install [--force-refresh | --no-remote-check] [--allow-local-addon-build] [--allow-fresh] [--frozen]
  taida ingot migrate-lockfile
  taida ingot update [--allow-local-addon-build]
  taida ingot publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]
  taida ingot cache [clean] [--addons|--store|--store-pkg <org>/<name>|--all] [--yes]

Commands:
  deps      Resolve/install dependencies strictly
  install   Install dependencies and write lockfile
  migrate-lockfile
            Rewrite legacy taida.lock entries to the current SHA-256 schema
  update    Update dependencies and lockfile
  publish   Push a package tag; CI creates release assets
  cache     Manage WASM/runtime/addon caches

Notes:
  `taida ingot` without a subcommand prints this help and exits successfully.
  Dependencies are declared in packages.tdm with `>>> author/pkg@a.1`.
  `taida ingot <author/package>` is not a supported form."
    );
}

pub(crate) fn print_check_help() {
    println!(
        "\
Usage:
  taida way check [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>

Options:
  --format, -f    text | json | jsonl | sarif
  --strict        Treat WARNING diagnostics as failure
  --quiet, -q     Suppress diagnostic output

Examples:
  taida way check src
  taida way check --format json main.td"
    );
}

pub(crate) fn print_build_help() {
    println!(
        "\
Usage:
  taida build [native|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--no-cache] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] [--handler SYMBOL] <PATH>
  taida build <PATH> --unit NAME [--release] [--diag-format text|jsonl]
  taida build <PATH> --plan NAME [--release] [--diag-format text|jsonl]
  taida build <PATH> --all-units [--release] [--diag-format text|jsonl]

Options:
  --output, -o    Output file or directory
  --outdir        Alias of `--output`
  --entry         Native dir entry override (default: main.td)
  --handler       Native/WASM handler entry symbol
  --release, -r   Fail if TODO/Stub remains in source
  --no-cache      Disable WASM runtime .o cache
  --diag-format   text | jsonl
  --unit          Descriptor build: build one exported BuildUnit by name
  --plan          Descriptor build: build one exported BuildPlan by name
  --all-units     Descriptor build: build all exported BuildUnit values

Examples:
  taida build app.td
  taida build wasm-wasi src
  taida build --release app.td
  taida build app.td --unit server-x

Notes:
  Target defaults to native when omitted.
  The legacy js target remains hidden for transitional compatibility; it is not part of the release parity contract.
  Descriptor mode does not accept a positional target.
  `--no-check` is a global option and applies here."
    );
}

pub(crate) fn print_todo_help() {
    println!(
        "\
Usage:
  taida way todo [--format text|json|jsonl|sarif] [--strict] [--quiet] [PATH]

Options:
  --format, -f    text | json | jsonl | sarif
  --strict        Accepted for `way` flag consistency
  --quiet, -q     Suppress diagnostic output

Examples:
  taida way todo
  taida way todo --format json src"
    );
}

pub(crate) fn print_verify_help() {
    println!(
        "\
Usage:
  taida way verify [--check CHECK] [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>

Options:
  --check, -c     Run a specific check (repeatable)
  --format, -f    text | json | jsonl | sarif
  --strict        Treat WARNING findings as failure
  --quiet, -q     Suppress diagnostic output

Examples:
  taida way verify src
  taida way verify --check error-coverage --format jsonl main.td"
    );
}

pub(crate) fn print_init_help() {
    println!(
        "\
Usage:
  taida init [--target rust-addon] [DIR]

Options:
  --target rust-addon  Scaffold a Rust addon project (Cargo.toml, src/lib.rs,
                       native/addon.toml, taida/<name>.td, README.md)

Examples:
  taida init hello-taida
  taida init --target rust-addon my-addon"
    );
}

pub(crate) fn print_deps_help() {
    println!(
        "\
Usage:
  taida ingot deps

Behavior:
  Resolve dependencies strictly and stop before install/lockfile update on any error.

Example:
  taida ingot deps"
    );
}

pub(crate) fn print_install_help() {
    println!(
        "\
Usage:
  taida ingot install [--force-refresh | --no-remote-check] [--allow-local-addon-build] [--allow-fresh] [--frozen]

Behavior:
  Install resolved dependencies and generate/update `.taida/taida.lock`.

  For addons with a `[library.prebuild]` section in `native/addon.toml`,
  downloads the prebuild binary for the current host target, verifies its
  SHA-256 against the manifest, and places it in
  `.taida/deps/<pkg>/native/lib<name>.<ext>`. Downloads are cached under
  `~/.taida/addon-cache/`; use `taida ingot cache clean --addons` to prune.

  Large addon downloads (>= 256 KiB) show a progress indicator on stderr.

  C17: before reusing a cached `~/.taida/store/<pkg>/<version>/` entry,
  `taida ingot install` compares the resolved commit SHA of `<version>` on the
  remote with the `commit_sha` recorded in the store `_meta.toml` sidecar.
  When they differ (tag was retagged / recreated), the store entry is
  re-extracted automatically. Offline or unverifiable states emit a
  warning to stderr but never silently skip.

  `.taida/taida.lock` uses schema v3 and SHA-256 integrity. Legacy lockfiles
  and `fnv1a:` integrity are rejected. Run
  `taida ingot migrate-lockfile` once after installing dependencies to rewrite
  an old lockfile from the installed `.taida/deps` tree.

Options:
  --force-refresh              Invalidate the cached store entry for every
                               registry dependency and re-extract it. Also
                               ignores the addon-cache (legacy behaviour).
                               Mutually exclusive with --no-remote-check.
  --no-remote-check            Skip the remote commit-SHA lookup; trust the
                               existing store sidecar. Intended for offline
                               or rate-limited environments. Mutually
                               exclusive with --force-refresh.
  --allow-local-addon-build    When a prebuild is missing or unavailable, fall back
                               to building the addon from source using `cargo build`.
                               Integrity mismatches are never overridden by fallback.
  --allow-fresh                Allow a third-party addon release before the
                               default cooling-off window has elapsed.
  --frozen                     Require `.taida/taida.lock` to already match the
                               resolved `(name, version, integrity)` triples.
                               No lockfile writes are allowed.

Example:
  taida ingot install
  taida ingot install --force-refresh
  taida ingot install --no-remote-check
  taida ingot install --allow-local-addon-build
  taida ingot install --allow-fresh
  taida ingot install --frozen"
    );
}

pub(crate) fn print_update_help() {
    println!(
        "\
Usage:
  taida ingot update [--allow-local-addon-build]

Behavior:
  Resolve dependencies with remote-preferred generation lookup, then reinstall and update lockfile.

Options:
  --allow-local-addon-build    When a prebuild is missing or unavailable, fall back
                               to building the addon from source using `cargo build`.
                               Integrity mismatches are never overridden by fallback.

Example:
  taida ingot update
  taida ingot update --allow-local-addon-build"
    );
}

#[cfg(feature = "community")]
pub(crate) fn print_publish_help() {
    println!(
        "\
Usage:
  taida ingot publish [--label LABEL] [--force-version VERSION] [--retag] [--dry-run]

C14 tag-only publish:
  `taida ingot publish` only creates and pushes a git tag. It does not build
  cdylibs, does not compute SHA-256 digests, does not push to `main`,
  and does not call `gh release create`. The addon's CI
  (`.github/workflows/release.yml`) is the exclusive owner of release
  artefact build and upload — the release author will be
  `github-actions[bot]`, not the CLI user.

Options:
  --label LABEL            Attach a pre-release label (rc, rc2, beta, alpha-1, ...)
                           Applied on top of the auto-detected next version.
  --force-version VERSION  Override the auto-detected version. Must be a
                           valid Taida version (`gen.num(.label)?`).
  --retag                  Allow re-tagging an existing tag. The existing
                           remote tag is force-replaced.
  --dry-run                Print the publish plan (next version, tag, push
                           target) without making any git changes.

Auto version bump:
  - First release (no previous tag)      -> a.1
  - Public API removal or rename         -> generation bump (a.3 -> b.1)
  - Public API addition or internal only -> number bump     (a.3 -> a.4)

Examples:
  taida ingot publish --dry-run
  taida ingot publish
  taida ingot publish --label rc
  taida ingot publish --force-version a.5
  taida ingot publish --force-version a.5.rc --retag"
    );
}

pub(crate) fn print_doc_help() {
    println!(
        "\
Usage:
  taida doc generate [-o OUTPUT] <PATH>

Options:
  --output, -o    Output path (stdout when omitted)

Examples:
  taida doc generate src
  taida doc generate -o docs/api.md src"
    );
}

#[cfg(feature = "lsp")]
pub(crate) fn print_lsp_help() {
    println!(
        "\
Usage:
  taida lsp

Behavior:
  Start the Taida language server over stdio."
    );
}

pub(crate) fn print_lint_help() {
    println!(
        "\
Usage:
  taida way lint [--format text|json|jsonl|sarif] [--strict] [--quiet] <PATH>

Description:
  Run the naming-convention lint pass over <PATH>. <PATH> may be
  a single .td file or a directory (.td files are collected recursively).
  The lint enforces category-based naming rules and emits diagnostics in the
  E1801..E1809 band.

Exit codes:
  0   No lint diagnostics surfaced.
  1   At least one E18xx diagnostic was reported.
  2   Argument / IO / parse / type error (lint cannot run cleanly).

Options:
  --format, -f    text | json | jsonl | sarif
  --strict        Treat lint diagnostics as failure (same as default)
  --quiet         Suppress diagnostic output, exit code only.
  --help, -h      Show this help.

Examples:
  taida way lint examples
  taida way lint --quiet src/main.td"
    );
}

pub(crate) fn print_upgrade_help() {
    println!(
        "\
Usage:
  taida upgrade [--check] [--gen GEN] [--label LABEL] [--version VERSION]

Options:
  --check          Check for updates without installing
  --gen GEN        Filter by generation (e.g. b)
  --label LABEL    Filter by label (e.g. rc2)
  --version VER    Upgrade to an exact version (e.g. @b.10.rc2)

Notes:
  --gen and --label can be combined.
  --version is mutually exclusive with --gen/--label.
  By default, upgrades to the latest stable version.
  AST rewrite flags (`--d28`, `--d29`, `--e30`) were removed in @e.X.
  No migration command is provided.
  Windows: only --check is supported (self-replace is not yet implemented).

Examples:
  taida upgrade
  taida upgrade --check
  taida upgrade --label rc2
  taida upgrade --gen b
  taida upgrade --version @b.10.rc2"
    );
}

pub(crate) fn print_build_usage_and_exit() -> ! {
    eprintln!(
        "\
Usage:
  taida build [native|wasm-min|wasm-wasi|wasm-edge|wasm-full] [--release] [--no-cache] [--diag-format text|jsonl] [-o OUTPUT] [--entry ENTRY] [--handler SYMBOL] <PATH>
  taida build <PATH> --unit NAME [--release] [--diag-format text|jsonl]
  taida build <PATH> --plan NAME [--release] [--diag-format text|jsonl]
  taida build <PATH> --all-units [--release] [--diag-format text|jsonl]

Options:
  --output, -o    Output file or directory
  --outdir        Alias of `--output`
  --entry         Native dir entry override (default: main.td)
  --handler       Native/WASM handler entry symbol
  --release, -r   Fail if TODO/Stub remains in source
  --no-cache      Disable WASM runtime .o cache
  --diag-format   text | jsonl
  --unit          Descriptor build: build one exported BuildUnit by name
  --plan          Descriptor build: build one exported BuildPlan by name
  --all-units     Descriptor build: build all exported BuildUnit values
  --run-hooks     Descriptor build: execute BuildHook before hooks"
    );
    std::process::exit(1);
}
