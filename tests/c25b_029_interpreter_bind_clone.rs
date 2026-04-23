//! C25B-029 — interpreter `<=` deep-clone on locally-bound addon
//! `Value::Record` perf regression guard (Phase 5-F scope-reduced).
//!
//! # Scope
//!
//! Phase 5-F (C25_PROGRESS.md) started by exploring an
//! `HashMap<String, Arc<Value>>` environment to turn Ident lookups
//! into O(1) `Arc::clone` calls. The exploration revealed that without
//! also migrating `Value::List` / `Value::BuchiPack` to interior
//! `Arc<Vec<...>>` with copy-on-write, the outer `Rc::clone` plus
//! inner `Value::clone` combination is still a full deep-clone with no
//! net wall-clock improvement. The proper root-cause fix is the Value
//! interior-arc migration (tracked as 5-F2 in C25_PROGRESS.md), which
//! is out of scope for the current session and will land in a
//! dedicated follow-up or in D26.
//!
//! # What this test pins
//!
//! The minimal reproducer built around a pure-Taida `Value::BuchiPack`
//! containing a 4800-item `Value::List` of cell-shaped BuchiPacks.
//! Without the Phase 5-F2-1 interior-Arc migration, a chain of 11
//! `touch(p)` calls (each clones the outer pack's cells reference)
//! performs repeated deep copies and balloons wall-clock.
//!
//! We *intentionally* do **not** assert "fast" numbers here; the
//! blocker is still open. Instead the test:
//!   * validates the fixture *runs to completion at all*;
//!   * pins the shape of the final value so a regression in Append
//!     / BuchiPack field access surfaces loudly;
//!   * ships a `SLOW` annotation so a future Phase 5-F2 landing can
//!     flip this test to a hard latency gate without re-writing it.
mod common;

use common::taida_bin;
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

/// The scenario mirrors Hachikuma's Pane.Render pattern: carry a
/// large structured buffer through a chain of `touch` calls. To keep
/// this test scoped to C25B-029, we materialize the cell list as a
/// large literal instead of building it with `Append`, whose O(N²)
/// tail-recursive accumulator cost is already pinned separately by
/// C25B-021.
///
/// Phase 5-F2-1 (2026-04-23) migrated `Value::List` to interior
/// `Arc<Vec<Value>>`, collapsing the touch-chain cost from repeated
/// Vec deep-clones to cheap Arc refcount bumps. Before that change,
/// the pure-Taida reproducer froze for multiple seconds even though
/// the operation was just "clone outer pack, keep same cells list".
///
/// A 4800-item literal list is large enough to make a deep-clone
/// regression visible while remaining stable on CI because it avoids
/// the unrelated `Append` builder path entirely.
const CELL_COUNT: usize = 4800;
const TOUCH_CHAIN_LEN: usize = 11;
// The 15.8 s seen on PR #39 was caused by the fixture building its
// cells via Append recursion — the O(N²) Append ceiling that
// session-20 reverted. After switching the fixture to a 4800-item
// literal list (no Append), the test measures only the touch-chain
// shallow-clone path, which Phase 5-F2-1's List interior-Arc keeps
// effectively O(chain). Local measurement: ~0.03 s release / 3-run
// median. The 1 s ceiling leaves room for CI 2C jitter while still
// catching freeze-class regressions.
const MAX_DURATION: Duration = Duration::from_secs(1);

fn clone_probe_fixture() -> String {
    let cell_literal = "@(ch <= \"a\", fg <= \"white\", bg <= \"black\")";
    let cells = std::iter::repeat_n(cell_literal, CELL_COUNT)
        .collect::<Vec<_>>()
        .join(", ");

    let mut fixture =
        String::from("touch p =\n  @(cols <= p.cols, rows <= p.rows, cells <= p.cells)\n\n");
    fixture.push_str("cells <= @[");
    fixture.push_str(&cells);
    fixture.push_str("]\n");
    fixture.push_str("buf <= @(cols <= 120, rows <= 40, cells <= cells)\n\n");

    for i in 1..=TOUCH_CHAIN_LEN {
        let current = format!("b{i:02}");
        let prev = if i == 1 {
            "buf".to_string()
        } else {
            format!("b{:02}", i - 1)
        };
        fixture.push_str(&format!("{current} <= touch({prev})\n"));
    }

    fixture.push_str("stdout(b11.cols)\nstdout(b11.rows)\n");
    fixture
}

#[test]
fn c25b_029_bufferlike_touch_chain_does_not_freeze() {
    let dir = std::env::temp_dir().join(format!("c25b_029_{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create tmp dir");
    let path = dir.join("clone_probe.td");
    fs::write(&path, clone_probe_fixture()).expect("write fixture");

    let start = Instant::now();
    let out = Command::new(taida_bin())
        .arg(&path)
        .output()
        .expect("spawn taida");
    let elapsed = start.elapsed();

    assert!(
        out.status.success(),
        "clone probe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        elapsed < MAX_DURATION,
        "C25B-029 clone probe took {}ms — regressed past {}ms ceiling. \
         The fixture avoids Append/Prepend entirely, so this points at \
         the touch() clone path itself rather than the separately-tracked \
         list-builder cost. Phase 5-F2-1's List interior-Arc contract \
         must keep this chain effectively O(chain).",
        elapsed.as_millis(),
        MAX_DURATION.as_millis()
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["120", "40"],
        "touch chain output regressed — BuchiPack field access path changed"
    );

    let _ = fs::remove_dir_all(&dir);
}
