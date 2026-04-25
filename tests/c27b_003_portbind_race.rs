//! C27B-003 (@c.27 Round 2, wE) — port-bind race regression guard.
//!
//! # Background
//!
//! C26B-003 closed the kernel-ephemeral port collision window in
//! `tests/parity.rs::find_free_loopback_port` (cooldown list, double-bind
//! check, allocator restricted below `ip_local_port_range.min`). However,
//! a separate failure mode persisted in `src/interpreter/net_eval/tests.rs`
//! unit tests: a blind `sleep(100ms)` followed by a bare
//! `TcpStream::connect(...).unwrap()` would surface ConnectionRefused
//! (errno 111) when a freshly spawned interpreter thread had not yet
//! reached `TcpListener::bind()` within the 100 ms window — observed at
//! ~6/100 single-test runs of `test_http_serve_max_requests_3` on this
//! 16T workstation, and reported in CI as the dominant remaining
//! `project_flaky_h2_parity.md` symptom.
//!
//! # Root cause
//!
//! "Wait for wall-clock" is not the same as "wait for bind". The fix
//! (`net_eval/tests.rs::connect_with_retry`) polls up to 200 attempts × 10
//! ms = 2 s, retrying only on ConnectionRefused. This eliminates the
//! sleep race entirely while preserving fail-fast behavior on real
//! errors (any non-ConnectionRefused error panics immediately).
//!
//! # This regression guard
//!
//! Runs the historically-flakiest unit test (`test_http_serve_max_requests_3`)
//! many times in a row from an integration-test crate, asserting 100%
//! success across the iteration count. The `--ignored` gating allows CI
//! to opt in for the long version (100 iters, ~7 minutes) while the
//! default `cargo test` invocation runs a smoke (10 iters, ~45 s).
//!
//! # D28 escalation checklist (3 points, all NO → C27 scope-in)
//!
//!  1. **Public mold signature unchanged.** Test-helper-only fix.
//!  2. **No STABILITY-pinned error string altered.** No surface change.
//!  3. **Append-only with respect to existing fixtures.** No fixture changed.
//!
//! # Acceptance
//!
//! - `cargo test --release --test c27b_003_portbind_race` (smoke, 10 iter)
//!   → 10/10 GREEN, no ConnectionRefused panic.
//! - `cargo test --release --test c27b_003_portbind_race -- --ignored`
//!   (long, 100 iter) → 100/100 GREEN.
//!
//! Both modes assert that the originally-flaky behavior is gone and
//! cannot regress without alerting CI.

mod common;

use common::taida_bin;
use std::process::Command;

fn run_one_iteration() -> bool {
    let out = Command::new("cargo")
        .args([
            "test",
            "--release",
            "--lib",
            "net_eval::tests::test_http_serve_max_requests_3",
            "--",
            "--test-threads=1",
        ])
        .output()
        .expect("spawn cargo test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout.contains("1 passed")
}

#[test]
fn c27b_003_portbind_race_smoke_10_iter() {
    // Smoke variant: 10 iterations. Runs in ~45 s on a 16T machine, ~90 s
    // on CI 2C. Catches gross regressions of the connect-with-retry
    // helper without bloating CI wallclock.
    //
    // The pre-fix baseline on this workstation showed 6/100 failures, so
    // 10 iterations is statistically tight (p(0 failure | 6% rate) ≈ 53%).
    // For higher confidence run the `--ignored` long variant below.
    //
    // We also smoke that `taida_bin()` resolves so harness errors are
    // distinguished from regression hits.
    assert!(taida_bin().exists(), "taida binary must exist");

    let mut pass = 0u32;
    let mut fail = 0u32;
    for i in 0..10 {
        if run_one_iteration() {
            pass += 1;
        } else {
            fail += 1;
            eprintln!("c27b_003 smoke iter {} FAILED", i);
        }
    }
    assert_eq!(
        fail, 0,
        "C27B-003 smoke regression: {} pass / {} fail (expected 10/0)",
        pass, fail
    );
}

#[test]
#[ignore = "long-running 100-iteration regression: opt in via --ignored on CI"]
fn c27b_003_portbind_race_long_100_iter() {
    // Long variant: 100 iterations. ~7 min on 16T, ~15 min on CI 2C.
    // Run from CI weekly / on PR labels touching net_eval. The pre-fix
    // baseline showed 6/100 failures so any non-zero count flags a
    // regression of `connect_with_retry`.
    assert!(taida_bin().exists(), "taida binary must exist");

    let mut pass = 0u32;
    let mut fail = 0u32;
    for i in 0..100 {
        if run_one_iteration() {
            pass += 1;
        } else {
            fail += 1;
            eprintln!("c27b_003 long iter {} FAILED", i);
        }
    }
    assert_eq!(
        fail, 0,
        "C27B-003 long regression: {} pass / {} fail (expected 100/0)",
        pass, fail
    );
}
