// `Result[T,P].mapError(fn: P -> Q)` must invoke the mapper with the
// throw payload `P` itself, not its display string. The previous
// runtime contract (passing a Str) silently broke `fn(e: Fail)` /
// `e.message` access for type-correct programs.

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
        "map_error_payload_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("mkdir fixture");
    dir
}

fn wasmtime_available() -> bool {
    Command::new("wasmtime")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_four_backends(main_path: &std::path::Path, dir: &std::path::Path) -> [(String, String); 4] {
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

    let wasm_full = if cc_available() && wasmtime_available() {
        let wasm = dir.join("main.wasm");
        let build = Command::new(taida_bin())
            .args(["build", "wasm-full"])
            .arg(main_path)
            .arg("-o")
            .arg(&wasm)
            .output()
            .expect("build wasm-full");
        assert!(
            build.status.success(),
            "wasm-full build failed: {}",
            String::from_utf8_lossy(&build.stderr)
        );
        let run = Command::new("wasmtime")
            .arg(&wasm)
            .output()
            .expect("wasmtime run");
        assert!(
            run.status.success(),
            "wasm-full run failed: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        String::from_utf8_lossy(&run.stdout).trim().to_string()
    } else {
        eprintln!("wasmtime unavailable; skipping wasm-full leg");
        String::new()
    };

    [
        ("interp".to_string(), interp),
        ("js".to_string(), js),
        ("native".to_string(), native),
        ("wasm-full".to_string(), wasm_full),
    ]
}

fn assert_four_backends_agree(results: &[(String, String); 4]) {
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    for (backend, out) in results {
        if out.is_empty() {
            continue;
        }
        assert_eq!(out, &interp, "{} backend disagrees with interp", backend);
    }
}

#[test]
fn map_error_invokes_mapper_with_payload_not_display_string() {
    // `render(e: Fail)` reads `e.message`. The payload must arrive as
    // the Fail BuchiPack so this access does not crash.
    let dir = fixture_dir("payload");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Error => Fail = @(message: Str)\n\
         render e: Fail = e.message => :Str\n\
         r <= Result[0](throw <= Fail(message <= \"boom\"))\n\
         mapped <= r.mapError(render)\n\
         stdout(mapped.toString())\n",
    )
    .expect("write main");
    let results = run_four_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert!(
        interp.contains("boom"),
        "interp output should embed the original payload's message, got {:?}",
        interp
    );
    assert_four_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_error_q_shape_error_derived_pack_round_trips_four_backends() {
    // Q is `Error => Fail2 = @(extraField: Str, message: Str)`; the
    // mapped throw must be stored directly so toString() can extract
    // the message field and yield the same render across backends.
    let dir = fixture_dir("q_pack");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Error => Fail = @(message: Str)\n\
         Error => Fail2 = @(extraField: Str, message: Str)\n\
         toFail2 e: Fail = Fail2(extraField <= \"x\", message <= \"wrapped: \" + e.message) => :Fail2\n\
         r <= Result[0](throw <= Fail(message <= \"boom\"))\n\
         mapped <= r.mapError(toFail2)\n\
         stdout(mapped.toString())\n",
    )
    .expect("write main");
    let results = run_four_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert!(
        interp.contains("wrapped: boom"),
        "interp must surface the Fail2 message field, got {:?}",
        interp
    );
    assert_four_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_error_q_shape_primitive_string_round_trips_four_backends() {
    // Q is plain Str. The runtime must wrap it into a generic
    // ResultError carrier so toString() produces the same render
    // across backends instead of one backend dumping the raw string
    // and another pretty-printing the wrapper.
    let dir = fixture_dir("q_str");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Error => Fail = @(message: Str)\n\
         render e: Fail = \"prefix: \" + e.message => :Str\n\
         r <= Result[0](throw <= Fail(message <= \"boom\"))\n\
         mapped <= r.mapError(render)\n\
         stdout(mapped.toString())\n",
    )
    .expect("write main");
    let results = run_four_backends(&main, &dir);
    assert_four_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_error_q_shape_message_less_error_pack_round_trips_four_backends() {
    // Q is `Error => CodeOnly = @(code: Int)` — Error-derived but with
    // no `message` field. Direct-store still fires (the pack carries
    // `__type`), and `Result.toString()` falls back to the type name on
    // every backend instead of dumping pack bytes (which previously
    // surfaced as `[object Object]` on JS and as garbage like
    // `KAPDIAT` on Native).
    let dir = fixture_dir("q_message_less");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Error => CodeOnly = @(code: Int)\n\
         makeIt e: Str = CodeOnly(code <= 7) => :CodeOnly\n\
         r <= Result[0](throw <= \"boom\")\n\
         mapped <= r.mapError(makeIt)\n\
         stdout(mapped.toString())\n",
    )
    .expect("write main");
    let results = run_four_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert!(
        interp.contains("CodeOnly"),
        "interp must surface the __type fallback when no message field is present, got {:?}",
        interp
    );
    assert_four_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_error_q_shape_bool_round_trips_four_backends() {
    // Q is a Bool. Untagged scalar paths previously surfaced as the
    // raw integer `1` on Native / WASM-full; the callback return tag
    // is now consumed before formatting so all four backends emit
    // `Result(throw <= true)`.
    let dir = fixture_dir("q_bool");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Error => Fail = @(message: Str)\n\
         toBool e: Fail = true => :Bool\n\
         r <= Result[0](throw <= Fail(message <= \"boom\"))\n\
         mapped <= r.mapError(toBool)\n\
         stdout(mapped.toString())\n",
    )
    .expect("write main");
    let results = run_four_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "Result(throw <= true)",
        "interp must surface the Bool literal verbatim, got {:?}",
        interp
    );
    assert_four_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_error_q_shape_float_round_trips_four_backends() {
    // Q is a Float. Without tag-aware formatting the Native / WASM-full
    // backends would emit the raw IEEE-754 bit pattern (e.g.
    // `4615063718147915776` for 3.5).
    let dir = fixture_dir("q_float");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Error => Fail = @(message: Str)\n\
         toFloat e: Fail = 3.5 => :Float\n\
         r <= Result[0](throw <= Fail(message <= \"boom\"))\n\
         mapped <= r.mapError(toFloat)\n\
         stdout(mapped.toString())\n",
    )
    .expect("write main");
    let results = run_four_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "Result(throw <= 3.5)",
        "interp must surface the Float literal verbatim, got {:?}",
        interp
    );
    assert_four_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_error_empty_message_error_pack_round_trips_four_backends() {
    // `Fail(message <= "")` cannot be distinguished from "no message
    // supplied" on JS — the Error factory always materialises a
    // `message: ''` default. Treat empty strings as "missing" on every
    // backend and fall back to the declared `__type` name so the four
    // backends produce the same output.
    let dir = fixture_dir("empty_message");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Error => Fail = @(message: Str)\n\
         keepIt e: Fail = e => :Fail\n\
         r <= Result[0](throw <= Fail(message <= \"\"))\n\
         mapped <= r.mapError(keepIt)\n\
         stdout(mapped.toString())\n",
    )
    .expect("write main");
    let results = run_four_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .map(|(_, o)| o.clone())
        .unwrap_or_default();
    assert_eq!(
        interp, "Result(throw <= Fail)",
        "interp must fall back to __type when message is empty, got {:?}",
        interp
    );
    assert_four_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn map_error_q_shape_anonymous_pack_round_trips_four_backends() {
    // Q is an anonymous BuchiPack (no `__type`). Direct-store does not
    // apply, so the runtime must wrap it into a generic ResultError
    // carrier and serialise the pack through the polymorphic
    // to-string helper instead of casting the pointer.
    let dir = fixture_dir("q_anon");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Error => Fail = @(message: Str)\n\
         makeAnon e: Fail = @(extra <= \"x\", message <= e.message) => :@(extra: Str, message: Str)\n\
         r <= Result[0](throw <= Fail(message <= \"boom\"))\n\
         mapped <= r.mapError(makeAnon)\n\
         stdout(mapped.toString())\n",
    )
    .expect("write main");
    let results = run_four_backends(&main, &dir);
    assert_four_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}
