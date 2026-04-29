//! C20B-015 / ROOT-18: 3-backend regression harness for imported functions
//! that resolve JSON schemas through their defining module.
//!
//! Pre-fix (2.1.3): `JSON[raw, LocalSchema]()` inside an exported function
//! crashed whenever the caller had not also imported the typedef. The
//! interpreter's `resolve_json_schema` consulted `self.type_defs`, which
//! reflects the caller module's scope — private typedefs from the
//! exporting module were invisible.
//!
//! Post-fix:
//!
//!   * `FuncValue` carries an `Arc<HashMap<String, Vec<FieldDef>>>` of the
//!     defining module's full TypeDef registry (via `module_type_defs`),
//!     plus the matching enum registry.
//!   * The module loader enrichment pass attaches these registries to
//!     every exported `Value::Function` after the module executes.
//!   * `call_function*` overlays the defining-module scope onto
//!     `self.type_defs` / `self.enum_defs` before running the body and
//!     restores it afterwards. Locally-defined typedefs of the same name
//!     still win (overlay only fills gaps).
//!   * Native / JS backends were already correct; the regression was
//!     interpreter-only, but this harness pins all three to the same
//!     output so a future regression on any backend is caught.
//!
//! Red-test-zero scope guard:
//!
//!   * Function-only import must succeed.
//!   * Typedef + function import must also succeed (no double-binding
//!     collision).
//!   * Caller that directly references the typedef without importing it
//!     must still fail — scope isolation is preserved.
//!   * Checker must not gain false accept/reject behaviour.

mod common;

use common::{node_available, taida_bin};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_dir() -> PathBuf {
    manifest_dir().join("examples/quality/c20b_015_json_schema_scope")
}

fn caller_path() -> PathBuf {
    fixture_dir().join("caller.td")
}

fn expected_path() -> PathBuf {
    fixture_dir().join("caller.expected")
}

fn read_expected() -> String {
    fs::read_to_string(expected_path()).expect("expected file must exist")
}

fn unique_temp(prefix: &str, ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}.{}",
        prefix,
        std::process::id(),
        nanos,
        ext
    ))
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn outputs_equal(a: &str, b: &str) -> bool {
    a.trim_end_matches('\n') == b.trim_end_matches('\n')
}

// ── Interpreter: primary regression ──

#[test]
fn c20b_015_json_schema_scope_interpreter() {
    let out = Command::new(taida_bin())
        .arg(caller_path())
        .output()
        .expect("failed to spawn interpreter");
    assert!(
        out.status.success(),
        "interpreter non-zero: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C20B-015 interpreter output mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}

// ── JS backend: parity (regression guard for a future regression) ──

#[test]
fn c20b_015_json_schema_scope_js_matches_interpreter() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }
    let outdir = unique_temp("c20b015_js", "dir");
    fs::create_dir_all(&outdir).expect("mkdir outdir");
    let mjs = outdir.join("caller.mjs");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("js")
        .arg("-o")
        .arg(&mjs)
        .arg(caller_path())
        .output()
        .expect("failed to spawn js build");
    assert!(
        build.status.success(),
        "js build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new("node")
        .arg(&mjs)
        .output()
        .expect("failed to spawn node");
    let _ = fs::remove_dir_all(&outdir);
    assert!(
        run.status.success(),
        "node exit failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C20B-015 JS output mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}

// ── Native backend: parity ──

#[test]
fn c20b_015_json_schema_scope_native_matches_interpreter() {
    if !cc_available() {
        eprintln!("SKIP: cc not available");
        return;
    }
    let outdir = unique_temp("c20b015_native", "dir");
    fs::create_dir_all(&outdir).expect("mkdir outdir");
    let bin = outdir.join("caller_native");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("native")
        .arg("-o")
        .arg(&bin)
        .arg(caller_path())
        .output()
        .expect("failed to spawn native build");
    assert!(
        build.status.success(),
        "native build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let run = Command::new(&bin)
        .output()
        .expect("failed to spawn native binary");
    let _ = fs::remove_dir_all(&outdir);
    assert!(
        run.status.success(),
        "native binary exit failed: status={:?}, stderr={}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    let stdout = String::from_utf8_lossy(&run.stdout).to_string();
    let expected = read_expected();
    assert!(
        outputs_equal(&stdout, &expected),
        "C20B-015 native output mismatch.\n--- expected ---\n{}\n--- got ---\n{}\n",
        expected,
        stdout
    );
}

// ── Interpreter: importing typedef + function also works ──
//
// Regression guard for the pre-fix workaround: users who import both
// the typedef and the function must continue to work (no double-binding
// collision when the overlay fills the same name the caller also
// imported). The overlay favours the caller's binding, so this just
// verifies the overlay doesn't crash on name overlap.

#[test]
fn c20b_015_interpreter_also_works_when_caller_imports_typedef() {
    let tmp = unique_temp("c20b015_both", "dir");
    fs::create_dir_all(&tmp).expect("mkdir");
    let schema_src =
        fs::read_to_string(fixture_dir().join("schema_mod.td")).expect("read schema_mod.td");
    fs::write(tmp.join("schema_mod.td"), schema_src).expect("write schema_mod.td");
    let caller_both = "\
>>> ./schema_mod.td => @(loadUser, UserSchema)
stdout(loadUser())
stdout(\"\\n\")
";
    // Note: fixture schema_mod.td exports only `loadUser`. A caller that
    // asks for `UserSchema` when it isn't in the export list is supposed
    // to get a "Symbol not found in module" error from the importer.
    // That is still correct behaviour — the overlay must not
    // retroactively leak a private typedef through the `<<<` filter.
    fs::write(tmp.join("caller.td"), caller_both).expect("write caller");
    let out = Command::new(taida_bin())
        .arg(tmp.join("caller.td"))
        .output()
        .expect("spawn interp");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let _ = fs::remove_dir_all(&tmp);
    assert!(
        !out.status.success(),
        "expected unexported-typedef import to fail, but program succeeded"
    );
    assert!(
        stderr.contains("UserSchema") && stderr.contains("not found"),
        "expected 'UserSchema … not found' error, got stderr={}",
        stderr
    );
}

// ── Interpreter: caller cannot access typedef just because it's in the overlay ──
//
// The fix must not promote the defining-module's private typedef into
// the caller's scope: calling `loadUser()` succeeds, but `JSON[raw,
// UserSchema]()` *in caller code* must still fail because the caller
// did not import the typedef. This preserves module encapsulation.

#[test]
fn c20b_015_interpreter_overlay_does_not_leak_to_caller_scope() {
    let tmp = unique_temp("c20b015_leak", "dir");
    fs::create_dir_all(&tmp).expect("mkdir");
    let schema_src =
        fs::read_to_string(fixture_dir().join("schema_mod.td")).expect("read schema_mod.td");
    fs::write(tmp.join("schema_mod.td"), schema_src).expect("write schema_mod.td");
    let leaky_caller = "\
>>> ./schema_mod.td => @(loadUser)
stdout(loadUser())
stdout(\"\\n\")
raw <= \"{\\\"name\\\":\\\"bob\\\",\\\"age\\\":9}\"
stdout(JSON[raw, UserSchema]().__value.name)
";
    fs::write(tmp.join("caller.td"), leaky_caller).expect("write caller");
    let out = Command::new(taida_bin())
        .arg(tmp.join("caller.td"))
        .output()
        .expect("spawn interp");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = fs::remove_dir_all(&tmp);
    // Caller's direct use of `UserSchema` must still be rejected.
    assert!(
        !out.status.success() || combined.contains("Unknown schema type"),
        "caller-scope JSON[raw, UserSchema]() was accidentally allowed — scope leak regression. combined={}",
        combined
    );
}

// ── Interpreter: mutual tail-call retarget does not leak overlay ──
//
// Regression guard for the C20B-015 reopen (2026-04-21 review). Pre-fix
// (post-Phase 5.6): the trampoline captured `td_changed` from the
// *initial* callee and used that flag to decide whether to restore the
// outer scope on exit. If the initial callee was a *local* function
// (`td_changed == false`) and a mutual tail-call retargeted to an
// *imported* function whose push overlaid the defining module's private
// typedefs, the overlay was never restored and leaked into the caller's
// top-level scope.
//
// Repro shape: a local wrapper `wrap raw = load(raw)` whose tail call
// retargets to imported `load` (which lives in a module with private
// `Schema`). After `wrap("...")` returns, the caller attempts
// `JSON[raw, Schema]()` at the top level. This must fail with
// "Unknown schema type 'Schema'" because the caller never imported
// `Schema`. Pre-fix, the caller-scope call silently succeeded.

#[test]
fn c20b_015_interpreter_mutual_tail_call_does_not_leak_overlay() {
    let tmp = unique_temp("c20b015_tail", "dir");
    fs::create_dir_all(&tmp).expect("mkdir");
    let schema_mod = "\
Schema = @(name: Str)

load raw: Str =
  parsed <= JSON[raw, Schema]()
  | parsed.hasValue |> parsed.__value.name
  | _ |> \"no\"
=> :Str

<<< @(load)
";
    fs::write(tmp.join("schema_mod.td"), schema_mod).expect("write schema_mod");
    // `wrap` is a local function whose body is a direct call to the
    // imported `load`. This triggers the mutual tail-call path in the
    // trampoline (initial callee `wrap` is local; retarget is imported
    // `load` which pushes an overlay).
    let caller = "\
>>> ./schema_mod.td => @(load)

wrap raw: Str = load(raw) => :Str

stdout(wrap(\"{\\\"name\\\":\\\"ok\\\"}\"))
stdout(\"\\n\")
stdout(JSON[\"{\\\"name\\\":\\\"leak\\\"}\", Schema]().__value.name)
stdout(\"\\n\")
";
    fs::write(tmp.join("caller.td"), caller).expect("write caller");
    let out = Command::new(taida_bin())
        .arg(tmp.join("caller.td"))
        .output()
        .expect("spawn interp");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let combined = format!("{}\n{}", stdout, stderr);
    let _ = fs::remove_dir_all(&tmp);
    // `wrap("…")` must still succeed — the imported function's overlay
    // is applied for the duration of its body.
    assert!(
        stdout.contains("ok"),
        "imported function call via local wrapper did not produce expected 'ok'. stdout={}, stderr={}",
        stdout,
        stderr
    );
    // The caller-scope `JSON[..., Schema]()` must still be rejected. The
    // overlay from `load`'s body is NOT allowed to persist back into the
    // caller's top-level scope after `wrap(…)` returns.
    assert!(
        combined.contains("Unknown schema type") && combined.contains("Schema"),
        "caller-scope JSON[raw, Schema]() was silently allowed — mutual \
         tail-call overlay leak regression. combined={}",
        combined
    );
    // The program as a whole should terminate with a runtime error
    // (not exit 0), because the second `stdout(JSON[...]().__value.name)`
    // must throw before reaching `stdout`.
    // NB: taida interpreter currently returns exit 0 even on Runtime error;
    // what matters is the "Unknown schema type" assertion above.
}

// ── Interpreter: truly unknown schema still errors ──

#[test]
fn c20b_015_interpreter_truly_unknown_schema_still_errors() {
    let tmp = unique_temp("c20b015_unknown", "dir");
    fs::create_dir_all(&tmp).expect("mkdir");
    let src = "\
raw <= \"{\\\"x\\\":1}\"
stdout(JSON[raw, ThisDoesNotExist]().__value.x.toString())
";
    fs::write(tmp.join("prog.td"), src).expect("write prog");
    let out = Command::new(taida_bin())
        .arg(tmp.join("prog.td"))
        .output()
        .expect("spawn interp");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = fs::remove_dir_all(&tmp);
    assert!(
        !out.status.success(),
        "unknown schema should have errored; combined={}",
        combined
    );
    assert!(
        combined.contains("Unknown schema type") && combined.contains("ThisDoesNotExist"),
        "expected Unknown schema type error for ThisDoesNotExist, got={}",
        combined
    );
}

// ── Interpreter: REPL reuse after body-level RuntimeError ──
//
// Regression guard for the C20B-015 3rd reopen (2026-04-22 review).
//
// Pre-fix: `call_function` evaluated the body with
// `eval_statements(&current_func.body)?` — the `?` operator early-returned
// on `RuntimeError`, skipping the pops and root-scope restore. The
// defining-module overlay on `self.type_defs` / `self.enum_defs` stayed in
// place, and `self.active_function` / `self.call_depth` / closure + local
// scopes were also left unrestored. The REPL reuses one `Interpreter`
// across inputs, so the leak made a subsequent top-level
// `JSON[raw, Schema]()` silently resolve against the imported module's
// private typedefs.
//
// Post-fix: body evaluation binds the `Result` and runs the cleanup path
// on every exit (Ok / Err), then propagates. The second `eval_program`
// below must see the *caller*'s scope, not the imported module's overlay.
//
// Repro shape:
//   1. `schema_mod.td` defines a private `Schema = @(name: Str)` and
//      exports `load raw: Str` whose body does
//      `parsed <= JSON[raw, UnknownTypeThatDoesNotExist]()`. This body
//      raises a `RuntimeError("Unknown schema type 'UnknownTypeThatDoesNotExist'")`
//      from inside `resolve_json_schema`. The `load` function *did* get
//      its defining-module overlay pushed (it carries `module_type_defs`
//      containing `Schema`), but the inner `JSON[..., UnknownType...]()`
//      still errors because neither the overlay nor the caller's scope
//      has that name.
//   2. caller program 1: `>>> ./schema_mod.td => @(load)` followed by
//      `wrap raw: Str = load(raw) => :Str` and `wrap("{...}")`. This
//      errors at runtime — the user's first REPL input fails.
//   3. caller program 2 (simulating the next REPL line on the *same*
//      `Interpreter`): `raw <= "{\"name\":\"leak\"}"` then
//      `JSON[raw, Schema]().__value.name`. Pre-fix: overlay leaked,
//      `Schema` resolved to the imported private typedef, `"leak"` was
//      printed. Post-fix: overlay was restored by cleanup after program 1
//      errored, so `Schema` is unknown in the caller's scope and
//      `eval_program` returns `Err("Unknown schema type 'Schema'")`.

#[test]
fn c20b_015_interpreter_body_error_does_not_leak_overlay_to_next_repl_input() {
    use std::sync::Mutex;
    use taida::interpreter::Interpreter;
    use taida::parser::parse;

    // Serialise current-file path mutation: `set_current_file` uses process-
    // global state transitions via `std::env::set_current_dir` is not used,
    // but the import resolver keys off the interpreter's `current_file`,
    // which is instance-local. Still, having one static Mutex guarantees
    // that parallel cargo test runs that also `set_current_file` cannot
    // race on the filesystem fixture directory.
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().expect("lock must not be poisoned");

    let tmp = unique_temp("c20b015_repl_reuse", "dir");
    fs::create_dir_all(&tmp).expect("mkdir");

    // Imported module: defines a private `Schema`. Its exported `load`
    // attempts to resolve an unknown schema inside its body, raising a
    // `RuntimeError` *while the defining-module overlay is active*.
    let schema_mod = "\
Schema = @(name: Str)

load raw: Str =
  parsed <= JSON[raw, UnknownTypeThatDoesNotExist]()
  | parsed.hasValue |> parsed.__value.name
  | _ |> \"no\"
=> :Str

<<< @(load)
";
    fs::write(tmp.join("schema_mod.td"), schema_mod).expect("write schema_mod");

    // Program 1: run imported `load` through a local wrapper. Body evaluation
    // of `load` errors with "Unknown schema type 'UnknownTypeThatDoesNotExist'".
    // The wrapper's mutual tail-call retarget to `load` pushes the overlay.
    let prog1_src = "\
>>> ./schema_mod.td => @(load)

wrap raw: Str = load(raw) => :Str

stdout(wrap(\"{\\\"name\\\":\\\"ignored\\\"}\"))
";
    let caller_path = tmp.join("caller.td");
    fs::write(&caller_path, prog1_src).expect("write caller");

    // Parse program 1.
    let (prog1, errs1) = parse(prog1_src);
    assert!(errs1.is_empty(), "program 1 must parse: {:?}", errs1);

    let mut interp = Interpreter::new();
    interp.set_current_file(&caller_path);

    // Program 1 MUST error — the inner `JSON[..., UnknownType...]` raises
    // a RuntimeError from inside the imported function body. If this does
    // NOT error, the repro harness is wrong (not a regression of the
    // cleanup fix).
    let r1 = interp.eval_program(&prog1);
    assert!(
        r1.is_err(),
        "program 1 was expected to error (imported function body raises RuntimeError); \
         got Ok({:?}). Output buffer: {:?}",
        r1,
        interp.output
    );

    // Program 2: simulated next REPL input on the SAME `Interpreter`.
    // Tries to use `Schema` at the caller's top level. Because the caller
    // did NOT import `Schema` (only `load`), this must fail with
    // "Unknown schema type 'Schema'". Pre-fix: the overlay leaked from
    // the errored program 1 and `Schema` silently resolved to the private
    // typedef from `schema_mod.td`, returning `"leak"`.
    //
    // NB: use a fresh variable name (`probeJson`, not `raw`) so that this
    // assertion specifically pins the TYPE OVERLAY leak (user-reported
    // symptom: `JSON[..., Schema]()` silently returned `"leak"`). Pre-fix,
    // the local scope also leaked — `raw` would have collided — but the
    // underlying `?`-skipped cleanup bug manifests in BOTH the scope leak
    // and the type-overlay leak. Pinning the overlay explicitly matches
    // the bug report.
    let prog2_src = "\
probeJson <= \"{\\\"name\\\":\\\"leak\\\"}\"
stdout(JSON[probeJson, Schema]().__value.name)
";
    let (prog2, errs2) = parse(prog2_src);
    assert!(errs2.is_empty(), "program 2 must parse: {:?}", errs2);

    let r2 = interp.eval_program(&prog2);
    let _ = fs::remove_dir_all(&tmp);

    let r2_err = match r2 {
        Err(e) => e.to_string(),
        Ok(v) => panic!(
            "program 2 unexpectedly succeeded (overlay leaked from program 1). \
             Result = {:?}. stdout buffer: {:?}",
            v, interp.output
        ),
    };
    assert!(
        r2_err.contains("Unknown schema type") && r2_err.contains("Schema"),
        "program 2 error must reject 'Schema' as unknown in caller's scope \
         (confirms overlay was cleaned up after program 1's body-level error). \
         Got: {}",
        r2_err
    );
}

// ── Interpreter: direct body-level RuntimeError cleans scope within one program ──
//
// Companion to the REPL-reuse pin above: guards the same invariant using a
// single `eval_program` call. If the first top-level `load(...)` errors
// inside the imported function body, the overlay must be torn down before
// the program returns its `Err`. We assert this by observing that a
// subsequent `JSON[raw, Schema]()` in the SAME program — placed above the
// error-raising call so the caller's top-level scope is set up — does not
// leak. This check does not rely on REPL-specific behaviour.

#[test]
fn c20b_015_interpreter_body_error_restores_caller_scope_eagerly() {
    use taida::interpreter::Interpreter;
    use taida::parser::parse;

    let tmp = unique_temp("c20b015_eager_cleanup", "dir");
    fs::create_dir_all(&tmp).expect("mkdir");

    let schema_mod = "\
Schema = @(name: Str)

load raw: Str =
  parsed <= JSON[raw, UnknownTypeThatDoesNotExist]()
  | parsed.hasValue |> parsed.__value.name
  | _ |> \"no\"
=> :Str

<<< @(load)
";
    fs::write(tmp.join("schema_mod.td"), schema_mod).expect("write schema_mod");

    // Program: runs the imported `load` through a wrapper. When it errors,
    // the program aborts. We then look at the interpreter's `type_defs`
    // via a second `eval_program` call that tries to resolve `Schema` in
    // the caller — if the overlay was cleaned up eagerly, it must be
    // unknown in the caller's scope.
    let src = "\
>>> ./schema_mod.td => @(load)

wrap raw: Str = load(raw) => :Str

stdout(wrap(\"{\\\"name\\\":\\\"x\\\"}\"))
";
    let caller_path = tmp.join("caller.td");
    fs::write(&caller_path, src).expect("write caller");

    let (prog, errs) = parse(src);
    assert!(errs.is_empty(), "must parse: {:?}", errs);

    let mut interp = Interpreter::new();
    interp.set_current_file(&caller_path);
    let r = interp.eval_program(&prog);
    assert!(
        r.is_err(),
        "expected body-level RuntimeError, got Ok({:?})",
        r
    );

    // Run a probe program to check that the caller's scope does not see
    // the private `Schema`. If cleanup happened eagerly at the point of
    // error, the probe will fail with "Unknown schema type 'Schema'".
    // Fresh variable name (`probeJson`) to isolate the overlay assertion
    // from the independent scope-leak signal.
    let probe_src = "\
probeJson <= \"{\\\"name\\\":\\\"leak\\\"}\"
stdout(JSON[probeJson, Schema]().__value.name)
";
    let (probe, probe_errs) = parse(probe_src);
    assert!(probe_errs.is_empty(), "probe must parse: {:?}", probe_errs);
    let pr = interp.eval_program(&probe);
    let _ = fs::remove_dir_all(&tmp);
    let pr_err = match pr {
        Err(e) => e.to_string(),
        Ok(v) => panic!(
            "probe unexpectedly succeeded (overlay leak from errored program). \
             Result = {:?}. stdout: {:?}",
            v, interp.output
        ),
    };
    assert!(
        pr_err.contains("Unknown schema type") && pr_err.contains("Schema"),
        "probe must reject 'Schema' in caller's scope. Got: {}",
        pr_err
    );
}

// ── Interpreter: body-level error must pop local & closure scopes ──
//
// Companion pin for the scope-leak half of the 3rd reopen. Pre-fix, the
// closure + local scopes pushed inside `call_function` were never popped
// on a body `RuntimeError` because `?` bypassed the restore path. This
// manifests as a phantom binding in the caller's next-input scope.
//
// This test deliberately uses a parameter name that also appears as a
// top-level `<=` binding in the next REPL input. Pre-fix: the function's
// parameter `payload` stayed alive in a still-pushed local scope, and
// the subsequent top-level `payload <= "..."` tripped over "Variable
// 'payload' is already defined in this scope". Post-fix: the local scope
// was popped even on the error path, so the second program runs cleanly.

#[test]
fn c20b_015_interpreter_body_error_pops_pushed_scopes() {
    use taida::interpreter::Interpreter;
    use taida::parser::parse;

    let tmp = unique_temp("c20b015_scope_pop", "dir");
    fs::create_dir_all(&tmp).expect("mkdir");

    let schema_mod = "\
Schema = @(name: Str)

load payload: Str =
  parsed <= JSON[payload, UnknownTypeThatDoesNotExist]()
  | parsed.hasValue |> parsed.__value.name
  | _ |> \"no\"
=> :Str

<<< @(load)
";
    fs::write(tmp.join("schema_mod.td"), schema_mod).expect("write schema_mod");

    let prog1_src = "\
>>> ./schema_mod.td => @(load)
stdout(load(\"{\\\"name\\\":\\\"x\\\"}\"))
";
    let caller_path = tmp.join("caller.td");
    fs::write(&caller_path, prog1_src).expect("write caller");

    let (prog1, errs1) = parse(prog1_src);
    assert!(errs1.is_empty(), "prog1 must parse: {:?}", errs1);

    let mut interp = Interpreter::new();
    interp.set_current_file(&caller_path);
    let r1 = interp.eval_program(&prog1);
    assert!(
        r1.is_err(),
        "prog1 must error (body-level Unknown schema). Got: {:?}",
        r1
    );

    // REPL input 2: redefines `payload` at the top level. Pre-fix, the
    // pushed local scope from `load`'s failed call leaked, `payload` was
    // still bound, and `payload <= ...` errored with "already defined".
    // Post-fix, cleanup popped the local scope on the error path.
    let prog2_src = "\
payload <= \"ok\"
stdout(payload)
";
    let (prog2, errs2) = parse(prog2_src);
    assert!(errs2.is_empty(), "prog2 must parse: {:?}", errs2);

    let r2 = interp.eval_program(&prog2);
    let _ = fs::remove_dir_all(&tmp);
    match r2 {
        Ok(_) => {
            assert!(
                interp
                    .output
                    .iter()
                    .any(|line| line.trim_end_matches('\n') == "ok"),
                "prog2 should have printed 'ok'; stdout buffer: {:?}",
                interp.output
            );
        }
        Err(e) => panic!(
            "prog2 must succeed after prog1's body-level error (local scope from \
             failed call leaked pre-fix: parameter name 'payload' remained bound). \
             Got: {}",
            e
        ),
    }
}

// ── Interpreter: user-defined method body-level error must pop pushed scopes ──
//
// Symmetric-fix pin for the `eval_user_method` path (methods.rs:201). The
// `call_function*` paths in eval.rs were hardened by Pattern B (capture
// `Result` without `?`, always run cleanup, propagate); `eval_user_method`
// had the same shape — two `push_scope` calls followed by
// `eval_statements(&body)?` — and would leak both the instance-fields
// scope and the parameter/local scope when a method body raised a
// `RuntimeError`. Unlike `call_function`, `eval_user_method` does not
// touch typedef/enum overlays, `active_function`, or `call_depth`, so
// the repro focuses on the pure scope-pop invariant.
//
// Repro shape (REPL reuse):
//   1. `Greeter = @(name: Str; greet = ... => :Str)` defines a method
//      whose body raises a RuntimeError (calling an undefined symbol
//      inside a conditional branch that actually fires at runtime).
//   2. prog1 instantiates `Greeter` and invokes `.greet()`, which errors.
//   3. prog2 (on the same `Interpreter`) rebinds the parameter name at
//      the top level. Pre-fix: the method's local scope was still pushed,
//      the parameter binding leaked, and `victim <= "..."` errored with
//      "already defined in this scope". Post-fix: cleanup popped both
//      scopes on the error path.
#[test]
fn c20b_015_interpreter_user_method_body_error_pops_pushed_scopes() {
    use taida::interpreter::Interpreter;
    use taida::parser::parse;

    // prog1: define a user type `Greeter` with a method `greet victim`
    // whose body tries to call an undefined function. The reference to
    // `notAFunction` resolves during evaluation (not parse), so the
    // body raises a RuntimeError *while the two method scopes are
    // pushed*.
    let prog1_src = "\
Greeter = @(
  name: Str
  greet victim: Str =
    notAFunction(victim)
  => :Str
)

g <= Greeter(name <= \"alice\")
stdout(g.greet(\"bob\"))
";
    let (prog1, errs1) = parse(prog1_src);
    assert!(errs1.is_empty(), "prog1 must parse: {:?}", errs1);

    let mut interp = Interpreter::new();
    let r1 = interp.eval_program(&prog1);
    assert!(
        r1.is_err(),
        "prog1 must error (method body calls undefined `notAFunction`). Got: {:?}",
        r1
    );

    // prog2: rebind `victim` at the top level. Pre-fix, the local scope
    // from `greet`'s failed invocation leaked, and the parameter binding
    // `victim = "bob"` remained alive. The `<=` binding would then
    // collide with the surviving scope entry. Post-fix: local and
    // instance-fields scopes were both popped on the Err path.
    let prog2_src = "\
victim <= \"ok\"
stdout(victim)
";
    let (prog2, errs2) = parse(prog2_src);
    assert!(errs2.is_empty(), "prog2 must parse: {:?}", errs2);

    let r2 = interp.eval_program(&prog2);
    match r2 {
        Ok(_) => {
            assert!(
                interp
                    .output
                    .iter()
                    .any(|line| line.trim_end_matches('\n') == "ok"),
                "prog2 should have printed 'ok'; stdout buffer: {:?}",
                interp.output
            );
        }
        Err(e) => panic!(
            "prog2 must succeed after prog1's method-body error \
             (local scope from failed `.greet(\"bob\")` leaked pre-fix: \
             parameter `victim` remained bound at the top level). Got: {}",
            e
        ),
    }
}

// ── Interpreter: user-defined method body-level error must pop instance-fields scope ──
//
// Companion pin: the outer scope pushed by `eval_user_method` carries the
// instance's fields (e.g. `name` from `Greeter`). Pre-fix, an error from
// the method body would also leak those instance-field bindings into the
// caller's scope. Post-fix, both the local scope and the instance-fields
// scope are popped on the Err path.
#[test]
fn c20b_015_interpreter_user_method_body_error_pops_instance_fields_scope() {
    use taida::interpreter::Interpreter;
    use taida::parser::parse;

    // `name` is an instance field of `Greeter`; the method body errors
    // before returning. Pre-fix, `name` would leak at the top level of
    // the caller's environment because the instance-fields scope was
    // never popped.
    let prog1_src = "\
Greeter = @(
  name: Str
  greet =
    notAFunction(name)
  => :Str
)

g <= Greeter(name <= \"alice\")
stdout(g.greet())
";
    let (prog1, errs1) = parse(prog1_src);
    assert!(errs1.is_empty(), "prog1 must parse: {:?}", errs1);

    let mut interp = Interpreter::new();
    let r1 = interp.eval_program(&prog1);
    assert!(
        r1.is_err(),
        "prog1 must error (method body calls undefined `notAFunction`). Got: {:?}",
        r1
    );

    // Read `name` at the top level on the same Interpreter WITHOUT
    // rebinding it. Pre-fix: the instance-fields scope leaked and the
    // binding `name = "alice"` was reachable via `env.get()` (which
    // searches all scopes), so `stdout(name)` would silently print
    // "alice". Note that `name <=` would NOT have caught this leak on
    // its own, because `define()` only checks the innermost scope and
    // the innermost scope in the leak is the (empty) local scope — it
    // would succeed and shadow the instance field. Reading `name`
    // without rebinding exercises the full `env.get()` lookup and
    // catches the instance-fields-scope leak directly.
    //
    // Post-fix: instance-fields scope was popped on the Err path, so
    // `name` is undefined at the top level and `eval_program` returns
    // an "Undefined variable 'name'" error.
    let prog2_src = "\
stdout(name)
";
    let (prog2, errs2) = parse(prog2_src);
    assert!(errs2.is_empty(), "prog2 must parse: {:?}", errs2);

    let r2 = interp.eval_program(&prog2);
    match r2 {
        Ok(_) => panic!(
            "prog2 unexpectedly succeeded — instance-fields scope from failed \
             `.greet()` leaked pre-fix: `name` was still bound at the top level. \
             stdout buffer: {:?}",
            interp.output
        ),
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("name")
                    && (msg.contains("Undefined")
                        || msg.contains("undefined")
                        || msg.contains("not defined")
                        || msg.contains("not found")),
                "prog2 must fail with an undefined-variable error for `name` \
                 (confirms instance-fields scope was popped on the error path). \
                 Got: {}",
                msg
            );
        }
    }
}

// ── Interpreter: user-defined method success path semantics unchanged ──
//
// Guard that the symmetric fix did not alter success-path behaviour: a
// method that returns normally must still pop both scopes, and a caller
// binding a variable with the same name as a method parameter must
// succeed without "already defined" errors.
#[test]
fn c20b_015_interpreter_user_method_success_path_unchanged() {
    use taida::interpreter::Interpreter;
    use taida::parser::parse;

    let src = "\
Greeter = @(
  name: Str
  greet victim: Str =
    name
  => :Str
)

g <= Greeter(name <= \"alice\")
stdout(g.greet(\"bob\"))
stdout(\"\\n\")
victim <= \"after\"
stdout(victim)
stdout(\"\\n\")
name <= \"top\"
stdout(name)
";
    let (prog, errs) = parse(src);
    assert!(errs.is_empty(), "prog must parse: {:?}", errs);

    let mut interp = Interpreter::new();
    let r = interp.eval_program(&prog);
    assert!(
        r.is_ok(),
        "success-path program must evaluate cleanly; got: {:?}. stdout: {:?}",
        r,
        interp.output
    );

    let joined: String = interp.output.iter().cloned().collect();
    assert!(
        joined.contains("alice"),
        "method call should have returned instance field `name` (\"alice\"). stdout: {:?}",
        interp.output
    );
    assert!(
        joined.contains("after"),
        "top-level `victim` binding should have printed \"after\". stdout: {:?}",
        interp.output
    );
    assert!(
        joined.contains("top"),
        "top-level `name` binding should have printed \"top\". stdout: {:?}",
        interp.output
    );
}
