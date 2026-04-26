//! D28B-007 (2026-04-26, wJ Round 3) — `taida upgrade --d28` AST-aware
//! code-rewrite tool integration test.
//!
//! Pins the contract for the D28 migration tool:
//!
//! 1. **AST-aware rewrite** (not text-pattern substitution): the tool
//!    parses Taida source into AST and rewrites symbols based on
//!    category × value-type per D28B-001 (Phase 0 Lock 2026-04-26).
//!
//! 2. **Idempotent**: running `taida upgrade --d28 <path>` twice on the
//!    same file produces identical output (second run = no-op).
//!
//! 3. **Bit-identical on no-op**: when no rewrites apply, the file is
//!    not touched (zero bytes changed).
//!
//! 4. **`--check` mode**: returns non-zero exit when rewrites would
//!    happen, zero when the file is compliant.
//!
//! 5. **Function-shape values are preserved**: a buchi-pack field
//!    holding a Lambda value (`@(safeDiv <= _ x y = ...)`) keeps its
//!    camelCase name because the Lock allows camelCase for function-
//!    valued fields.
//!
//! Scope (Round 3 wJ initial implementation): the tool focuses on the
//! most common pre-Lock violation — buchi-pack fields with non-function
//! values named in camelCase (e.g. `callSign <= "Eva-02"`). Schema-style
//! field declarations (`User = @(callSign: Str, ...)`) are also covered.

use std::path::PathBuf;
use std::process::Command;

mod common;

use common::taida_bin;

fn temp_td(label: &str, content: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "d28b007_{}_{}_{}.td",
        std::process::id(),
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::write(&path, content).expect("write temp .td");
    path
}

fn run_upgrade(path: &PathBuf, extra_args: &[&str]) -> (i32, String, String) {
    let out = Command::new(taida_bin())
        .arg("upgrade")
        .arg("--d28")
        .args(extra_args)
        .arg(path)
        .output()
        .expect("spawn taida upgrade --d28");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Acceptance 1: basic rewrite — `callSign` (string-valued field) renamed
/// to `call_sign` per D28B-001 Lock.
#[test]
fn d28b_007_upgrade_rewrites_string_pack_field() {
    let td = temp_td(
        "rewrite_string",
        "pilot <= @(name <= \"Asuka\", age <= 14, callSign <= \"Eva-02\")\n",
    );
    let (code, stdout, _) = run_upgrade(&td, &[]);
    assert_eq!(code, 0, "stdout: {}", stdout);
    let after = std::fs::read_to_string(&td).expect("read after");
    assert!(after.contains("call_sign <= \"Eva-02\""), "got: {}", after);
    assert!(!after.contains("callSign"), "got: {}", after);
    let _ = std::fs::remove_file(&td);
}

/// Acceptance 2: idempotency — running twice on the same file yields
/// identical content. Second run reports no rewrites needed.
#[test]
fn d28b_007_upgrade_is_idempotent() {
    let td = temp_td("idempotent", "p <= @(callSign <= \"X\")\n");
    let (c1, _, _) = run_upgrade(&td, &[]);
    assert_eq!(c1, 0);
    let after_first = std::fs::read_to_string(&td).unwrap();

    let (c2, stdout2, _) = run_upgrade(&td, &[]);
    assert_eq!(c2, 0);
    let after_second = std::fs::read_to_string(&td).unwrap();
    assert_eq!(after_first, after_second, "second run must be no-op");
    assert!(
        stdout2.contains("All files compliant"),
        "second-run stdout should report compliance, got: {}",
        stdout2
    );
    let _ = std::fs::remove_file(&td);
}

/// Acceptance 3: `--check` mode reports non-zero exit + does NOT modify
/// the source. After --check, the original content is preserved.
#[test]
fn d28b_007_upgrade_check_mode_does_not_modify() {
    let original = "p <= @(callSign <= \"X\")\n";
    let td = temp_td("check_mode", original);
    let (code, stdout, _) = run_upgrade(&td, &["--check"]);
    assert_ne!(
        code, 0,
        "--check should exit non-zero on non-compliant file. stdout: {}",
        stdout
    );
    let after = std::fs::read_to_string(&td).unwrap();
    assert_eq!(after, original, "--check must not modify source");
    let _ = std::fs::remove_file(&td);
}

/// Acceptance 4: bit-identical on no-op — files already compliant are
/// not touched (mtime / content unchanged).
#[test]
fn d28b_007_upgrade_compliant_file_unchanged() {
    let compliant = "p <= @(call_sign <= \"X\", name <= \"Asuka\")\n";
    let td = temp_td("compliant", compliant);
    let pre = std::fs::read_to_string(&td).unwrap();
    let (code, stdout, _) = run_upgrade(&td, &[]);
    assert_eq!(code, 0);
    let post = std::fs::read_to_string(&td).unwrap();
    assert_eq!(pre, post, "compliant file content must not change");
    assert!(
        stdout.contains("All files compliant"),
        "compliant file should report success, got: {}",
        stdout
    );
    let _ = std::fs::remove_file(&td);
}

/// Acceptance 5: function-shape values preserved — Lambda value with
/// camelCase field name (`safeDiv <= _ x y = ...`) is left untouched.
#[test]
fn d28b_007_upgrade_preserves_function_valued_field() {
    let original = "obj <= @(safeDiv <= _ x y = x / y)\n";
    let td = temp_td("function_field", original);
    let (code, _, _) = run_upgrade(&td, &[]);
    assert_eq!(code, 0);
    let after = std::fs::read_to_string(&td).unwrap();
    assert_eq!(after, original, "function-valued field must not be renamed");
    let _ = std::fs::remove_file(&td);
}

/// Acceptance 6: schema-style field declarations are also rewritten.
/// `User = @(callSign: Str, ...)` → `User = @(call_sign: Str, ...)`.
#[test]
fn d28b_007_upgrade_rewrites_schema_field_def() {
    let td = temp_td("schema", "User = @(name: Str, callSign: Str, age: Int)\n");
    let (code, _, _) = run_upgrade(&td, &[]);
    assert_eq!(code, 0);
    let after = std::fs::read_to_string(&td).unwrap();
    assert!(after.contains("call_sign: Str"), "got: {}", after);
    assert!(!after.contains("callSign"), "got: {}", after);
    let _ = std::fs::remove_file(&td);
}

/// Acceptance 7: --dry-run reports rewrites but does not modify the
/// file. Exit code is 0 (success) regardless of whether rewrites would
/// apply (--dry-run is informational).
#[test]
fn d28b_007_upgrade_dry_run_does_not_modify() {
    let original = "p <= @(callSign <= \"X\")\n";
    let td = temp_td("dry_run", original);
    let (code, stdout, _) = run_upgrade(&td, &["--dry-run"]);
    assert_eq!(
        code, 0,
        "--dry-run should exit 0, got: {} stdout={}",
        code, stdout
    );
    let after = std::fs::read_to_string(&td).unwrap();
    assert_eq!(after, original, "--dry-run must not modify source");
    assert!(
        stdout.contains("dry-run"),
        "stdout should mention dry-run, got: {}",
        stdout
    );
    let _ = std::fs::remove_file(&td);
}
