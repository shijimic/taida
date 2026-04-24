//! C26B-024 (@c.26, wepsilon Round 10 Step 4, 2026-04-24): router-bench
//! 3-backend parity.
//!
//! This fixture exercises the full router hot path (splitPath +
//! matchSegs + buildRoutes + benchLoop) at a scale that triggers
//! the Step 4 runtime tiers (Tier-1 freelists, Tier-2 bump arena,
//! mincore fast-paths, arena-aware list_push migration). The rework
//! is a pure performance optimisation with NO semantic change; this
//! test asserts that Interpreter / JS / Native produce bit-for-bit
//! identical output under the workload.
//!
//! Benchmark on @c.26 wepsilon Round 10 (Linux x86_64, gcc):
//!
//! | Backend | real  | sys   | sys/real | vs JS |
//! |---------|------:|------:|---------:|------:|
//! | Native  | 0.34s | 0.03s | 9%       | 2.0x  |
//! | JS      | 0.17s | 0.01s | 6%       | 1.0x  |
//!
//! Both acceptance criteria from `.dev/C26_BLOCKERS.md::C26B-024` Step 4
//! (`Native / JS <= 2x`, `sys/real <= 30%`) are satisfied; prior
//! @c.25.rc7 / @c.26 wT Round 8 measurements had Native/JS at 12.1x
//! and sys/real at 81%.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_fixture(tag: &str, source: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "c26b_024_router_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("mkdir tmpdir");
    let src = dir.join("fixture.td");
    fs::write(&src, source).expect("write fixture");
    (dir, src)
}

fn run_interp(src: &PathBuf) -> String {
    let out = Command::new(taida_bin())
        .arg(src)
        .output()
        .expect("run interp");
    assert!(
        out.status.success(),
        "interp failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn run_js(src: &Path, dir: &Path) -> Option<String> {
    if !node_available() {
        eprintln!("node unavailable; skipping JS leg");
        return None;
    }
    let js = dir.join("out.mjs");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(src)
        .arg("-o")
        .arg(&js)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "js build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node").arg(&js).output().expect("run js");
    assert!(
        run.status.success(),
        "js run failed: stderr={}",
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).trim().to_string())
}

fn run_native(src: &Path, dir: &Path) -> Option<String> {
    if !cc_available() {
        eprintln!("cc unavailable; skipping native leg");
        return None;
    }
    let bin = dir.join("out.bin");
    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(src)
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("build native");
    assert!(
        build.status.success(),
        "native build failed: stderr={}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin).output().expect("run native");
    assert!(
        run.status.success(),
        "native run failed (status={}): stderr={}",
        run.status,
        String::from_utf8_lossy(&run.stderr)
    );
    Some(String::from_utf8_lossy(&run.stdout).trim().to_string())
}

fn parity_assert(tag: &str, source: &str, expected: &str) {
    let (dir, src) = write_fixture(tag, source);
    let interp = run_interp(&src);
    assert_eq!(interp, expected, "interp mismatch ({tag})");
    if let Some(js) = run_js(&src, &dir) {
        assert_eq!(js, expected, "js mismatch ({tag})");
    }
    if let Some(native) = run_native(&src, &dir) {
        assert_eq!(native, expected, "native mismatch ({tag})");
    }
    let _ = fs::remove_dir_all(&dir);
}

/// Mini router scenario hitting splitPath / matchSegs / buildRoutes at
/// a scale large enough to force at least one arena chunk allocation
/// and exercise the list-push arena migration path.
#[test]
fn c26b_024_router_bench_smoke_parity() {
    let source = r#"
splitPath p: Str =
  | p == "" |> @[]
  | p == "/" |> @[""]
  | _ |>
    stripped <= | p.startsWith("/") |> Slice[p](start <= 1, end <= p.length()) | _ |> p
    stripped.split("/")
=> :@[Str]

matchSegs pat: @[Str] pth: @[Str] i: Int n: Int =
  | i >= n |> true
  | _ |>
    pat.get(i) ]=> pseg
    pth.get(i) ]=> sseg
    | pseg.startsWith(":") |> matchSegs(pat, pth, i + 1, n)
    | pseg == sseg |> matchSegs(pat, pth, i + 1, n)
    | _ |> false
=> :Bool

matchPattern pattern: Str path: Str =
  patSegs <= splitPath(pattern)
  pthSegs <= splitPath(path)
  | patSegs.length() != pthSegs.length() |> false
  | _ |> matchSegs(patSegs, pthSegs, 0, patSegs.length())
=> :Bool

matchRoutes routes: @[@(method: Str, pattern: Str)] method: Str path: Str i: Int n: Int =
  | i >= n |> -1
  | _ |>
    routes.get(i) ]=> r
    | r.method != method |> matchRoutes(routes, method, path, i + 1, n)
    | _ |>
      | matchPattern(r.pattern, path) |> i
      | _ |> matchRoutes(routes, method, path, i + 1, n)
=> :Int

matchRoute routes: @[@(method: Str, pattern: Str)] method: Str path: Str =
  matchRoutes(routes, method, path, 0, routes.length())
=> :Int

buildRoutesIter acc: @[@(method: Str, pattern: Str)] i: Int n: Int =
  | i >= n |> acc
  | _ |>
    newRoute <= @(method <= "GET", pattern <= "/route/" + i.toString())
    buildRoutesIter(Append[acc, newRoute](), i + 1, n)
=> :@[@(method: Str, pattern: Str)]

buildRoutes n: Int = buildRoutesIter(@[], 0, n) => :@[@(method: Str, pattern: Str)]

benchLoop routes: @[@(method: Str, pattern: Str)] method: Str path: Str i: Int m: Int acc: Int =
  | i >= m |> acc
  | _ |>
    r <= matchRoute(routes, method, path)
    benchLoop(routes, method, path, i + 1, m, acc + r)
=> :Int

routes <= buildRoutes(20)
best <= benchLoop(routes, "GET", "/route/0", 0, 50, 0)
worst <= benchLoop(routes, "GET", "/route/19", 0, 50, 0)
miss <= benchLoop(routes, "GET", "/nomatch", 0, 50, 0)
stdout(best.toString())
stdout(worst.toString())
stdout(miss.toString())
"#;
    // best:  hit index 0  50 times -> sum = 0
    // worst: hit index 19 50 times -> sum = 950
    // miss:  no match -> 50 * -1 = -50
    parity_assert("router_bench_smoke", source, "0\n950\n-50");
}

/// List push on an arena-backed list must correctly migrate to malloc
/// when the list grows past its initial cap=16. Without the migration
/// path, realloc on an arena-backed slab is UB and crashes on glibc.
#[test]
fn c26b_024_list_push_arena_migration_parity() {
    let source = r#"
push acc: @[Int] i: Int n: Int =
  | i >= n |> acc
  | _ |> push(Append[acc, i](), i + 1, n)
=> :@[Int]

xs <= push(@[], 0, 50)
sum acc: Int i: Int n: Int items: @[Int] =
  | i >= n |> acc
  | _ |>
    items.get(i) ]=> v
    sum(acc + v, i + 1, n, items)
=> :Int
total <= sum(0, 0, xs.length(), xs)
stdout(total.toString())
stdout(xs.length().toString())
"#;
    // Sum of 0..50 = 1225; length = 50
    parity_assert("list_push_arena_migration", source, "1225\n50");
}

/// Small-string churn below the 32-byte bucket. Ensures the bucket-0
/// freelist (or arena fallback) yields correct content on reuse.
#[test]
fn c26b_024_small_string_churn_parity() {
    let source = r#"
churn i: Int n: Int acc: Int =
  | i >= n |> acc
  | _ |>
    s <= "k=" + i.toString()
    v <= | s.startsWith("k=") |> 1 | _ |> 0
    churn(i + 1, n, acc + v)
=> :Int

total <= churn(0, 200, 0)
stdout(total.toString())
"#;
    // All 200 iterations hit startsWith("k=") -> 200
    parity_assert("small_string_churn", source, "200");
}

// ---------------------------------------------------------------------------
// C26B-024 Step 1 — CI perf regression gate (wη Round 11, 2026-04-24)
// ---------------------------------------------------------------------------
//
// The three parity tests above lock bit-for-bit output across Interpreter /
// JS / Native and cover ~50-route workloads. The perf-gate test below
// measures the ratio `Native / JS` and `sys / real` on the full
// `examples/quality/c26_native_router_bench/router.td` (N=50 + N=200 × 6
// scenarios × M=500 iter) and hard-fails when the wε Round 10 acceptance
// invariants regress.
//
// Gate invariants (pinned by `.github/bench-baselines/perf_router.json`):
//   - `Native / JS <= native_js_ratio_max` (CI 2C default: 3.0; local 16T
//     wε Round 10 measured 2.0. Headroom = 1.5× for CI noise + 2-core
//     allocator contention).
//   - `sys / real <= sys_real_ratio_max` for Native (CI 2C default: 0.40;
//     local 16T wε Round 10 measured 0.09. Headroom ≈ 4× for CI kernel
//     overhead).
//
// Execution mode: this test is `#[ignore]`-gated so the existing CI unit
// test job (`cargo nextest run --profile ci`) does not run it. The
// dedicated `perf-router.yml` workflow opts in via `--ignored
// --test-threads=1` and the environment variables documented below.
//
// Environment variables:
//   - `TAIDA_PERF_ROUTER_ENABLED=1` — mandatory opt-in. Without it, the
//     test prints a skip message and returns. This prevents accidental
//     runs by `cargo test -- --ignored` from local developers.
//   - `TAIDA_PERF_ROUTER_STRICT=1` — when set, threshold violations
//     `panic!` and fail the test. When unset, violations print
//     `WARN:` lines and the test passes (baseline sampling mode).
//   - `TAIDA_PERF_ROUTER_RUNS` (default: 3) — number of timing runs per
//     backend. Median of the runs is used.
//   - `TAIDA_PERF_ROUTER_NATIVE_JS_MAX` (default: 3.0) — hard-fail
//     threshold for `median_native_real / median_js_real`.
//   - `TAIDA_PERF_ROUTER_SYS_REAL_MAX` (default: 0.40) — hard-fail
//     threshold for `median_native_sys / median_native_real`.
//
// Emitted lines (parsed by the workflow):
//   PERF_ROUTER_NATIVE_JS_RATIO=<f64>
//   PERF_ROUTER_NATIVE_SYS_RATIO=<f64>
//   PERF_ROUTER_NATIVE_REAL_MEDIAN_SEC=<f64>
//   PERF_ROUTER_JS_REAL_MEDIAN_SEC=<f64>
//
// D27 escalation checklist (all NO):
//   - surface change?           NO (adds a gated test only)
//   - error string change?      NO (new emit lines, not existing errors)
//   - existing assertion edit?  NO (three parity tests above untouched)

fn env_flag(name: &str) -> bool {
    std::env::var(name).ok().as_deref() == Some("1")
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(default)
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default)
}

fn perf_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("quality")
        .join("c26_native_router_bench")
        .join("router.td")
}

/// Time a binary invocation and return (real_sec, sys_sec). Uses GNU
/// `/usr/bin/time -f "%e %S"` which is available on ubuntu-latest
/// runners. Falls back to wall-clock only (sys=0.0) when absent.
fn time_command(cmd: &mut Command) -> Option<(f64, f64)> {
    use std::time::Instant;

    let gnu_time = Path::new("/usr/bin/time");
    if gnu_time.exists() {
        // Rebuild a `/usr/bin/time -f "%e %S" <program> <args>` invocation.
        // We cannot reuse the passed-in Command directly because it already
        // carries the program + args, so we inspect those via get_program /
        // get_args and rebuild.
        let program = cmd.get_program().to_os_string();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_os_string()).collect();
        let envs: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_os_string(), v.map(|vv| vv.to_os_string())))
            .collect();
        let current_dir = cmd.get_current_dir().map(|p| p.to_path_buf());

        let mut timed = Command::new(gnu_time);
        timed.arg("-f").arg("%e %S").arg(&program);
        for a in &args {
            timed.arg(a);
        }
        for (k, v) in &envs {
            match v {
                Some(val) => {
                    timed.env(k, val);
                }
                None => {
                    timed.env_remove(k);
                }
            }
        }
        if let Some(d) = current_dir {
            timed.current_dir(d);
        }

        let out = timed.output().ok()?;
        // `/usr/bin/time` writes its summary to stderr.
        let stderr = String::from_utf8_lossy(&out.stderr);
        // Last non-empty line should be "<real> <sys>".
        let last = stderr.lines().rev().find(|l| !l.trim().is_empty())?;
        let mut parts = last.split_whitespace();
        let real: f64 = parts.next()?.parse().ok()?;
        let sys: f64 = parts.next()?.parse().ok()?;
        if !out.status.success() {
            return None;
        }
        Some((real, sys))
    } else {
        let start = Instant::now();
        let status = cmd.status().ok()?;
        let real = start.elapsed().as_secs_f64();
        if !status.success() {
            return None;
        }
        Some((real, 0.0))
    }
}

fn median_f64(mut xs: Vec<f64>) -> f64 {
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = xs.len();
    if n == 0 {
        return f64::NAN;
    }
    if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    }
}

#[test]
#[ignore = "C26B-024 Step 1 perf gate: opt-in via TAIDA_PERF_ROUTER_ENABLED=1 (CI perf-router workflow)"]
fn c26b_024_router_perf_gate() {
    if !env_flag("TAIDA_PERF_ROUTER_ENABLED") {
        eprintln!(
            "perf-router: TAIDA_PERF_ROUTER_ENABLED!=1; skipping (opt-in only for CI perf-router workflow)"
        );
        return;
    }

    let strict = env_flag("TAIDA_PERF_ROUTER_STRICT");
    let runs = env_usize("TAIDA_PERF_ROUTER_RUNS", 3).max(1);
    let native_js_max = env_f64("TAIDA_PERF_ROUTER_NATIVE_JS_MAX", 3.0);
    let sys_real_max = env_f64("TAIDA_PERF_ROUTER_SYS_REAL_MAX", 0.40);

    if !cc_available() {
        eprintln!("perf-router: cc unavailable; skipping");
        return;
    }
    if !node_available() {
        eprintln!("perf-router: node unavailable; skipping");
        return;
    }

    let fixture = perf_fixture_path();
    assert!(
        fixture.exists(),
        "perf-router fixture missing: {}",
        fixture.display()
    );

    // Build the JS + Native artifacts once; reuse them across timing runs.
    let work = std::env::temp_dir().join(format!(
        "c26b_024_perf_router_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&work).expect("mkdir work");
    let js_out = work.join("router.mjs");
    let native_out = work.join("router.bin");

    let js_build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(&fixture)
        .arg("-o")
        .arg(&js_out)
        .output()
        .expect("js build");
    assert!(
        js_build.status.success(),
        "perf-router js build failed: stderr={}",
        String::from_utf8_lossy(&js_build.stderr)
    );

    let native_build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(&fixture)
        .arg("-o")
        .arg(&native_out)
        .output()
        .expect("native build");
    assert!(
        native_build.status.success(),
        "perf-router native build failed: stderr={}",
        String::from_utf8_lossy(&native_build.stderr)
    );

    // Timed runs.
    let mut js_real = Vec::with_capacity(runs);
    let mut native_real = Vec::with_capacity(runs);
    let mut native_sys = Vec::with_capacity(runs);

    for i in 0..runs {
        let mut js_cmd = Command::new("node");
        js_cmd.arg(&js_out);
        let (jr, _js_sys) = time_command(&mut js_cmd)
            .unwrap_or_else(|| panic!("perf-router js timing failed at run {i}"));
        js_real.push(jr);

        let mut nat_cmd = Command::new(&native_out);
        let (nr, ns) = time_command(&mut nat_cmd)
            .unwrap_or_else(|| panic!("perf-router native timing failed at run {i}"));
        native_real.push(nr);
        native_sys.push(ns);
    }

    let js_median = median_f64(js_real.clone());
    let nat_median = median_f64(native_real.clone());
    let nat_sys_median = median_f64(native_sys.clone());

    // Guard against zero-time runs from CI clocks with coarse resolution.
    // Treat them as unusable samples and skip strict enforcement.
    let usable = js_median > 0.0 && nat_median > 0.0;

    let native_js_ratio = if js_median > 0.0 {
        nat_median / js_median
    } else {
        f64::INFINITY
    };
    let sys_ratio = if nat_median > 0.0 {
        nat_sys_median / nat_median
    } else {
        f64::INFINITY
    };

    println!("PERF_ROUTER_NATIVE_REAL_MEDIAN_SEC={nat_median:.6}");
    println!("PERF_ROUTER_JS_REAL_MEDIAN_SEC={js_median:.6}");
    println!("PERF_ROUTER_NATIVE_JS_RATIO={native_js_ratio:.6}");
    println!("PERF_ROUTER_NATIVE_SYS_RATIO={sys_ratio:.6}");
    println!("PERF_ROUTER_RUNS={runs}");
    println!("PERF_ROUTER_NATIVE_JS_MAX={native_js_max:.6}");
    println!("PERF_ROUTER_SYS_REAL_MAX={sys_real_max:.6}");

    let mut violations: Vec<String> = Vec::new();
    if usable {
        if native_js_ratio > native_js_max {
            violations.push(format!(
                "Native/JS ratio {native_js_ratio:.3} exceeds max {native_js_max:.3}"
            ));
        }
        if sys_ratio > sys_real_max {
            violations.push(format!(
                "Native sys/real ratio {sys_ratio:.3} exceeds max {sys_real_max:.3}"
            ));
        }
    } else {
        eprintln!(
            "perf-router: WARN: median times too small to gate reliably (js={js_median} native={nat_median}); skipping strict check"
        );
    }

    // Best-effort cleanup.
    let _ = fs::remove_dir_all(&work);

    if !violations.is_empty() {
        let msg = violations.join("; ");
        if strict {
            panic!("perf-router regression: {msg}");
        } else {
            eprintln!("perf-router: WARN: {msg} (non-strict mode, not failing)");
        }
    }
}
