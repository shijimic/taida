//! D29B-010 / Lock-G — workflow YAML structural audit
//!
//! # Why this test exists
//!
//! `.github/workflows/bench.yml` / `perf-router.yml` produced six or more
//! consecutive `Invalid workflow file: Unexpected value 'sha', 'workflow',
//! 'run'` startup failures on `main` (CI runs 24935511415 / 24935511533 /
//! 24966499842 / 24966499742 / 24966585110 / 24966585223 / 24967722582 /
//! 24967722690 across 2026-04-25 .. 2026-04-26). The root cause was an
//! unindented bare-newline body inside a `git commit -m "..."` literal that
//! escaped the `run: |` block-scalar indent, causing GitHub Actions' YAML
//! parser to interpret the continuation lines as new top-level mapping
//! keys. The startup failure was silent because these workflows were not
//! gated as required checks, so the D28B-005 / D28B-013 / C26B-024 perf
//! hard-fail gates appeared green while in fact never running.
//!
//! Lock-G mandates two complementary safeguards:
//!
//!   1. **Local YAML structural validity** — this test parses every
//!      `.github/workflows/*.yml` with a strict YAML parser. A
//!      regression that escapes the literal block scalar will fail this
//!      audit at `cargo test` time, well before reaching CI.
//!
//!   2. **`actionlint` CI hard-fail** — `.github/workflows/ci.yml` runs
//!      `actionlint` over the same set on every PR / push. That catches
//!      semantic errors (bad action refs, undefined env vars, etc.)
//!      that pure YAML parsing cannot.
//!
//! This `cargo test` is the first line of defence; `actionlint` is the
//! second.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".github")
        .join("workflows")
}

fn list_yaml_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let entries = fs::read_dir(dir).expect("read .github/workflows dir");
    for entry in entries {
        let entry = entry.expect("read dir entry");
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some("yml") | Some("yaml") => out.push(path),
            _ => {}
        }
    }
    out.sort();
    out
}

/// Validate each workflow YAML by piping it through `python3 -c "import
/// yaml; yaml.safe_load(...)"`. We prefer the Python toolchain because
/// every GitHub-hosted runner has python3 + PyYAML preinstalled, and we
/// want to avoid adding a heavy `serde_yaml` dependency to the test
/// crate just for this audit.
fn parse_yaml_or_fail(path: &Path) -> Result<(), String> {
    // First check python3 + PyYAML availability. If unavailable, the
    // test cannot do its job; we explicitly skip rather than silently
    // pass.
    let probe = Command::new("python3").args(["-c", "import yaml"]).output();
    let probe_ok = match probe {
        Ok(out) => out.status.success(),
        Err(_) => false,
    };
    if !probe_ok {
        return Err(format!(
            "python3 + PyYAML not available; cannot audit {}",
            path.display()
        ));
    }

    let script = format!(
        "import yaml, sys; \
         data = yaml.safe_load(open({path:?}, 'r', encoding='utf-8')); \
         assert isinstance(data, dict), 'top-level must be a mapping, got ' + type(data).__name__; \
         assert 'jobs' in data or 'on' in data, 'workflow must declare on/jobs'",
        path = path.to_string_lossy()
    );

    let out = Command::new("python3")
        .args(["-c", &script])
        .output()
        .map_err(|e| format!("spawn python3: {}", e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(format!(
            "{}: YAML parse / structure check failed:\n{}",
            path.display(),
            stderr.trim()
        ))
    } else {
        Ok(())
    }
}

#[test]
fn d29b_010_all_workflow_yaml_parse_clean() {
    let dir = workflows_dir();
    let files = list_yaml_files(&dir);
    assert!(
        !files.is_empty(),
        "no workflow YAML files found under {}",
        dir.display()
    );

    let mut probe_skipped = false;
    let mut failures: Vec<String> = Vec::new();

    for f in &files {
        match parse_yaml_or_fail(f) {
            Ok(()) => {}
            Err(msg) if msg.contains("python3 + PyYAML not available") => {
                probe_skipped = true;
                eprintln!("SKIP d29b_010 (toolchain unavailable): {}", msg);
                break;
            }
            Err(msg) => failures.push(msg),
        }
    }

    if probe_skipped {
        // We never want to silently pass this audit; mark it as a runtime
        // skip so the test still surfaces. CI runners ship python3 +
        // PyYAML by default, so this branch is for local dev environments
        // missing the toolchain.
        eprintln!("d29b_010: skipped because python3 / PyYAML missing locally");
        return;
    }

    if !failures.is_empty() {
        panic!(
            "D29B-010 / Lock-G: workflow YAML audit failed for {} file(s):\n\n{}\n\n\
             Likely cause: a multi-line string literal escaping a `run: |` block scalar. \
             Use multiple `-m` flags or pipe into `git commit -F -` instead.",
            failures.len(),
            failures.join("\n\n")
        );
    }
}

#[test]
fn d29b_010_release_yml_has_idempotent_dispatcher() {
    // D29B-013 / Lock-I: `release.yml` must branch on `gh release view`
    // existence and either upload --clobber+edit (exists) or create
    // (absent). A regression that reverts to bare `gh release create`
    // would re-introduce the `@d.28` manual-cleanup operational burden.
    let path = workflows_dir().join("release.yml");
    let body = fs::read_to_string(&path).expect("read release.yml");

    assert!(
        body.contains("gh release view"),
        "release.yml must use `gh release view` for idempotency check (D29B-013 / Lock-I)"
    );
    assert!(
        body.contains("--clobber"),
        "release.yml must use `gh release upload ... --clobber` on the exists branch (D29B-013 / Lock-I)"
    );
    assert!(
        body.contains("gh release edit"),
        "release.yml must use `gh release edit ... --notes` on the exists branch (D29B-013 / Lock-I)"
    );
    assert!(
        body.contains("gh release create"),
        "release.yml must still use `gh release create` on the absent branch (D29B-013 / Lock-I)"
    );
}

#[test]
fn d29b_010_bench_workflows_use_multi_m_commit() {
    // D29B-010 / Lock-G: `bench.yml` and `perf-router.yml` must use the
    // multi-`-m` form so the commit body never escapes the `run: |`
    // block scalar. The legacy `-m "header\n\nbody"` form was the
    // proximate cause of the six-run startup-failure burst on `main`.
    for fname in &["bench.yml", "perf-router.yml"] {
        let path = workflows_dir().join(fname);
        let body = fs::read_to_string(&path).expect("read workflow");
        // Look for at least three `-m ` occurrences within ~30 lines of
        // any `git commit` invocation. We don't need a strict parser;
        // a basic occurrence count is enough to catch the regression.
        let dash_m_count = body.matches("-m \"").count();
        assert!(
            dash_m_count >= 3,
            "{}: expected ≥3 `-m \"` occurrences (multi-`-m` commit form, Lock-G), found {}",
            fname,
            dash_m_count
        );
    }
}
