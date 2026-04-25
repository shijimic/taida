// D28B-013 invariant test (Round 2 wH).
//
// Pins the structural shape of the four perf hard-fail gates so a
// workflow-side regression (e.g. `continue-on-error: true` re-introduced,
// `--tolerance-pct` lowered, `min_samples` changed, threshold
// percentages reduced) is caught at the test layer independently of
// CI itself. The test runs as part of `cargo test --release` and
// reads the workflow / script / baseline files via the repo root
// resolved from `CARGO_MANIFEST_DIR`.
//
// Acceptance pinned here:
//   - bench.yml has no `continue-on-error: true` (D28B-005).
//   - bench.yml runs the throughput regression gate via
//     scripts/bench/compare_baseline.py with tolerance=10.0 + min-samples=30.
//   - bench.yml runs the peak-RSS regression gate via
//     scripts/bench/compare_baseline.py with tolerance=10.0 + min-samples=30
//     against scripts/perf/peak_rss_baseline.json (D28B-013 #2).
//   - coverage.yml has no `continue-on-error: true` and threshold
//     gate `interpreter` line >= 80%, branch >= 70% (D28B-013 #3).
//   - memory.yml runs valgrind definitely-lost hard-fail
//     (`--errors-for-leak-kinds=definite --error-exitcode=1`)
//     (D28B-013 #1).
//   - scripts/perf/peak_rss_baseline.json has the same schema_version,
//     min_samples_required, tolerance_pct as the throughput baseline
//     (state-machine parity).
//   - examples/quality/d28_perf_smoke/ has at least 3 fixtures (one
//     per axis: arith / list / string).

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_to_string(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("D28B-013: failed to read {}: {e}", path.display()))
}

#[test]
fn bench_workflow_has_no_continue_on_error() {
    let path = repo_root().join(".github/workflows/bench.yml");
    let content = read_to_string(&path);
    // The literal `continue-on-error: true` inside any active job is
    // forbidden by D28B-005 hard-fail acceptance. We allow the
    // documentation comment but not a YAML-level setting.
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        assert!(
            !trimmed.contains("continue-on-error: true"),
            "D28B-005: bench.yml line {} re-introduces `continue-on-error: true`: {line}",
            idx + 1
        );
    }
}

#[test]
fn bench_workflow_invokes_throughput_gate_with_tolerance_and_min_samples() {
    let path = repo_root().join(".github/workflows/bench.yml");
    let content = read_to_string(&path);
    assert!(
        content.contains("scripts/bench/compare_baseline.py"),
        "D28B-005: bench.yml must invoke compare_baseline.py"
    );
    assert!(
        content.contains(".github/bench-baselines/perf_baseline.json"),
        "D28B-005: bench.yml must reference the throughput baseline JSON"
    );
    assert!(
        content.contains("--tolerance-pct 10.0"),
        "D28B-005: throughput gate tolerance must be 10.0 (regression budget)"
    );
    assert!(
        content.contains("--min-samples 30"),
        "D28B-005: throughput gate min-samples must be 30 (EWMA window)"
    );
}

#[test]
fn bench_workflow_invokes_peak_rss_gate_with_tolerance_and_min_samples() {
    let path = repo_root().join(".github/workflows/bench.yml");
    let content = read_to_string(&path);
    assert!(
        content.contains("scripts/perf/peak_rss_baseline.json"),
        "D28B-013 #2: bench.yml must reference the peak-RSS baseline JSON"
    );
    assert!(
        content.contains("scripts/perf/measure_peak_rss.sh"),
        "D28B-013 #2: bench.yml must invoke measure_peak_rss.sh"
    );
    // Both tolerance and min-samples appear twice in bench.yml (once
    // per gate). We assert the literals exist; the per-gate binding
    // is enforced by the surrounding context being a `compare_baseline.py`
    // invocation against the peak_rss_baseline.json file.
    let peak_rss_block = content
        .split("peak-RSS baseline")
        .nth(1)
        .expect("D28B-013 #2: bench.yml must contain a `peak-RSS baseline` step header");
    assert!(
        peak_rss_block.contains("--tolerance-pct 10.0"),
        "D28B-013 #2: peak-RSS gate tolerance must be 10.0"
    );
    assert!(
        peak_rss_block.contains("--min-samples 30"),
        "D28B-013 #2: peak-RSS gate min-samples must be 30"
    );
}

#[test]
fn coverage_workflow_has_no_continue_on_error() {
    let path = repo_root().join(".github/workflows/coverage.yml");
    let content = read_to_string(&path);
    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        assert!(
            !trimmed.contains("continue-on-error: true"),
            "D28B-013 #3: coverage.yml line {} re-introduces `continue-on-error: true`: {line}",
            idx + 1
        );
    }
}

#[test]
fn coverage_workflow_pins_interpreter_thresholds() {
    let path = repo_root().join(".github/workflows/coverage.yml");
    let content = read_to_string(&path);
    // The threshold dictionary in the embedded Python uses
    // `"interpreter": 80.0` / `"interpreter": 70.0`. We pin both
    // literals so an accidental relaxation surfaces here.
    assert!(
        content.contains("\"interpreter\": 80.0"),
        "D28B-013 #3: coverage.yml must pin `src/interpreter/` line >= 80.0"
    );
    assert!(
        content.contains("\"interpreter\": 70.0"),
        "D28B-013 #3: coverage.yml must pin `src/interpreter/` branch >= 70.0"
    );
    // The error sentinel emitted on threshold violation must match
    // the convention used in scripts/perf/gate_summary.md.
    assert!(
        content.contains("D28B-013 coverage gate FAILED"),
        "D28B-013 #3: coverage.yml must emit a recognisable failure sentinel"
    );
}

#[test]
fn memory_workflow_runs_valgrind_definite_hard_fail() {
    let memory_yml = repo_root().join(".github/workflows/memory.yml");
    let valgrind_sh = repo_root().join("scripts/mem/run_valgrind_smoke.sh");
    let memory_content = read_to_string(&memory_yml);
    let valgrind_content = read_to_string(&valgrind_sh);
    assert!(
        memory_content.contains("scripts/mem/run_valgrind_smoke.sh"),
        "D28B-013 #1: memory.yml must invoke run_valgrind_smoke.sh"
    );
    assert!(
        valgrind_content.contains("--errors-for-leak-kinds=definite"),
        "D28B-013 #1: valgrind smoke must filter on definite leak kinds"
    );
    assert!(
        valgrind_content.contains("--error-exitcode=1"),
        "D28B-013 #1: valgrind smoke must exit non-zero on definite leak"
    );
}

#[test]
fn peak_rss_baseline_schema_matches_throughput_baseline() {
    let throughput = repo_root().join(".github/bench-baselines/perf_baseline.json");
    let peak_rss = repo_root().join("scripts/perf/peak_rss_baseline.json");
    let t: serde_json::Value =
        serde_json::from_str(&read_to_string(&throughput)).expect("perf_baseline.json must parse");
    let r: serde_json::Value = serde_json::from_str(&read_to_string(&peak_rss))
        .expect("peak_rss_baseline.json must parse");
    assert_eq!(
        t["schema_version"], r["schema_version"],
        "D28B-013 #2: peak_rss_baseline.json schema_version must match throughput baseline"
    );
    assert_eq!(
        t["min_samples_required"], r["min_samples_required"],
        "D28B-013 #2: peak_rss_baseline.json min_samples_required must match throughput baseline"
    );
    assert_eq!(
        t["tolerance_pct"], r["tolerance_pct"],
        "D28B-013 #2: peak_rss_baseline.json tolerance_pct must match throughput baseline"
    );
    let benches = r["benches"]
        .as_object()
        .expect("peak_rss_baseline.json must have a `benches` map");
    assert!(
        benches.len() >= 3,
        "D28B-013 #2: peak_rss_baseline.json must enumerate at least 3 fixtures (arith/list/string)"
    );
    for (name, entry) in benches {
        assert!(
            name.starts_with("rss_"),
            "D28B-013 #2: peak-RSS baseline keys must be prefixed with `rss_`; got `{name}`"
        );
        let obj = entry.as_object().unwrap_or_else(|| {
            panic!("peak_rss_baseline.json entry for `{name}` must be an object")
        });
        assert!(
            obj.contains_key("ns_median"),
            "peak_rss_baseline.json entry `{name}` missing `ns_median`"
        );
        assert!(
            obj.contains_key("sample_count"),
            "peak_rss_baseline.json entry `{name}` missing `sample_count`"
        );
        assert!(
            obj.contains_key("notes"),
            "peak_rss_baseline.json entry `{name}` missing `notes`"
        );
    }
}

#[test]
fn perf_smoke_fixtures_exist() {
    let dir = repo_root().join("examples/quality/d28_perf_smoke");
    let mut count = 0;
    for entry in fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("D28B-013 #2: missing fixture dir {}: {e}", dir.display()))
    {
        let entry = entry.expect("read_dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("td") {
            count += 1;
        }
    }
    assert!(
        count >= 3,
        "D28B-013 #2: examples/quality/d28_perf_smoke/ must hold at least 3 .td fixtures (arith/list/string), found {count}"
    );
}

#[test]
fn measure_peak_rss_script_is_executable() {
    let path = repo_root().join("scripts/perf/measure_peak_rss.sh");
    let content = read_to_string(&path);
    // shebang + the public CLI surface that bench.yml depends on.
    assert!(
        content.starts_with("#!/usr/bin/env bash"),
        "D28B-013 #2: measure_peak_rss.sh must use bash shebang"
    );
    assert!(
        content.contains("--check-against-baseline"),
        "D28B-013 #2: measure_peak_rss.sh must support --check-against-baseline (PR gate)"
    );
    assert!(
        content.contains("/usr/bin/time -v"),
        "D28B-013 #2: measure_peak_rss.sh must use /usr/bin/time -v for peak RSS capture"
    );
    let metadata = fs::metadata(&path).expect("script metadata");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "D28B-013 #2: scripts/perf/measure_peak_rss.sh must be executable (mode={:o})",
            mode
        );
    }
    let _ = metadata;
}

#[test]
fn stability_md_marks_throughput_and_memory_fixed_at_d28() {
    let path = repo_root().join("docs/STABILITY.md");
    let content = read_to_string(&path);
    // §5.1 throughput line FIXED at @d.X.
    assert!(
        content.contains("D28B-005")
            || content.contains("Throughput regression guard hard-fail at `@d.X`"),
        "D28B-005: STABILITY.md §5.1 throughput must reference D28B-005 / @d.X FIXED"
    );
    // §5.5 Memory line FIXED at @d.X.
    assert!(
        content.contains("D28B-013"),
        "D28B-013: STABILITY.md §5.5 Memory must reference D28B-013"
    );
}
