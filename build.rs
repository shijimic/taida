//! C24 Phase 5 (RC-SLOW-2 / C24B-006): Build-time fixture enumeration.
//!
//! Generates two things consumed by collection-runner integration tests:
//!
//! 1. `pub const` fixture stem lists (for aggregate count/regression guards)
//! 2. Per-fixture `#[test]` function bodies, one for each fixture, that each
//!    forward into a shared runner. nextest sees one test per fixture and
//!    schedules them in parallel across CPUs.
//!
//! This replaces the previous monolithic pattern:
//! ```ignore
//! #[test]
//! fn wasm_wasi_parity_all_examples() {
//!     for td in read_dir("examples") { ... run ... collect failures ... }
//! }
//! ```
//! which (a) hid fixture-level failures behind one test name, (b) forced
//! strict sequential iteration even though fixtures are independent, and
//! (c) blocked nextest binary-level and test-level parallelism from
//! scaling across the ~80 fixtures per runner.
//!
//! Generated files (emitted to `$OUT_DIR`):
//!
//!  - `examples_all_td_fixtures.rs`      — `ALL_TD_FIXTURES: &[&str]`
//!  - `examples_compile_td_fixtures.rs`  — `COMPILE_TD_FIXTURES: &[&str]`
//!  - `examples_numbered_td_fixtures.rs` — `NUMBERED_TD_FIXTURES: &[&str]`
//!  - `quality_cross_module_fixtures.rs` — `QUALITY_CROSS_MODULE_FIXTURES: &[&str]`
//!  - `examples_all_td_tests.rs`         — `fixture_all_td_<stem>()` per fixture
//!  - `examples_compile_td_tests.rs`     — `fixture_compile_<stem>()` per fixture
//!  - `examples_numbered_td_tests.rs`    — `fixture_numbered_<stem>()` per fixture
//!  - `quality_cross_module_tests.rs`    — `fixture_quality_<name>()` per dir
//!
//! The `_tests.rs` files define test functions that call back into a runner
//! defined in the host test crate (e.g. `crate::run_wasm_wasi_fixture(&stem)`).
//! This keeps the generated file thin and testable helpers in plain Rust.
//!
//! Cargo reruns this script whenever the enumerated directories change.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let examples_dir = manifest_dir.join("examples");
    let quality_dir = examples_dir.join("quality");
    let crash_dir = manifest_dir.join("tests").join("crash_regression");

    // Rerun when any fixture in the enumerated sets is added/removed/renamed.
    println!("cargo:rerun-if-changed={}", examples_dir.display());
    println!("cargo:rerun-if-changed={}", quality_dir.display());
    println!("cargo:rerun-if-changed={}", crash_dir.display());
    println!("cargo:rerun-if-changed=build.rs");

    // ----- examples/*.td (wasm parity runners) -----
    let mut all_td: Vec<String> = read_td_stems(&examples_dir)
        .into_iter()
        .filter(|s| is_valid_rust_ident(s))
        .collect();
    all_td.sort();
    write_stem_list(
        &out_dir.join("examples_all_td_fixtures.rs"),
        "ALL_TD_FIXTURES",
        &all_td,
    );
    write_per_fixture_tests(&out_dir.join("examples_all_td_tests.rs"), "all_td", &all_td);

    // ----- examples/compile_*.td (three_way_parity / native_compile_parity) -----
    let mut compile_td: Vec<String> = all_td
        .iter()
        .filter(|s| s.starts_with("compile_"))
        .cloned()
        .collect();
    compile_td.sort();
    write_stem_list(
        &out_dir.join("examples_compile_td_fixtures.rs"),
        "COMPILE_TD_FIXTURES",
        &compile_td,
    );
    write_per_fixture_tests(
        &out_dir.join("examples_compile_td_tests.rs"),
        "compile",
        &compile_td,
    );

    // ----- examples/<digit>*.td (numbered_examples_native_parity) -----
    let mut numbered_td: Vec<String> = all_td
        .iter()
        .filter(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .cloned()
        .collect();
    numbered_td.sort();
    write_stem_list(
        &out_dir.join("examples_numbered_td_fixtures.rs"),
        "NUMBERED_TD_FIXTURES",
        &numbered_td,
    );
    write_per_fixture_tests(
        &out_dir.join("examples_numbered_td_tests.rs"),
        "numbered",
        &numbered_td,
    );

    // ----- tests/crash_regression/*.td (crash_regression_corpus_three_way) -----
    let mut crash_td: Vec<String> = read_td_stems(&crash_dir)
        .into_iter()
        .filter(|s| is_valid_rust_ident(s))
        .collect();
    crash_td.sort();
    write_stem_list(
        &out_dir.join("crash_regression_fixtures.rs"),
        "CRASH_REGRESSION_FIXTURES",
        &crash_td,
    );
    write_per_fixture_tests(
        &out_dir.join("crash_regression_tests.rs"),
        "crash",
        &crash_td,
    );

    // ----- tests/parity.rs + src/interpreter/net_h2.rs (NET6-5b release gate) -----
    //
    // C25B-003 Phase 2: eliminate cargo subprocess spawn from
    // `test_net6_5b_release_gate_v6_test_counts`. Previously the test shelled
    // out to `cargo test --test parity -- --list` and `cargo test --lib --
    // --list net_h2`, which on CI (2C) took ~79s/run due to cargo freshness
    // checks + spawn overhead + core contention (single SLOW test that alone
    // accounted for ~12% of CI wallclock, per PR #38 3-run median).
    //
    // Replacement: scan the source files at build time, emit
    //   - PARITY_TEST_FN_NAMES: &[&str]   -- fn identifiers discovered in
    //     tests/parity.rs with `#[test]` on a preceding line
    //   - NET_H2_UNIT_TEST_COUNT: usize   -- count of `#[test]` attributes
    //     inside the `#[cfg(test)] mod tests { ... }` region of net_h2.rs
    // The release gate test then reads these consts and performs pure
    // in-process assertions. Runtime drops from ~79s to sub-millisecond.
    let parity_rs = manifest_dir.join("tests").join("parity.rs");
    let net_h2_rs = manifest_dir
        .join("src")
        .join("interpreter")
        .join("net_h2.rs");
    println!("cargo:rerun-if-changed={}", parity_rs.display());
    println!("cargo:rerun-if-changed={}", net_h2_rs.display());

    let parity_fn_names = scan_test_fn_names(&parity_rs);
    let net_h2_test_count = count_cfg_test_attrs_in_tests_mod(&net_h2_rs);
    write_parity_release_gate(
        &out_dir.join("parity_release_gate.rs"),
        &parity_fn_names,
        net_h2_test_count,
    );

    // ----- examples/quality/*/main.{td,tdm} (quality_cross_module_*) -----
    let mut cross_module_dirs: Vec<String> = Vec::new();
    if quality_dir.exists()
        && let Ok(entries) = fs::read_dir(&quality_dir)
    {
        for entry in entries.flatten() {
            if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            if !path.join("main.td").exists() && !path.join("main.tdm").exists() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if is_valid_rust_ident(&name) {
                cross_module_dirs.push(name);
            }
        }
    }
    cross_module_dirs.sort();
    write_stem_list(
        &out_dir.join("quality_cross_module_fixtures.rs"),
        "QUALITY_CROSS_MODULE_FIXTURES",
        &cross_module_dirs,
    );
    write_per_fixture_tests(
        &out_dir.join("quality_cross_module_tests.rs"),
        "quality",
        &cross_module_dirs,
    );
}

fn read_td_stems(dir: &Path) -> Vec<String> {
    let mut stems = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return stems;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "td")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            stems.push(stem.to_string());
        }
    }
    stems
}

/// Rust identifier check (used to filter out fixtures whose stem cannot form
/// a valid `fn` name). Permits ASCII alphanumerics + underscore, must not
/// start with a digit.
///
/// For numbered examples like `01_hello`, callers prefix the generated test
/// name with `fixture_numbered_` so digits are allowed after the underscore;
/// we still want to reject stems with weird characters (hyphens, dots, etc)
/// because those would turn into illegal Rust idents.
fn is_valid_rust_ident(stem: &str) -> bool {
    if stem.is_empty() {
        return false;
    }
    stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn write_stem_list(out_path: &Path, const_name: &str, stems: &[String]) {
    let mut out = String::new();
    out.push_str("// @generated by build.rs -- DO NOT EDIT\n");
    out.push_str(&format!("pub const {}: &[&str] = &[\n", const_name));
    for stem in stems {
        out.push_str(&format!("    {:?},\n", stem));
    }
    out.push_str("];\n");
    fs::write(out_path, out).expect("failed to write generated fixture list");
}

/// Generate per-fixture `#[test]` functions that forward to a shared runner.
///
/// The generated file looks like:
///
/// ```ignore
/// // @generated by build.rs -- DO NOT EDIT
/// macro_rules! c24_fixture_runner { ($stem:expr) => { run_fixture($stem) } }
/// #[test] fn fixture_all_td_01_hello() { c24_fixture_runner!("01_hello"); }
/// ...
/// ```
///
/// The including test crate defines `run_fixture` (or uses a provided macro)
/// to specialize per-runner. The macro indirection lets the same generated
/// file be `include!`d by multiple test binaries if needed.
/// Scan a Rust source file for `fn <name>(` identifiers preceded (possibly
/// across blank lines and doc-comments) by a `#[test]` attribute. Returns
/// only names starting with `test_` (the NET6-5b release gate convention).
///
/// The scanner is deliberately simple: it walks lines top-to-bottom and
/// tracks whether the most recent non-comment / non-blank line was a
/// `#[test]` attribute. This matches the grep pattern used historically
/// by the release gate without requiring a full Rust parser.
fn scan_test_fn_names(path: &Path) -> Vec<String> {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut names = Vec::new();
    let mut pending_test_attr = false;
    for raw in source.lines() {
        let line = raw.trim_start();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with("#[test]") || line.starts_with("#[tokio::test") {
            pending_test_attr = true;
            continue;
        }
        if pending_test_attr {
            let after_fn = line
                .strip_prefix("fn ")
                .or_else(|| line.strip_prefix("async fn "))
                .or_else(|| line.strip_prefix("pub fn "))
                .or_else(|| line.strip_prefix("pub async fn "));
            if let Some(rest) = after_fn
                && let Some(end) = rest.find('(')
            {
                let name = rest[..end].trim();
                if name.starts_with("test_") && is_valid_rust_ident(name) {
                    names.push(name.to_string());
                }
            }
            pending_test_attr = false;
        }
    }
    names
}

/// Count `#[test]` attributes inside the first `#[cfg(test)] mod tests { ... }`
/// region of a source file. Uses simple brace-depth tracking scoped to the
/// region start, which suffices for our `net_h2.rs` layout.
fn count_cfg_test_attrs_in_tests_mod(path: &Path) -> usize {
    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut in_tests_mod = false;
    let mut depth: i32 = 0;
    let mut count: usize = 0;
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if !in_tests_mod {
            if trimmed.starts_with("#[cfg(test)]") {
                let mut found_mod = false;
                for lookahead in lines.by_ref() {
                    let la = lookahead.trim_start();
                    if la.is_empty() || la.starts_with("//") {
                        continue;
                    }
                    if la.starts_with("mod tests") || la.starts_with("pub mod tests") {
                        found_mod = true;
                    }
                    break;
                }
                if found_mod {
                    in_tests_mod = true;
                    depth = 1;
                    continue;
                }
            }
            continue;
        }
        for ch in line.chars() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth <= 0 {
                        return count;
                    }
                }
                _ => {}
            }
        }
        if trimmed.starts_with("#[test]") || trimmed.starts_with("#[tokio::test") {
            count += 1;
        }
    }
    count
}

fn write_parity_release_gate(out_path: &Path, fn_names: &[String], net_h2_count: usize) {
    let mut out = String::new();
    out.push_str("// @generated by build.rs -- DO NOT EDIT\n");
    out.push_str("// C25B-003 Phase 2: NET6-5b release gate source-scan artefact.\n");
    out.push_str("#[allow(dead_code)]\n");
    out.push_str("pub const PARITY_TEST_FN_NAMES: &[&str] = &[\n");
    for name in fn_names {
        out.push_str(&format!("    {:?},\n", name));
    }
    out.push_str("];\n");
    out.push_str("#[allow(dead_code)]\n");
    out.push_str(&format!(
        "pub const NET_H2_UNIT_TEST_COUNT: usize = {};\n",
        net_h2_count
    ));
    fs::write(out_path, out).expect("failed to write generated parity release gate consts");
}

fn write_per_fixture_tests(out_path: &Path, category: &str, stems: &[String]) {
    let mut out = String::new();
    out.push_str("// @generated by build.rs -- DO NOT EDIT\n");
    for stem in stems {
        // prefix defends against Rust `fn` names not starting with a digit.
        out.push_str(&format!(
            "#[test]\nfn fixture_{}_{}() {{ c24_fixture_runner!({:?}); }}\n",
            category, stem, stem
        ));
    }
    fs::write(out_path, out).expect("failed to write generated fixture tests");
}
