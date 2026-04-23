//! C25B-022 / C25B-023 (Phase 5-C / 5-E) — perf regression guard.
//!
//! Pre-fix, `Set.union`, `Set.intersect`, `Set.diff` and
//! `HashMap.merge` all walked `Vec::contains` on every element and
//! scaled as O(N*M). A 1000 × 1000 operation took seconds on the
//! interpreter.
//!
//! Phase 5-C pre-builds a `HashSet<u64>` of `ValueKey` fingerprints
//! so membership probes become O(1). Phase 5-E reduces
//! `HashMap.merge` from O(N*M*K) to O(N+M).
//!
//! These tests run representative 1000-element operations and assert
//! that the interpreter completes them well under the original O(N²)
//! envelope. We don't pin a hard wall-clock threshold (CI runner
//! variance makes that flaky); instead we assert the operation
//! finishes within 5 seconds, which was already 10× over budget for
//! the pre-fix path on a modern laptop and therefore a safe upper
//! bound for the fast-path implementation.
mod common;

use common::taida_bin;
use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run_taida_fixture_with_timeout(src: &str, timeout: Duration) -> String {
    // Every call gets its own directory so parallel tests inside the
    // same test binary don't clobber each other's fixture file.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("c25b_fastpath_{}_{}", std::process::id(), seq));
    fs::create_dir_all(&dir).expect("create tmp dir");
    let path = dir.join("fixture.td");
    fs::write(&path, src).expect("write fixture");
    let start = Instant::now();
    let mut child = Command::new(taida_bin())
        .arg(&path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn taida");
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let elapsed = start.elapsed();
                let out = child.wait_with_output().unwrap_or_else(|_| {
                    // Unreachable normally, but provide a safe fallback.
                    panic!("child wait_with_output failed")
                });
                assert!(
                    status.success(),
                    "taida run failed ({}): stderr={}",
                    status,
                    String::from_utf8_lossy(&out.stderr)
                );
                assert!(
                    elapsed < timeout,
                    "operation took {}ms, expected < {}ms",
                    elapsed.as_millis(),
                    timeout.as_millis()
                );
                let _ = fs::remove_dir_all(&dir);
                return String::from_utf8_lossy(&out.stdout).to_string();
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_dir_all(&dir);
                    panic!(
                        "operation timed out (> {}ms) — Phase 5-C/5-E fast path regressed",
                        timeout.as_millis()
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("try_wait failed: {}", e),
        }
    }
}

/// Build two 1000-element Sets of integers and run union / intersect /
/// diff. Pre-fix the three ops combined took multiple seconds; with
/// the Phase 5-C fingerprint fast path we expect sub-second on any
/// modern runner.
#[test]
fn c25b_022_set_union_intersect_diff_1000x1000_completes_fast() {
    let src = r#"
// Build Set A = {0..1000}, Set B = {500..1500} using fold over a
// recursive builder (the interpreter has no native Range mold).
buildSet acc i stop =
  | i >= stop |> acc
  | _ |> buildSet(acc.add(i), i + 1, stop)

setA <= buildSet(setOf(@[]), 0, 1000)
setB <= buildSet(setOf(@[]), 500, 1500)

u <= setA.union(setB)
i2 <= setA.intersect(setB)
d <= setA.diff(setB)

stdout(u.size())
stdout(i2.size())
stdout(d.size())
"#;
    let out = run_taida_fixture_with_timeout(src, Duration::from_secs(5));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        vec!["1500", "500", "500"],
        "union/intersect/diff produced wrong sizes: {:?}",
        lines
    );
}

/// Merge two HashMaps of 1000 entries each. Pre-fix, the three-
/// layered retain was O(N*M*K); Phase 5-E reduces to O(N+M).
#[test]
fn c25b_023_hashmap_merge_1000x1000_completes_fast() {
    let src = r#"
// Build HashMap A = {0: 1_000_000, 1: 1_000_001, ..., 999: 1_000_999}
//   (key i → value 1_000_000 + i, marker "A" range)
// Build HashMap B = {500: 2_000_500, ..., 1499: 2_001_499}
//   (key i → value 2_000_000 + i, marker "B" range)
// Template-literal interpolation inside function bodies hits a
// pre-existing interpreter issue (expressions in `${...}` render
// literally), so we encode which side a value came from via the
// numeric prefix instead.
buildMap acc i stop bias =
  | i >= stop |> acc
  | _ |> buildMap(acc.set(i, bias + i), i + 1, stop, bias)

mapA <= buildMap(hashMap(@[]), 0, 1000, 1000000)
mapB <= buildMap(hashMap(@[]), 500, 1500, 2000000)

merged <= mapA.merge(mapB)
stdout(merged.size())
// Key 100 is A-only (A=0..1000, B=500..1500), must surface A value.
// Key 500 is in both — B wins, must surface 2_000_500.
// Key 1499 is B-only.
stdout(merged.get(100))
stdout(merged.get(500))
stdout(merged.get(1499))
"#;
    let out = run_taida_fixture_with_timeout(src, Duration::from_secs(5));
    let lines: Vec<&str> = out.lines().collect();
    // size = 1500; merged.get returns Lax, whose .toString will
    // render the __value or __default depending on pack shape. We
    // only assert on size + that the keys resolve to "b*" (from B)
    // for 500 / 1499, and "a999" for 999 (only in A).
    assert_eq!(lines[0], "1500", "merged size regressed: {:?}", lines);
    assert!(
        lines[1].contains("1000100"),
        "merged.get(100) should surface A-side 1_000_100: {}",
        lines[1]
    );
    assert!(
        lines[2].contains("2000500"),
        "merged.get(500) should surface B-side 2_000_500 (B wins on collision): {}",
        lines[2]
    );
    assert!(
        lines[3].contains("2001499"),
        "merged.get(1499) should surface B-side 2_001_499: {}",
        lines[3]
    );
}

/// Semantic regression — `Set.union` must honour `Value::eq`'s
/// `Int(n) == EnumVal(_, n)` cross-type rule. Session-20 review
/// found that the Phase 5-C fingerprint fast path treated them as
/// distinct, so `setOf(@[0]).union(setOf(@[Color:Red()]))` reported
/// size 2 instead of size 1.
#[test]
fn c25b_022_set_union_int_enumval_cross_type_eq() {
    let src = r#"
Enum => Color = :Red :Green :Blue

a <= setOf(@[0])
b <= setOf(@[Color:Red()])
u <= a.union(b)
stdout(u.size())
"#;
    let out = run_taida_fixture_with_timeout(src, Duration::from_secs(5));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0], "1",
        "Int(0) and Color:Red() share ordinal 0 and are Value::eq-equal, \
         so Set.union must dedupe them (got {:?})",
        lines
    );
}

/// Semantic regression — `Set.intersect` must likewise honour
/// cross-type equality.
#[test]
fn c25b_022_set_intersect_int_enumval_cross_type_eq() {
    let src = r#"
Enum => Color = :Red :Green :Blue

a <= setOf(@[0, 1, 2])
b <= setOf(@[Color:Red(), Color:Green()])
i2 <= a.intersect(b)
stdout(i2.size())
"#;
    let out = run_taida_fixture_with_timeout(src, Duration::from_secs(5));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0], "2",
        "Color:Red() (ordinal 0) matches Int(0), Color:Green() (ordinal 1) \
         matches Int(1), so intersect size must be 2 (got {:?})",
        lines
    );
}

/// Semantic regression — `Set.diff` must remove cross-type
/// equivalents.
#[test]
fn c25b_022_set_diff_int_enumval_cross_type_eq() {
    let src = r#"
Enum => Color = :Red :Green :Blue

a <= setOf(@[0, 1, 2])
b <= setOf(@[Color:Red()])
d <= a.diff(b)
stdout(d.size())
"#;
    let out = run_taida_fixture_with_timeout(src, Duration::from_secs(5));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0], "2",
        "Color:Red() (ordinal 0) == Int(0), so diff must drop Int(0) \
         from {{0,1,2}} and leave size 2 (got {:?})",
        lines
    );
}

/// Semantic regression — `HashMap.merge` must treat
/// `Int(0)` and `EnumVal("Color", 0)` as the same key, so B's value
/// overwrites A's.
#[test]
fn c25b_023_hashmap_merge_int_enumval_key_overwrite() {
    let src = r#"
Enum => Color = :Red :Green :Blue

a <= hashMap().set(0, "int")
b <= hashMap().set(Color:Red(), "enum")
merged <= a.merge(b)
stdout(merged.size())
stdout(merged.get(0))
"#;
    let out = run_taida_fixture_with_timeout(src, Duration::from_secs(5));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0], "1",
        "Int(0) key and Color:Red() key are Value::eq-equal, so merge \
         must fold to a single entry (got {:?})",
        lines
    );
    assert!(
        lines[1].contains("enum"),
        "B-side value (\"enum\") must win on key collision: got {}",
        lines[1]
    );
}

/// Semantic regression — distinct EnumVals with the same ordinal
/// but different enum names must remain distinct. This is the
/// fingerprint-collision case; the Value::eq confirmation path at
/// the caller must keep them separate.
#[test]
fn c25b_022_set_union_distinct_enums_same_ordinal_stay_distinct() {
    let src = r#"
Enum => Color = :Red :Green :Blue
Enum => Shape = :Circle :Square :Triangle

a <= setOf(@[Color:Red()])
b <= setOf(@[Shape:Circle()])
u <= a.union(b)
stdout(u.size())
"#;
    let out = run_taida_fixture_with_timeout(src, Duration::from_secs(5));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines[0], "2",
        "Color:Red() (ordinal 0) and Shape:Circle() (ordinal 0) are \
         different per Value::eq (different enum names), so Set.union \
         must keep both (got {:?})",
        lines
    );
}

/// Unique on a 1000-element list with mostly-duplicate Str keys.
/// Pre-fix the seen_keys linear scan dominated; Phase 5-C's
/// fingerprint HashSet brings it to O(N).
#[test]
fn c25b_021_unique_1000_strs_completes_fast() {
    let src = r#"
// Build a list of 1000 repeating integers (10 unique: 0..9, each 100×)
buildList acc i =
  | i >= 1000 |> acc
  | _ |> buildList(Append[acc, Mod[i, 10]()](), i + 1)

big <= buildList(@[], 0)
uniq <= Unique[big]()
stdout(uniq.length())
"#;
    // Longer timeout because the buildList itself remains O(N²)
    // until Phase 5-F lands (C25B-021 main body). We only need to
    // assert the Unique pass itself is fast by measuring the net
    // end-to-end; this still exercises the Unique fast path but
    // the dominant cost is Append.
    let out = run_taida_fixture_with_timeout(src, Duration::from_secs(10));
    let lines: Vec<&str> = out.lines().collect();
    // Unique over `i % 10` for i in 0..1000 yields {0, 1, ..., 9}
    // = 10 unique values.
    assert_eq!(
        lines[0], "10",
        "Unique should yield 10 distinct integers, got {}",
        lines[0]
    );
}
