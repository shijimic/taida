// Calling a function-typed BuchiPack field (`p.check(0)` where
// `Predicate = @(check: Int => :Bool)`) must surface the field's
// declared return type — not the bare `Type::Function` — so downstream
// callers see Bool. The arg-aware checker variant delegates this case
// back to the arg-less variant's Named-pack arm; this fixture keeps
// the three backends in lock-step on the runtime payload so a future
// regression in either path is caught here.

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
        "named_pack_fn_field_{}_{}_{}",
        tag,
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&dir).expect("mkdir fixture");
    dir
}

fn run_three_backends(
    main_path: &std::path::Path,
    dir: &std::path::Path,
) -> [(String, Option<String>); 3] {
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
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
        Some(String::from_utf8_lossy(&run.stdout).trim().to_string())
    } else {
        eprintln!("node unavailable; skipping JS leg");
        None
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
        Some(String::from_utf8_lossy(&run.stdout).trim().to_string())
    } else {
        eprintln!("cc unavailable; skipping native leg");
        None
    };

    [
        ("interp".to_string(), interp),
        ("js".to_string(), js),
        ("native".to_string(), native),
    ]
}

fn assert_three_backends_agree(results: &[(String, Option<String>); 3]) {
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .and_then(|(_, o)| o.clone())
        .expect("interp output is required");
    for (backend, out) in results {
        match out {
            None => continue,
            Some(actual) => {
                assert_eq!(actual, &interp, "{} backend disagrees with interp", backend)
            }
        }
    }
}

#[test]
fn named_pack_function_field_returns_bool_three_backends() {
    let dir = fixture_dir("bool_field");
    let main = dir.join("main.td");
    fs::write(
        &main,
        "Predicate = @(check: Int => :Bool)\n\
         p <= Predicate(check <= _ x: Int = x > 0)\n\
         positive <= p.check(7)\n\
         negative <= p.check(-3)\n\
         stdout(positive.toString() + \"|\" + negative.toString())\n",
    )
    .expect("write main");
    let results = run_three_backends(&main, &dir);
    let interp = results
        .iter()
        .find(|(b, _)| b == "interp")
        .and_then(|(_, o)| o.clone())
        .expect("interp output is required");
    assert_eq!(
        interp, "true|false",
        "interp: function-field Bool method must surface its declared return"
    );
    assert_three_backends_agree(&results);
    let _ = fs::remove_dir_all(&dir);
}
