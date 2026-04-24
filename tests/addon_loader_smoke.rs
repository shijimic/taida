//! RC1 Phase 2 -- Native addon loader smoke test.
//!
//! End-to-end verification that the host loader can:
//!
//! 1. `dlopen` the real `libtaida_addon_sample.so` cdylib produced by
//!    the `taida-addon-sample` workspace crate.
//! 2. Resolve the frozen `taida_addon_get_v1` entry symbol.
//! 3. Pass the ABI handshake.
//! 4. Enumerate the sample's two-function table (`noop`, `echo`).
//! 5. Invoke `noop` through the raw call pointer with arity 0.
//!
//! This test exercises the `dlopen` path that unit tests deliberately
//! skip (unit tests in `src/addon/loader.rs` use in-process descriptors
//! to keep validation paths hermetic).
//!
//! ## Behaviour when the cdylib is missing
//!
//! Cargo builds the workspace's binaries lazily. If `cargo test` is
//! invoked in a way that does not produce `libtaida_addon_sample.so`
//! (for example a cross-compile harness or a freshly cleaned target
//! directory in a feature-stripped build), this test prints a `note:`
//! and returns successfully rather than failing the build. The hard
//! checks in `src/addon/loader.rs` already cover the loader logic;
//! this file's job is the *integration* hop, not gating CI on a
//! particular target layout.

#![cfg(feature = "native")]

use std::path::PathBuf;

use taida::addon::abi_crate as taida_addon;
use taida::addon::call::AddonCallError;
use taida::addon::loader::{AddonLoadError, load_addon};
use taida::addon::{TAIDA_ADDON_ABI_VERSION, TAIDA_ADDON_ENTRY_SYMBOL};
use taida::interpreter::value::Value;

/// Locate `libtaida_addon_sample.so` in the workspace target directory.
///
/// Tries (in order): `target/debug/`, `target/release/`,
/// `target/debug/deps/`, `target/release/deps/`.
fn find_sample_cdylib() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Honor CARGO_TARGET_DIR if set.
    let target_root = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("target"));

    let lib_name = if cfg!(target_os = "linux") {
        "libtaida_addon_sample.so"
    } else if cfg!(target_os = "macos") {
        "libtaida_addon_sample.dylib"
    } else if cfg!(target_os = "windows") {
        "taida_addon_sample.dll"
    } else {
        return None;
    };

    let candidates = [
        target_root.join("debug").join(lib_name),
        target_root.join("release").join(lib_name),
        target_root.join("debug").join("deps").join(lib_name),
        target_root.join("release").join("deps").join(lib_name),
    ];

    candidates.into_iter().find(|p| p.exists())
}

#[test]
fn loads_sample_addon_and_enumerates_function_table() {
    let path = match find_sample_cdylib() {
        Some(p) => p,
        None => {
            eprintln!(
                "note: skipping addon_loader_smoke -- libtaida_addon_sample.{{so,dylib,dll}} not found in target/. \
                 Run `cargo build -p taida-addon-sample` first if you want this test to execute."
            );
            return;
        }
    };

    let addon = match load_addon(&path) {
        Ok(a) => a,
        Err(e) => panic!(
            "load_addon({}) failed: {e} (this is the RC1 Phase 2 smoke test -- if the path exists \
             and the cdylib was built by this workspace, the loader has a regression)",
            path.display()
        ),
    };

    assert_eq!(addon.path(), path.as_path());
    assert_eq!(addon.abi_version(), TAIDA_ADDON_ABI_VERSION);
    assert_eq!(addon.name(), "taida-lang/addon-rs-sample");
    assert_eq!(addon.function_count(), 2);

    // Enumerate the function table. Order is preserved from the
    // sample addon's `SAMPLE_FUNCTIONS` slice.
    let names: Vec<(String, u32)> = addon
        .functions()
        .map(|f| (f.name().to_string(), f.arity()))
        .collect();
    assert_eq!(
        names,
        vec![("noop".to_string(), 0u32), ("echo".to_string(), 1u32)]
    );

    // find_function lookup.
    let noop = addon.find_function("noop").expect("noop must exist");
    assert_eq!(noop.arity(), 0);
    assert!(addon.find_function("does_not_exist").is_none());

    // Invoke `noop` through the raw call pointer with arity 0. The
    // sample addon's `noop` accepts zero args and returns Ok.
    let status = (noop.raw_call())(
        core::ptr::null(),
        0,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    );
    assert_eq!(status, taida_addon::TaidaAddonStatus::Ok);

    // `echo` with bad arity must produce ArityMismatch through the
    // raw call pointer -- this proves we wired the function pointer
    // correctly.
    let echo = addon.find_function("echo").expect("echo must exist");
    let status = (echo.raw_call())(
        core::ptr::null(),
        0,
        core::ptr::null_mut(),
        core::ptr::null_mut(),
    );
    assert_eq!(status, taida_addon::TaidaAddonStatus::ArityMismatch);
}

// ── Phase 3 value-bridge round-trip tests ───────────────────────────

/// Shared helper: load the sample addon once per test. Returns `None`
/// if the cdylib hasn't been built.
fn load_sample_addon() -> Option<taida::addon::LoadedAddon> {
    let path = find_sample_cdylib()?;
    match load_addon(&path) {
        Ok(a) => Some(a),
        Err(e) => panic!("load_addon({}) failed: {e}", path.display()),
    }
}

#[test]
fn echo_round_trips_int() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!("note: skipping echo_round_trips_int -- sample cdylib not built");
            return;
        }
    };
    let result = addon
        .call_function("echo", &[Value::Int(12345)])
        .expect("echo(Int) must succeed");
    match result {
        Value::Int(n) => assert_eq!(n, 12345),
        other => panic!("expected Int(12345), got {other:?}"),
    }
}

#[test]
fn echo_round_trips_float() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!("note: skipping echo_round_trips_float -- sample cdylib not built");
            return;
        }
    };
    let result = addon
        .call_function("echo", &[Value::Float(-12.5e3)])
        .expect("echo(Float) must succeed");
    match result {
        Value::Float(f) => assert_eq!(f, -12.5e3),
        other => panic!("expected Float, got {other:?}"),
    }
}

#[test]
fn echo_round_trips_bool() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!("note: skipping echo_round_trips_bool -- sample cdylib not built");
            return;
        }
    };
    let r1 = addon
        .call_function("echo", &[Value::Bool(true)])
        .expect("echo(true) must succeed");
    assert!(matches!(r1, Value::Bool(true)));
    let r2 = addon
        .call_function("echo", &[Value::Bool(false)])
        .expect("echo(false) must succeed");
    assert!(matches!(r2, Value::Bool(false)));
}

#[test]
fn echo_round_trips_str() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!("note: skipping echo_round_trips_str -- sample cdylib not built");
            return;
        }
    };
    let input = "Taida Lang — AI 協業時代のプログラミング言語".to_string();
    let result = addon
        .call_function("echo", &[Value::str(input.clone())])
        .expect("echo(Str) must succeed");
    match result {
        Value::Str(s) => assert_eq!(s.as_str(), input),
        other => panic!("expected Str, got {other:?}"),
    }
}

#[test]
fn echo_round_trips_bytes() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!("note: skipping echo_round_trips_bytes -- sample cdylib not built");
            return;
        }
    };
    let input = vec![0x00u8, 0x01, 0xff, 0x7f, 0x42];
    let result = addon
        .call_function("echo", &[Value::bytes(input.clone())])
        .expect("echo(Bytes) must succeed");
    match result {
        Value::Bytes(b) => assert_eq!(&**b, &input),
        other => panic!("expected Bytes, got {other:?}"),
    }
}

#[test]
fn echo_round_trips_unit() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!("note: skipping echo_round_trips_unit -- sample cdylib not built");
            return;
        }
    };
    let result = addon
        .call_function("echo", &[Value::Unit])
        .expect("echo(Unit) must succeed");
    assert!(matches!(result, Value::Unit));
}

#[test]
fn echo_round_trips_nested_list() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!("note: skipping echo_round_trips_nested_list -- sample cdylib not built");
            return;
        }
    };
    let input = Value::list(vec![
        Value::Int(1),
        Value::str("two".to_string()),
        Value::list(vec![Value::Bool(true), Value::Float(2.5)]),
    ]);
    let result = addon
        .call_function("echo", std::slice::from_ref(&input))
        .expect("echo(List) must succeed");
    // Value lacks PartialEq, so compare via Debug — it still pins
    // the full recursive structure.
    assert_eq!(format!("{result:?}"), format!("{input:?}"));
}

#[test]
fn echo_round_trips_buchi_pack() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!("note: skipping echo_round_trips_buchi_pack -- sample cdylib not built");
            return;
        }
    };
    let input = Value::pack(vec![
        ("name".to_string(), Value::str("Taida".to_string())),
        ("version".to_string(), Value::Int(2)),
        (
            "tags".to_string(),
            Value::list(vec![
                Value::str("alpha".to_string()),
                Value::str("beta".to_string()),
            ]),
        ),
    ]);
    let result = addon
        .call_function("echo", std::slice::from_ref(&input))
        .expect("echo(BuchiPack) must succeed");
    assert_eq!(format!("{result:?}"), format!("{input:?}"));
}

#[test]
fn call_function_rejects_unknown_name() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!(
                "note: skipping call_function_rejects_unknown_name -- sample cdylib not built"
            );
            return;
        }
    };
    let err = addon
        .call_function("does_not_exist", &[])
        .expect_err("unknown function must be rejected");
    match err {
        AddonCallError::FunctionNotFound { function, .. } => {
            assert_eq!(function, "does_not_exist");
        }
        other => panic!("expected FunctionNotFound, got {other:?}"),
    }
}

#[test]
fn call_function_rejects_arity_mismatch() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!(
                "note: skipping call_function_rejects_arity_mismatch -- sample cdylib not built"
            );
            return;
        }
    };
    let err = addon
        .call_function("echo", &[Value::Int(1), Value::Int(2)])
        .expect_err("too many args must be rejected");
    match err {
        AddonCallError::ArityMismatch {
            expected, actual, ..
        } => {
            assert_eq!(expected, 1);
            assert_eq!(actual, 2);
        }
        other => panic!("expected ArityMismatch, got {other:?}"),
    }
}

#[test]
fn call_function_rejects_unsupported_input() {
    let addon = match load_sample_addon() {
        Some(a) => a,
        None => {
            eprintln!(
                "note: skipping call_function_rejects_unsupported_input -- sample cdylib not built"
            );
            return;
        }
    };
    // Gorilla is outside the RC1 Phase 3 whitelist; the host rejects
    // it *before* entering the addon so the addon never sees it.
    let err = addon
        .call_function("echo", &[Value::Gorilla])
        .expect_err("Gorilla must be rejected");
    match err {
        AddonCallError::UnsupportedInput { kind, .. } => assert_eq!(kind, "Gorilla"),
        other => panic!("expected UnsupportedInput, got {other:?}"),
    }
}

#[test]
fn library_not_found_for_missing_path() {
    // RC1B-102 fix verification (integration side).
    let err = load_addon("/this/path/should/not/exist/addon.so")
        .expect_err("missing path must produce LibraryNotFound");
    match err {
        AddonLoadError::LibraryNotFound { .. } => {}
        other => panic!("expected LibraryNotFound, got {other:?}"),
    }
}

#[test]
fn entry_symbol_constant_matches_addon_crate() {
    // Single source of truth: the host re-exports the constant from
    // the addon-authoring crate so they cannot drift.
    assert_eq!(TAIDA_ADDON_ENTRY_SYMBOL, "taida_addon_get_v1");
    assert_eq!(TAIDA_ADDON_ABI_VERSION, 1);
}
