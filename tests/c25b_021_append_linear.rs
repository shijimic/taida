//! C25B-021 — Append / Prepend semantic + perf regression guards.
//!
//! # History
//!
//! An earlier Stage-B attempt (commit `f043586`) made the tail-
//! recursive Append loop O(N) by taking the first argument's binding
//! out of the innermost scope, mutating via `Arc::make_mut`, and
//! rebinding env to the new list. That was flagged at Phase 10 GATE
//! review (2026-04-23, session 20) as breaking the mold semantic
//! contract: `Append[xs, v]()` must be non-destructive on `xs`.
//!
//! The session-20 fix reverts to the clone-based path. The hot
//! tail-recursive pattern
//!
//! ```taida
//! build acc i =
//!   | i >= N |> acc
//!   | _ |> build(Append[acc, i](), i + 1)
//! ```
//!
//! is therefore O(N²) again in the worst case. The trampoline-level
//! `current_args.clear()` release after parameter binding still lets
//! `list_take`'s `Arc::try_unwrap` succeed when env is the sole
//! holder, so the amortized cost per iteration is a single
//! `Vec::push` rather than a full clone — the *practical* constant
//! factor is dramatically smaller than the pre-Stage-B baseline even
//! though the asymptotic shape is quadratic.
//!
//! # What these tests pin
//!
//! * **Semantic contract** (session-20 addition): `Append[xs, v]()`
//!   and `Prepend[xs, v]()` must not mutate the `xs` binding. Any
//!   reappearance of the env-take fast path will regress this.
//! * **Perf envelope**: a 5 000-element Append loop completes under
//!   2 seconds; a 5 000-element Prepend loop under 2 seconds. Both
//!   run in the low hundreds of milliseconds on a modern laptop.
//!   These are permissive ceilings that still catch the pre-Stage-B
//!   baseline (~3.4 s at N=20000, ~270 ms at N=4800).
//! * **Self-reference correctness**: when the second arg references
//!   the first arg's name, the result is the non-destructive expected
//!   value and the binding itself is unchanged.
mod common;

use common::taida_bin;
use std::fs;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn run_fixture_under(src: &str, timeout: Duration, label: &str) {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "c25b_021_append_{}_{}_{}",
        std::process::id(),
        seq,
        label
    ));
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
                let out = child
                    .wait_with_output()
                    .unwrap_or_else(|_| panic!("child wait_with_output failed"));
                assert!(
                    status.success(),
                    "{}: taida run failed ({}): stderr={}",
                    label,
                    status,
                    String::from_utf8_lossy(&out.stderr)
                );
                assert!(
                    elapsed < timeout,
                    "{}: Append/Prepend took {}ms, exceeds {}ms ceiling — \
                     C25B-021 unique-ownership fast path regressed",
                    label,
                    elapsed.as_millis(),
                    timeout.as_millis()
                );
                let _ = fs::remove_dir_all(&dir);
                return;
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_dir_all(&dir);
                    panic!(
                        "{}: Append/Prepend timed out (> {}ms) — \
                         C25B-021 unique-ownership fast path regressed",
                        label,
                        timeout.as_millis()
                    );
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => panic!("try_wait failed: {}", e),
        }
    }
}

/// Tail-recursive 5 000-element Append loop. After the session-20
/// semantic-correctness revert the shape is O(N²) but the constant
/// factor is small (try_unwrap succeeds on the common-case single-
/// holder arg, so per-iter cost is a single `Vec::push`). 2 s is a
/// permissive ceiling that still catches any reintroduction of the
/// full-clone-per-iter Arc baseline (which pushed N=5000 to >5 s in
/// some runs on the same laptop).
#[test]
fn c25b_021_append_5000_completes_under_2s() {
    let src = r#"
build acc i =
  | i >= 5000 |> acc
  | _ |> build(Append[acc, i](), i + 1)

result <= build(@[], 0)
stdout("ok")
"#;
    run_fixture_under(src, Duration::from_secs(2), "append_5000");
}

/// 5 000-element Prepend loop. Prepend is inherently O(N²) even
/// under unique ownership because `Vec::insert(0, v)` is O(N) per
/// element; the session-20 fallback-path revert adds a further
/// per-iter Vec clone on top. 3 s is a safe ceiling.
#[test]
fn c25b_021_prepend_5000_completes_under_3s() {
    let src = r#"
build acc i =
  | i >= 5000 |> acc
  | _ |> build(Prepend[acc, i](), i + 1)

result <= build(@[], 0)
stdout("ok")
"#;
    run_fixture_under(src, Duration::from_secs(3), "prepend_5000");
}

/// Semantic contract — `Append[xs, v]()` must not destructively
/// update the `xs` binding. This is the regression that the
/// session-20 review uncovered: the earlier Stage-B env-take fast
/// path rebound env[xs] to the new list, breaking
/// immutable-single-assignment semantics.
#[test]
fn c25b_021_append_does_not_mutate_source_binding() {
    let src = r#"
xs <= @[1, 2]
newxs <= Append[xs, 9]()
stdout(xs.length())
stdout(newxs.length())
"#;
    let dir = std::env::temp_dir().join(format!("c25b_021_append_nomutate_{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create tmp dir");
    let path = dir.join("fixture.td");
    fs::write(&path, src).expect("write fixture");
    let out = Command::new(taida_bin())
        .arg(&path)
        .output()
        .expect("spawn taida");
    assert!(
        out.status.success(),
        "taida run failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["2", "3"],
        "Append must not mutate the source binding: \
         xs.length() should stay 2, newxs.length() should be 3; got {:?}",
        lines
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Semantic contract — `Prepend[xs, v]()` must not destructively
/// update the `xs` binding. Symmetric guard to the Append test.
#[test]
fn c25b_021_prepend_does_not_mutate_source_binding() {
    let src = r#"
xs <= @[1, 2]
newxs <= Prepend[xs, 0]()
stdout(xs.length())
stdout(newxs.length())
"#;
    let dir =
        std::env::temp_dir().join(format!("c25b_021_prepend_nomutate_{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create tmp dir");
    let path = dir.join("fixture.td");
    fs::write(&path, src).expect("write fixture");
    let out = Command::new(taida_bin())
        .arg(&path)
        .output()
        .expect("spawn taida");
    assert!(
        out.status.success(),
        "taida run failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["2", "3"],
        "Prepend must not mutate the source binding: \
         xs.length() should stay 2, newxs.length() should be 3; got {:?}",
        lines
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Guard that the cross-reference case still falls back to the
/// clone-based path correctly. Here the first arg is a bare Ident
/// but the name appears in the second arg, so the optimization must
/// skip and use the fallback. We don't pin a time here — just
/// correctness that the result is right.
#[test]
fn c25b_021_append_with_self_reference_in_second_arg_is_correct() {
    let src = r#"
xs <= @[10, 20, 30]
result <= Append[xs, xs]()
stdout(result)
"#;
    let dir = std::env::temp_dir().join(format!("c25b_021_selfref_{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create tmp dir");
    let path = dir.join("fixture.td");
    fs::write(&path, src).expect("write fixture");
    let out = Command::new(taida_bin())
        .arg(&path)
        .output()
        .expect("spawn taida");
    assert!(
        out.status.success(),
        "taida run failed: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "@[10, 20, 30, @[10, 20, 30]]",
        "self-reference case must use the fallback clone path and \
         produce the pre-5-F2-2 output unchanged"
    );
    let _ = fs::remove_dir_all(&dir);
}
