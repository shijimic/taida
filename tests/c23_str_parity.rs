//! C23: `Str[...]()` mold family 4-backend parity regression guard.
//!
//! Purpose
//! -------
//! C21 / C22 (PR #36) review follow-up raised FB-34, and subsequent audit
//! (`.dev/C23_BLOCKERS.md`) widened it to three root-cause blockers on the
//! `Str[x]()` mold family:
//!
//!   - C23B-001 (wasm primitive/Lax):
//!       * `Str[3.0]()` rendered `__value <= "3.0"` (should be `"3"`)
//!       * `Str[true]()` / `Str["abc"]()` / `Str[3.0]()` rendered
//!         `__default <= <pointer integer>` (should be `""`)
//!   - C23B-002 (native / wasm non-primitive): `Str[@[1,2,3]]()` /
//!     `Str[@(a <= 1)]()` / `Str[Int[3.0]()]()` fell through to
//!     `taida_str_mold_int`, so the Lax carried a raw pointer integer as
//!     `__value` instead of the interpreter's display string.
//!   - C23B-003 (JS, then reopened): `Str_mold(value)` initially used
//!     `String(value)` (`[object Object]` / `Lax(3)` short-form);
//!     post-C23-3 fix still fell through for runtime-object types
//!     (HashMap / Stream / Set / TODO / Gorillax / RelaxedGorillax), so
//!     their typed display strings leaked method source bodies. The
//!     reopen also covers native / wasm short-form HashMap (`HashMap({…})`)
//!     and Set (`Set({…})`) divergences, plus a missing `__error` field
//!     name registration that caused native Gorillax full-form to render
//!     as `@()`.
//!
//! Each fixture under `examples/quality/c23b_str_parity/` pins one branch
//! of the dispatch. Interpreter is the reference (`src/interpreter/mold_eval.rs`
//! `Str` arm → `format!("{}", other)` for non-primitive values). JS / Native
//! / WASM-wasi must match the interpreter byte-for-byte.
//!
//! Some fixtures are scoped narrower than 4-backend because of pre-existing
//! backend limitations that fall outside the C23 track:
//!   * `str_from_gorillax` — skipped on wasm-wasi (see
//!     `WASM_SKIP_FIXTURES`). The wasm runtime stores Gorillax with
//!     `isOk` as the first field name, while interpreter / JS / native
//!     all use `hasValue`. Unifying this requires changing wasm's
//!     `WASM_HASH_IS_OK` scheme and is a separate follow-up.
//!   * `str_from_stream` — interpreter + JS only (see
//!     `STREAM_ONLY_FIXTURES`). Native / wasm lowering do not yet support
//!     Stream (`unsupported mold type: Stream`).

mod common;

use common::{normalize, taida_bin, wasmtime_bin};
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Backend runners — mirror `tests/c21_float_fn_boundary.rs` conventions.
// ---------------------------------------------------------------------------

fn run_interpreter(td_path: &Path) -> Option<String> {
    let out = Command::new(taida_bin()).arg(td_path).output().ok()?;
    if !out.status.success() {
        eprintln!(
            "interpreter failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&out.stdout)))
}

fn tmp_artifact(td_path: &Path, suffix: &str) -> PathBuf {
    let stem = td_path.file_stem().unwrap().to_string_lossy();
    std::env::temp_dir().join(format!(
        "c23_str_{}_{}.{}",
        std::process::id(),
        stem,
        suffix
    ))
}

fn run_js(td_path: &Path) -> Option<String> {
    let js_path = tmp_artifact(td_path, "mjs");

    let build = Command::new(taida_bin())
        .args(["build", "--target", "js"])
        .arg(td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .ok()?;
    if !build.status.success() {
        let _ = std::fs::remove_file(&js_path);
        eprintln!(
            "js build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }

    let run = Command::new("node").arg(&js_path).output().ok()?;
    let _ = std::fs::remove_file(&js_path);
    if !run.status.success() {
        eprintln!(
            "node failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

fn run_native(td_path: &Path) -> Option<String> {
    let bin_path = tmp_artifact(td_path, "bin");

    let build = Command::new(taida_bin())
        .args(["build", "--target", "native"])
        .arg(td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .ok()?;
    if !build.status.success() {
        let _ = std::fs::remove_file(&bin_path);
        eprintln!(
            "native build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }

    let run = Command::new(&bin_path).output().ok()?;
    let _ = std::fs::remove_file(&bin_path);
    if !run.status.success() {
        eprintln!(
            "native binary failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

fn run_wasm_wasi(td_path: &Path) -> Option<String> {
    let wasmtime = wasmtime_bin()?;
    let wasm_path = tmp_artifact(td_path, "wasm");

    let build = Command::new(taida_bin())
        .args(["build", "--target", "wasm-wasi"])
        .arg(td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .ok()?;
    if !build.status.success() {
        let _ = std::fs::remove_file(&wasm_path);
        eprintln!(
            "wasm-wasi build failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&build.stderr)
        );
        return None;
    }

    let run = Command::new(&wasmtime).arg(&wasm_path).output().ok()?;
    let _ = std::fs::remove_file(&wasm_path);
    if !run.status.success() {
        eprintln!(
            "wasmtime failed for {}: {}",
            td_path.display(),
            String::from_utf8_lossy(&run.stderr)
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

fn which_node() -> Option<()> {
    Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| if o.status.success() { Some(()) } else { None })
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const FIXTURES: &[&str] = &[
    "str_from_float_int_form",
    "str_from_float_frac_form",
    "str_from_bool",
    "str_from_str",
    "str_from_list",
    "str_from_pack",
    "str_from_lax",
    // C23B-003 reopen — typed runtime values.
    "str_from_hashmap",
    "str_from_set",
    "str_from_gorillax",
    "str_from_stream",
    // C23B-003 reopen 2 — nested typed runtime object recursion.
    // Pin that nested HashMap/Set/Pack/List items all render through
    // the full-form helper on every backend (no collapse to
    // `HashMap({…})` / `Set({…})` short-form).
    "str_from_nested_hashmap",
    "str_from_nested_set",
    "str_from_list_of_hashmap",
    "str_from_pack_with_hashmap",
    // C23B-003 reopen 3 — empty ぶちパック (`@()` / `Value::Unit`) recursion.
    // WASM only: `_looks_like_pack` rejects `fc == 0`, so nested `@()`
    // fell through to `taida_int_to_str` and rendered as the raw heap
    // pointer integer. Now routed through `_looks_like_empty_pack` on
    // every wasm display helper, matching the interpreter's
    // `Value::Unit.to_debug_string() -> "@()"` contract across all four
    // backends (JS / Native / Interpreter were already correct).
    "str_from_empty_pack",
    "str_from_pack_with_empty_pack",
    "str_from_hashmap_with_empty_pack",
    "str_from_list_with_empty_pack",
    // C23B-003 reopen 4 — dynamic Int expressions routed through the
    // generic `Str[...]()` path. The WASM `_looks_like_empty_pack`
    // detector used to rely on a heap-range + zero-slot heuristic that
    // false-positive'd on dynamic Int sums whose bit pattern landed
    // inside the bump arena (e.g. `Str[a + b]()` for a + b ≈ 73088).
    // These fixtures pin the fix at two levels:
    //   1. Detector — `_looks_like_empty_pack` now requires a magic
    //      sentinel (`WASM_EMPTY_PACK_MAGIC`) stamped in `pack[1]` by
    //      `taida_pack_new(0)`, so plain integers cannot false-match.
    //   2. Lowering — `src/codegen/lower_molds.rs` `Str` dispatch uses
    //      `Lowering::expr_is_int` (was `expr_is_int_literal`) to
    //      short-circuit dynamic Int expressions (variables /
    //      arithmetic / negation / int-returning function calls) into
    //      `taida_str_mold_int` before they reach the generic helper.
    "str_from_dynamic_int",
    "str_from_dynamic_int_zero",
    "str_from_dynamic_int_negative",
    "str_from_dynamic_int_funcall",
    // C23B-005 reopen + widen + C23B-006 — WASM collection detector
    // false-positives on untagged large Ints (73088 aliased a heap
    // allocation whose `data[3]` matched a 4-byte collection marker).
    // These fixtures pin the tag-based positive identification at two
    // levels:
    //   1. Wide 8-byte printable-ASCII magic sentinels at every
    //      collection's `data[3]` (lists / sets / hashmaps) or tail
    //      (non-empty packs). `_looks_like_list` / `_is_wasm_set` /
    //      `_is_wasm_hashmap` / `_looks_like_pack` require exact
    //      sentinel match — heuristics are abolished.
    //   2. Tag-aware element renderers
    //      (`_wasm_render_elem_tagged_debug` +
    //      `_wasm_render_elem_tagged_debug_full`) dispatch primitive
    //      Int / Float / Bool / Str members via the stored
    //      `elem_type_tag` (list / set slot 2) / `value_type_tag`
    //      (hashmap slot 2) / `field_tag` (pack per-field slot),
    //      bypassing structural detectors entirely for primitive
    //      values. Even if a sentinel check were somehow fooled,
    //      primitives still never enter the recursive render path.
    "str_from_hashmap_with_large_int",
    "str_from_set_with_large_int",
    "str_from_list_with_large_int",
    "str_from_pack_with_large_int",
    "str_from_nested_collection_with_large_int",
    // C23B-007 — WASM tag re-promotion into heterogeneous containers.
    // Previously `taida_list_set_elem_tag` / `taida_hashmap_set_value_tag`
    // downgraded to UNKNOWN(-1) on type conflict but treated -1 as
    // "unset" on the next write, letting the tag re-promote to the
    // freshly-pushed primitive type. A later `_wasm_render_elem_tagged_debug`
    // then forced every element through that tag's fast path, rendering
    // non-matching primitives as the wrong type (e.g. string pointer
    // emitted as Int). Fix: introduced `WASM_TAG_HETEROGENEOUS = -2` as
    // a separate latching sentinel; once a container becomes
    // HETEROGENEOUS it stays that way. Native mirrored for symmetry and
    // defence in depth. Also taught `lower_list_lit` to stamp every
    // element's tag (was: only first) so list literals with mixed
    // primitives can trigger the downgrade path.
    "str_from_mixed_list",
    "str_from_mixed_hashmap",
    "str_from_mixed_set",
    "str_from_nested_mixed",
    // C23B-008 — multi-entry HashMap display must walk in insertion
    // order, not bucket order. Native / WASM previously iterated
    // buckets, so `hashMap().set("a", 1).set("b", 2)` came out as "b",
    // "a". Fix: append an insertion-order side-index
    // (`[next_ord, order_array[cap]]`) after the trailing magic on
    // both runtimes; insert/update/remove/resize/clone maintain it;
    // display / entries / keys / values / merge / JSON walk it. JS was
    // already correct (its `__entries` is a plain insertion-ordered
    // Array, matching interpreter's `Vec<(k,v)>`).
    "str_from_multi_entry_hashmap",
    "str_from_large_hashmap",
    "str_from_hashmap_after_remove",
    "str_from_hashmap_update_preserves_order",
    // C23B-008 reopen (2026-04-22): HashMap.merge() must follow the
    // interpreter's retain-then-push semantics
    // (`src/interpreter/methods.rs:787-822`). Previous native / wasm / JS
    // implementations called `taida_hashmap_set` (update-in-place) per
    // `other` entry, which preserved self's ordinal for overlap keys
    // instead of moving them to other's position with other's value.
    // Fix: allocate a fresh map, fill with (self \ other) in self-order,
    // then append every other entry in other-order (all guaranteed new
    // to the fresh map). Interpreter unchanged (source of truth). These
    // fixtures pin:
    //   * overlap — one key shared between self and other moves position
    //   * non_overlap — degenerate path, self-order + other-order
    //   * full_overlap — every self key in other, result = other in
    //     other-order with other's values
    //   * empty_self — retain-then-push over an empty self is other
    //   * empty_other — retain-then-push with empty other is self
    //   * resize — 16-entry merge that crosses the 0.75 load factor on
    //     the fresh result map (exercises `taida_hashmap_resize` +
    //     side-index rebuild during the fill loop)
    "str_from_hashmap_merge_overlap",
    "str_from_hashmap_merge_non_overlap",
    "str_from_hashmap_merge_full_overlap",
    "str_from_hashmap_merge_empty_self",
    "str_from_hashmap_merge_empty_other",
    "str_from_hashmap_merge_resize",
    // C23B-009 — HashMap.entries() field-name parity. Previously JS
    // emitted `@(first <= …, second <= …)` (legacy zip()-style
    // convention) while Native / WASM emitted `@()` (pair field-name
    // hashes were never registered in the field-name lookup, so
    // `taida_pack_to_display_string_full` / `_wasm_pack_to_string_full`
    // silently skipped every field). Interpreter
    // (`src/interpreter/methods.rs:761-783`) and the documented contract
    // (`docs/reference/standard_library.md:238`) both use
    // `@[@(key, value)]`. Fix: JS renamed to `{key, value}`; Native /
    // WASM idempotently register the `HASH_KEY` / `HASH_VAL` hashes with
    // `"key"` / `"value"` inside `taida_hashmap_entries`; WASM also
    // stamps per-field tags (`WASM_TAG_STR` on key, hashmap's
    // `value_type_tag` on value) and elem tag (`WASM_TAG_PACK` on the
    // outer list) for tagged rendering fast-path. Pins:
    //   * basic — two entries, insertion-order walk
    //   * empty — edge case, short-circuits before field lookup
    //   * single — minimum non-empty payload, exercises tag stamping
    //   * after_remove — walks side-index with null-out holes
    "str_from_hashmap_entries",
    "str_from_hashmap_entries_empty",
    "str_from_hashmap_entries_single",
    "str_from_hashmap_entries_after_remove",
];

/// Fixtures that interpreter + JS support but the other backends cannot
/// currently exercise (backend lowering limitation, tracked outside C23).
const STREAM_ONLY_FIXTURES: &[&str] = &["str_from_stream"];

/// Fixtures skipped on wasm-wasi because of a pre-existing backend
/// divergence (not a C23 regression, not solvable inside the C23 track).
const WASM_SKIP_FIXTURES: &[&str] = &[
    // Wasm Gorillax packs use `isOk` for the first field (see
    // `src/codegen/runtime_core_wasm/02_containers.inc.c:344`) where
    // interpreter / JS / native all use `hasValue`. Unifying the wasm
    // hash scheme is a separate follow-up (would also require auditing
    // any user-facing `.isOk()` calls compiled for wasm).
    "str_from_gorillax",
];

fn is_stream_only(name: &str) -> bool {
    STREAM_ONLY_FIXTURES.contains(&name)
}

fn is_wasm_skipped(name: &str) -> bool {
    WASM_SKIP_FIXTURES.contains(&name) || is_stream_only(name)
}

fn is_native_skipped(name: &str) -> bool {
    // Native doesn't support Stream lowering; other typed values are
    // matched via the native fixes in this same commit.
    is_stream_only(name)
}

fn fixture_td(name: &str) -> PathBuf {
    PathBuf::from(format!("examples/quality/c23b_str_parity/{}.td", name))
}

fn fixture_expected(name: &str) -> String {
    let path = PathBuf::from(format!(
        "examples/quality/c23b_str_parity/{}.expected",
        name
    ));
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    normalize(&raw)
}

// ---------------------------------------------------------------------------
// Interpreter reference — must pin first, so the `.expected` files never
// drift away from the source of truth (`src/interpreter/mold_eval.rs` `Str`).
// ---------------------------------------------------------------------------

#[test]
fn interpreter_matches_expected_fixtures() {
    for name in FIXTURES {
        let td = fixture_td(name);
        let out = run_interpreter(&td).expect("interpreter should succeed");
        let exp = fixture_expected(name);
        assert_eq!(
            out, exp,
            "interpreter output for {} drifted from .expected (source of truth)",
            name
        );
    }
}

// ---------------------------------------------------------------------------
// JS parity (C23-3)
// ---------------------------------------------------------------------------

#[test]
fn js_matches_interpreter() {
    if which_node().is_none() {
        return; // CI hosts without Node skip cleanly.
    }
    for name in FIXTURES {
        let td = fixture_td(name);
        let exp = fixture_expected(name);
        let out = run_js(&td).unwrap_or_else(|| panic!("js build+run failed for {}", name));
        assert_eq!(
            out, exp,
            "JS output for {} diverged from interpreter reference (C23B-003 regression?)",
            name
        );
    }
}

// ---------------------------------------------------------------------------
// Native parity (C23-2)
// ---------------------------------------------------------------------------

#[test]
fn native_matches_interpreter() {
    for name in FIXTURES {
        if is_native_skipped(name) {
            // Native lowering doesn't support this fixture's mold yet
            // (see `STREAM_ONLY_FIXTURES` / `run_native`'s
            // `unsupported mold type` failure path).
            continue;
        }
        let td = fixture_td(name);
        let exp = fixture_expected(name);
        let out = run_native(&td).unwrap_or_else(|| panic!("native build+run failed for {}", name));
        assert_eq!(
            out, exp,
            "Native output for {} diverged from interpreter reference (C23B-002 / C23B-003 regression?)",
            name
        );
    }
}

// ---------------------------------------------------------------------------
// WASM-wasi parity (C23-2 generic path + C23-4 primitive/Lax path)
// ---------------------------------------------------------------------------

#[test]
fn wasm_wasi_matches_interpreter() {
    if wasmtime_bin().is_none() {
        return; // hosts without wasmtime skip cleanly (mirrors c21_float_fn_boundary).
    }
    for name in FIXTURES {
        if is_wasm_skipped(name) {
            // `WASM_SKIP_FIXTURES` / `STREAM_ONLY_FIXTURES` — see the
            // module doc comment for why these fixtures aren't enforced
            // on wasm-wasi in the C23 track.
            continue;
        }
        let td = fixture_td(name);
        let exp = fixture_expected(name);
        let out =
            run_wasm_wasi(&td).unwrap_or_else(|| panic!("wasm-wasi build+run failed for {}", name));
        assert_eq!(
            out, exp,
            "WASM-wasi output for {} diverged from interpreter reference (C23B-001 / C23B-002 / C23B-003 regression?)",
            name
        );
    }
}
