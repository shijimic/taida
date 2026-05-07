// Codegen lower-side `expr_is_bool` parity gaps.
//
// `expr_is_bool` (`src/codegen/lower/tag_prop.rs`) decides whether
// `MethodCall(...).toString()` lowers to `taida_str_from_bool` (Bool
// surface) or to `taida_polymorphic_to_string` (raw value). The
// helper is syntax-driven and does NOT consult the type checker, so
// it has both directions of error:
//
//   - FALSE POSITIVE: a hard-coded allow-list of method names
//     (`has`, `isEmpty`, `contains`, …) returns `true` for any
//     receiver, even when the receiver is a user-defined pack whose
//     field of the same name returns a non-Bool. Native then renders
//     the Int as "true"/"false" while Interp and JS render the Int.
//
//   - FALSE NEGATIVE: a cross-module Bool function that never landed
//     in `bool_returning_funcs` during the local pre-pass falls
//     through to the polymorphic stringifier. Native renders "1"/"0"
//     while Interp and JS render "true"/"false".
//
// Both fixtures below are held ignored until the receiver-side type
// projection lands. They are tracked in the local FUTURE_BLOCKERS
// index. When the gap closes, drop the `#[ignore]` attributes.

mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn taida_bin() -> PathBuf {
    common::taida_bin()
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixture_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "expr_is_bool_completeness_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("mkdir fixture");
    dir
}

fn run_three_backends(main_path: &std::path::Path, dir: &std::path::Path) -> [(String, String); 3] {
    let interp = {
        let out = Command::new(taida_bin())
            .arg(main_path)
            .output()
            .expect("interp run");
        assert!(
            out.status.success(),
            "interp failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    let js = if node_available() {
        let mjs = dir.join("main.mjs");
        let build = Command::new(taida_bin())
            .args(["build", "js"])
            .arg(main_path)
            .arg("-o")
            .arg(&mjs)
            .output()
            .expect("build js");
        assert!(
            build.status.success(),
            "js build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new("node").arg(&mjs).output().expect("node run");
        assert!(
            run.status.success(),
            "js run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    } else {
        eprintln!("node unavailable; skipping JS leg");
        String::new()
    };

    let native = if cc_available() {
        let bin = dir.join("main.bin");
        let build = Command::new(taida_bin())
            .args(["build", "native"])
            .arg(main_path)
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("build native");
        assert!(
            build.status.success(),
            "native build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new(&bin).output().expect("native run");
        assert!(
            run.status.success(),
            "native run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    } else {
        eprintln!("cc unavailable; skipping native leg");
        String::new()
    };

    [
        ("interp".to_string(), interp),
        ("js".to_string(), js),
        ("native".to_string(), native),
    ]
}

#[test]
#[ignore = "post-stable: cross-module Bool fn detection (false negative); receiver-T projection required"]
fn expr_is_bool_cross_module_bool_get_or_default_three_backend_parity() {
    // FALSE NEGATIVE — a Bool fn imported from another module is not
    // in the local `bool_returning_funcs` registry, so `expr_is_bool`
    // returns false and `getOrDefault(importedFn(x)).toString()` falls
    // through to the polymorphic stringifier on Native (renders "1"
    // instead of "true").
    let dir = fixture_dir("cross_module");
    let lib = dir.join("lib.td");
    let main = dir.join("main.td");

    fs::write(&lib, "giveTrue x = x > 0 => :Bool\n\n<<< @(giveTrue)\n").expect("write lib");
    fs::write(
        &main,
        ">>> ./lib.td => @(giveTrue)\n\nempty: @[Bool] <= @[]\nb <= empty.first().getOrDefault(giveTrue(5))\nstdout(\"bool:\" + b.toString())\n",
    )
    .expect("write main");

    let results = run_three_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "bool:true",
        "interp must render the Bool surface form"
    );
    for (backend, out) in &results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(
            out, &interp,
            "{} backend disagrees with interp (false-negative gap: cross-module Bool fn not in local registry)",
            backend
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "post-stable: pack field shadowing built-in Bool method names (false positive); receiver-T gating required"]
fn expr_is_bool_pack_field_shadows_bool_method_three_backend_parity() {
    // FALSE POSITIVE — a user-defined pack with a field named like a
    // built-in Bool method (`has`, `isEmpty`, `contains`, …) hits the
    // allow-list before any receiver-type check, so Native lowers
    // `box.has(x).toString()` through `taida_str_from_bool` regardless
    // of the field's actual return type. Interp and JS render the Int
    // value, Native renders "true"/"false".
    let dir = fixture_dir("pack_shadow");
    let main = dir.join("main.td");

    fs::write(
        &main,
        "Box = @(label: Str, has: Int => :Int)\nb <= Box(label <= \"demo\")\nresult <= b.has(7)\nstdout(result.toString())\n",
    )
    .expect("write main");

    let results = run_three_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    // The defaultFn for an unbound `Int => :Int` field returns 0; all
    // three backends should agree on the Int representation.
    assert_eq!(
        interp, "0",
        "interp must render the underlying Int (defaultFn returns 0)"
    );
    for (backend, out) in &results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(
            out, &interp,
            "{} backend disagrees with interp (false-positive gap: allow-list matched on method name without checking receiver type)",
            backend
        );
    }

    let _ = fs::remove_dir_all(&dir);
}
