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
