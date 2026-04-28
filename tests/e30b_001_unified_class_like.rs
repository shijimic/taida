//! E30B-001 / Phase 7 sub-track A: aggregate test for the unified class-like
//! type system + `taida upgrade --e30` migration tool completion.
//!
//! This file is the integration / acceptance test for E30B-001 Phase 7 work:
//! the `Mold[T] => Foo[T] = @(...)` legacy header is rewritten in place to
//! the new unified `Foo[T] = @(...)` form. The migration tool is documented
//! in `src/upgrade_e30.rs`; this file pins the public surface (file I/O,
//! exit-code semantics, idempotency, and migration-fixture round-trip).
//!
//! Lock-E verdict integration:
//!  - subcommand: `taida upgrade --e30 <PATH>`
//!  - default mode: in-place rewrite
//!  - `--dry-run`: print proposals, no writes
//!  - `--check`: exit non-zero on legacy detection
//!  - deprecation: E gen has no deprecation, this is an immediate breaking
//!    change tool (no warning phase)
//!
//! 4-backend parity (interpreter / JS / native / wasm-wasi) for the
//! migrated `.td` output is exercised in `tests/parity.rs::e30b_001_*`
//! via fixtures shipped under
//! `examples/quality/e30b_001_unified_class_like/`.

use std::path::PathBuf;

use taida::upgrade_e30::{
    UpgradeE30Config, UpgradeE30Error, run, scan_source, upgrade_file, upgrade_source,
};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("quality")
        .join("e30b_001_unified_class_like")
}

// ── upgrade_source: text-level rewrite ──────────────────────────────

#[test]
fn upgrade_source_drops_legacy_mold_prefix_single_param() {
    let src = "Mold[T] => Box[T] = @(label: Str, transform: T => :T)\n";
    let (out, n) = upgrade_source(src);
    assert_eq!(out, "Box[T] = @(label: Str, transform: T => :T)\n");
    assert_eq!(n, 1);
}

#[test]
fn upgrade_source_drops_legacy_mold_prefix_multi_param() {
    let src = "Mold[T, U] => Pair[T, U] = @(first: T, second: U)\n";
    let (out, n) = upgrade_source(src);
    assert_eq!(out, "Pair[T, U] = @(first: T, second: U)\n");
    assert_eq!(n, 1);
}

#[test]
fn upgrade_source_handles_concrete_arg_mold_prefix() {
    let src = "Mold[:Int] => IntBox = @(value: Int)\n";
    let (out, n) = upgrade_source(src);
    assert_eq!(out, "IntBox = @(value: Int)\n");
    assert_eq!(n, 1);
}

#[test]
fn upgrade_source_handles_constrained_param_mold_prefix() {
    let src = "Mold[T <= :Int] => IntBox[T] = @(value: T)\n";
    let (out, n) = upgrade_source(src);
    assert_eq!(out, "IntBox[T] = @(value: T)\n");
    assert_eq!(n, 1);
}

#[test]
fn upgrade_source_is_idempotent() {
    let src = "Mold[T] => Box[T] = @(label: Str, transform: T => :T)\n";
    let (once, n1) = upgrade_source(src);
    assert_eq!(n1, 1, "first pass must rewrite");
    let (twice, n2) = upgrade_source(&once);
    assert_eq!(n2, 0, "second pass must be a no-op");
    assert_eq!(once, twice, "idempotency: second pass output identical");
}

#[test]
fn upgrade_source_leaves_new_form_untouched() {
    // Lock-B Sub-B1 / Sub-B2: zero-arity sugar / Error-prefix-optional
    // are NEW forms; migration must not touch them.
    let src = "Pilot = @(name: Str)\n\
               Pilot[] = @(name: Str)\n\
               Box[T] = @(filling: T)\n\
               Error => NotFound = @(msg: Str)\n";
    let (out, n) = upgrade_source(src);
    assert_eq!(out, src);
    assert_eq!(n, 0);
}

#[test]
fn upgrade_source_handles_consecutive_legacy_defs() {
    let src = "Mold[T] => Box[T] = @(filling: T)\n\
               Mold[T, U] => Pair[T, U] = @(first: T, second: U)\n";
    let (out, n) = upgrade_source(src);
    let expected = "Box[T] = @(filling: T)\n\
                    Pair[T, U] = @(first: T, second: U)\n";
    assert_eq!(out, expected);
    assert_eq!(n, 2);
}

// ── upgrade_file: file-level rewrite (tempdir) ─────────────────────

#[test]
fn upgrade_file_default_mode_rewrites_in_place() {
    let tmp = tempdir();
    let path = tmp.path.join("legacy.td");
    std::fs::write(&path, "Mold[T] => Box[T] = @(filling: T)\n").unwrap();

    let result = upgrade_file(&path, false, false).expect("upgrade_file ok");
    assert_eq!(result.rewrites, 1);
    assert!(result.changed);

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, "Box[T] = @(filling: T)\n");
}

#[test]
fn upgrade_file_dry_run_does_not_write() {
    let tmp = tempdir();
    let path = tmp.path.join("legacy.td");
    let original = "Mold[T] => Box[T] = @(filling: T)\n";
    std::fs::write(&path, original).unwrap();

    let result = upgrade_file(&path, false, true).expect("upgrade_file ok");
    assert_eq!(result.rewrites, 1);
    // changed flag reflects "would change", but file untouched
    assert!(result.changed);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn upgrade_file_check_only_does_not_write() {
    let tmp = tempdir();
    let path = tmp.path.join("legacy.td");
    let original = "Mold[T] => Box[T] = @(filling: T)\n";
    std::fs::write(&path, original).unwrap();

    let result = upgrade_file(&path, true, false).expect("upgrade_file ok");
    assert_eq!(result.rewrites, 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
}

#[test]
fn upgrade_file_no_op_for_new_form() {
    let tmp = tempdir();
    let path = tmp.path.join("modern.td");
    let modern = "Pilot[] = @(name: Str)\nBox[T] = @(filling: T)\n";
    std::fs::write(&path, modern).unwrap();

    let result = upgrade_file(&path, false, false).expect("upgrade_file ok");
    assert_eq!(result.rewrites, 0);
    assert!(!result.changed);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), modern);
}

// ── run: directory-recursive entry point ───────────────────────────

#[test]
fn run_check_mode_returns_check_failed_on_legacy() {
    let tmp = tempdir();
    let path = tmp.path.join("legacy.td");
    std::fs::write(&path, "Mold[T] => Box[T] = @(filling: T)\n").unwrap();

    let cfg = UpgradeE30Config {
        path: tmp.path.clone(),
        check_only: true,
        dry_run: false,
    };
    match run(cfg) {
        Err(UpgradeE30Error::CheckFailed { legacy_count }) => {
            assert_eq!(legacy_count, 1);
        }
        other => panic!("expected CheckFailed, got {:?}", other),
    }
    // check mode must not modify the file
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "Mold[T] => Box[T] = @(filling: T)\n"
    );
}

#[test]
fn run_check_mode_returns_ok_for_clean_tree() {
    let tmp = tempdir();
    let path = tmp.path.join("clean.td");
    let clean = "Box[T] = @(filling: T)\n";
    std::fs::write(&path, clean).unwrap();

    let cfg = UpgradeE30Config {
        path: tmp.path.clone(),
        check_only: true,
        dry_run: false,
    };
    let report = run(cfg).expect("clean tree returns Ok");
    assert_eq!(report.legacy_count, 0);
    assert_eq!(report.files_scanned, 1);
}

#[test]
fn run_default_mode_rewrites_directory_recursively() {
    let tmp = tempdir();
    let nested = tmp.path.join("inner");
    std::fs::create_dir(&nested).unwrap();
    let a = tmp.path.join("a.td");
    let b = nested.join("b.td");
    std::fs::write(&a, "Mold[T] => Box[T] = @(filling: T)\n").unwrap();
    std::fs::write(&b, "Mold[T, U] => Pair[T, U] = @(first: T, second: U)\n").unwrap();

    let cfg = UpgradeE30Config {
        path: tmp.path.clone(),
        check_only: false,
        dry_run: false,
    };
    let report = run(cfg).expect("default-mode rewrite ok");
    assert_eq!(report.legacy_count, 2);

    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        "Box[T] = @(filling: T)\n"
    );
    assert_eq!(
        std::fs::read_to_string(&b).unwrap(),
        "Pair[T, U] = @(first: T, second: U)\n"
    );
}

// ── fixture round-trip: pre-migration -> tool output == post-migration

#[test]
fn fixture_legacy_mold_migrates_to_post_class_like() {
    let pre = std::fs::read_to_string(fixtures_dir().join("legacy_mold_pre.td")).unwrap();
    let post_expected =
        std::fs::read_to_string(fixtures_dir().join("migrated_class_like_post.td")).unwrap();

    // The migration tool drops only the legacy `Mold[T] => ` prefix.
    // The instantiation-site change between Mold contract
    // (`Box[1, "apple"]()`) and class-like contract
    // (`Box(label <= "apple")`) is a separate, manual rewrite step
    // documented in `migrated_class_like_post.td`. So we compare ONLY
    // the migrated header against the post fixture's header.
    let (migrated_pre, n) = upgrade_source(&pre);
    assert!(n >= 1, "fixture must have at least 1 legacy mold");

    // Smoke check: the legacy declaration `Mold[T] => Box[T]` line is gone.
    // (Comments still mention `Mold[T]` for documentation, so we look at
    //  the actual statement-line shape.)
    assert!(
        !migrated_pre.contains("\nMold[T] => Box[T]"),
        "migrated source must not contain the legacy declaration: {:?}",
        migrated_pre
    );
    assert!(migrated_pre.contains("Box[T] = @"));

    // Smoke check: the post-fixture also contains the new header form.
    assert!(post_expected.contains("Box[T] = @"));
    assert!(
        !post_expected.contains("\nMold[T] => Box[T]"),
        "post fixture must not declare the legacy form"
    );

    // Idempotency on the fixture itself.
    let (migrated_twice, n2) = upgrade_source(&migrated_pre);
    assert_eq!(n2, 0, "fixture migration must be idempotent");
    assert_eq!(migrated_twice, migrated_pre);
}

// ── scan_source: legacy detection (delegates to existing skeleton) ─

#[test]
fn scan_source_locates_legacy_mold_in_source() {
    let proposals = scan_source(
        "Mold[T] => Box[T] = @(filling: T)\n",
        std::path::Path::new("test.td"),
    );
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].legacy_kind, "mold");
    assert_eq!(proposals[0].legacy_header, "Mold[T] => Box[T]");
    assert_eq!(proposals[0].proposed_header, "Box[T]");
}

// ── tempdir helper (no external dep) ─────────────────────────────

struct TempDir {
    path: PathBuf,
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn tempdir() -> TempDir {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("e30b001_unified_{}_{}", pid, n));
    std::fs::create_dir_all(&path).expect("tempdir create");
    TempDir { path }
}
