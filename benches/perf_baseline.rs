//! C25B-004: Performance regression gate baseline (@c.25.rc7).
//!
//! This is the *scaffold* for the C25 perf gate. It establishes three axes
//! of measurement chosen in the Phase 0 design lock:
//!
//!   1. **Parser throughput**  — `parser::parser::parse(&str)` on a
//!      representative fixture set. Detects regressions in tokenizer +
//!      recursive-descent hot paths.
//!   2. **Interpreter throughput** — `interpreter::eval::eval(&str)` on the
//!      same fixtures. Detects regressions in the reference-implementation
//!      runtime (Value layout / mold dispatch / closure env lookup).
//!   3. **Cold compile** — reserved for native-codegen bench in a later
//!      subphase; deliberately left as a TODO to avoid shelling out to
//!      the `taida` binary from criterion (process-spawn noise dwarfs
//!      codegen deltas at this scale).
//!
//! The corpus is small on purpose. Criterion needs multiple samples per
//! fixture; running 50-line programs gives us stable < 200ms benches.
//! Once baselines are published and the PR bench workflow runs for
//! several weeks, we can add larger fixtures + the cold-compile axis.
//!
//! The bench is *not* invoked from `cargo test` or `cargo nextest`. It
//! runs only under `cargo bench` (locally) or the dedicated
//! `.github/workflows/bench.yml` CI job, which is `continue-on-error: true`
//! until we establish stable main-baseline numbers (C25B-004 follow-up).

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::path::PathBuf;

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// Ten representative fixtures covering the major language surface areas.
/// Keep this list small + stable — adding fixtures mid-track invalidates
/// baselines. If a fixture is removed from `examples/`, update this list
/// and publish a new baseline.
const BASELINE_FIXTURES: &[&str] = &[
    // arithmetic / control flow
    "compile_arithmetic.td",
    "compile_cond.td",
    "compile_hello.td",
    // data shapes
    "compile_buchi.td",
    "compile_hashmap_set.td",
    // function / closure / pipeline
    "compile_functions.td",
    "compile_closure.td",
    "compile_hof_molds.td",
    // numeric / tail
    "compile_c13_1_tail_bind.td",
    // boolean + stdout
    "compile_bool_stdout.td",
];

fn load_source(name: &str) -> Option<String> {
    let path = fixtures_root().join(name);
    std::fs::read_to_string(&path).ok()
}

fn bench_parser(c: &mut Criterion) {
    let mut group = c.benchmark_group("parser");
    for name in BASELINE_FIXTURES {
        let Some(source) = load_source(name) else {
            continue;
        };
        group.bench_function(*name, |b| {
            b.iter(|| {
                let (prog, errors) = taida::parser::parse(black_box(&source));
                black_box((prog, errors));
            });
        });
    }
    group.finish();
}

fn bench_interpreter(c: &mut Criterion) {
    let mut group = c.benchmark_group("interpreter");
    for name in BASELINE_FIXTURES {
        let Some(source) = load_source(name) else {
            continue;
        };
        // Skip fixtures that do not round-trip through eval (e.g. I/O-heavy
        // fixtures would require stdin redirection). For the scaffold we
        // accept that a handful of fixtures will report errors; the bench
        // is still meaningful because it measures the same code path on
        // every run.
        group.bench_function(*name, |b| {
            b.iter(|| {
                let result = taida::interpreter::eval::eval(black_box(&source));
                black_box(result.ok());
            });
        });
    }
    group.finish();
}

criterion_group!(baselines, bench_parser, bench_interpreter);
criterion_main!(baselines);
