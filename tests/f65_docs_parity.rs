//! Every runnable ` ```taida ` block in `docs/`, with each top-level binding
//! printed at its end, gives the same output and exit status on the compiled
//! backends as on the interpreter.
//!
//! Blocks the interpreter does not run to completion, or whose output changes
//! from run to run, are not compared. Blocks known to differ are listed with
//! the reason in `tests/f65_docs_parity_known.txt`. The test fails when a
//! block that is not listed differs, and when a listed block matches again,
//! so the list only shrinks. `TAIDA_DOCS_PARITY_RECORD=1` rewrites the list
//! from the current results (reasons of blocks still listed are kept).
#![cfg(feature = "native")]
mod common;

use common::{taida_bin, unique_temp_dir, wasmtime_bin};
use regex::Regex;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const KNOWN_PATH: &str = "tests/f65_docs_parity_known.txt";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn collect_md(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_md(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(path);
        }
    }
}

/// `(line of the opening fence, body)` of each ` ```taida ` block (not
/// ` ```taida fragment `).
fn taida_blocks(md: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (i, line) in md.lines().enumerate() {
        match &mut current {
            None => {
                if line.trim_end() == "```taida" {
                    current = Some((i + 1, String::new()));
                }
            }
            Some((start, body)) => {
                if line.trim_end() == "```" {
                    blocks.push((*start, std::mem::take(body)));
                    current = None;
                } else {
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
    }
    blocks
}

/// The block with `stdout(name)` appended for every top-level binding, so
/// every backend has to render every value the example produces. The final
/// `stdout("")` keeps the interpreter from showing the value of a trailing
/// bare expression, which only the interpreter does.
fn instrument(src: &str) -> String {
    let bind_left = Regex::new(r"^([a-z_][A-Za-z0-9_]*)(\s*:\s*[^<=]+)?\s*(<=<|<=)\s").unwrap();
    let bind_right =
        Regex::new(r"(?:>=>|=>)\s*([a-z_][A-Za-z0-9_]*)(\s*:\s*[^/]+?)?\s*(//.*)?$").unwrap();
    let mut names: Vec<String> = Vec::new();
    for line in src.lines() {
        let Some(first) = line.chars().next() else {
            continue;
        };
        if " \t/|=<>".contains(first) {
            continue;
        }
        let name = bind_left
            .captures(line)
            .or_else(|| bind_right.captures(line))
            .map(|c| c[1].to_string());
        if let Some(name) = name
            && name != "_"
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    let mut out = src.to_string();
    out.push('\n');
    for name in names {
        out.push_str(&format!("stdout({name})\n"));
    }
    out.push_str("stdout(\"\")\n");
    out
}

/// Exit status (`None` when killed or timed out) and stdout of `cmd`.
fn run(mut cmd: Command, timeout: Duration) -> (Option<i32>, String) {
    let Ok(mut child) = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return (None, String::new());
    };
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = reader.join().unwrap_or_default();
    (status, String::from_utf8_lossy(&out).into_owned())
}

enum Outcome {
    /// Not compared: the interpreter fails, times out or is nondeterministic.
    Skipped,
    /// Backends whose output or exit status differs from the interpreter's.
    Compared(Vec<String>),
}

fn check_block(dir: &Path, src: &str, wasmtime: Option<&Path>) -> Outcome {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("main.td"), instrument(src)).unwrap();
    let taida = taida_bin();
    let interp = || {
        let mut cmd = Command::new(&taida);
        cmd.arg("main.td").current_dir(dir);
        run(cmd, Duration::from_secs(10))
    };
    let expected = interp();
    if expected.0 != Some(0) || interp() != expected {
        return Outcome::Skipped;
    }
    let mut differs = Vec::new();
    let mut backends = vec!["native"];
    if wasmtime.is_some() {
        backends.push("wasm-wasi");
    }
    for backend in backends {
        let bin = dir.join(format!("main.{backend}"));
        let build = Command::new(&taida)
            .args(["build", backend, "main.td", "-o"])
            .arg(&bin)
            .current_dir(dir)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        // An API the profile does not provide (listed in the docs as such)
        // is not a difference.
        if !build.status.success()
            && String::from_utf8_lossy(&build.stderr).contains("does not support")
        {
            continue;
        }
        let got = if !build.status.success() {
            (None, "<build failed>".to_string())
        } else if backend == "native" {
            let mut cmd = Command::new(&bin);
            cmd.current_dir(dir);
            run(cmd, Duration::from_secs(10))
        } else {
            let mut cmd = Command::new(wasmtime.unwrap());
            cmd.args(["run", "--dir=.", "--"])
                .arg(&bin)
                .current_dir(dir);
            run(cmd, Duration::from_secs(10))
        };
        if got != expected {
            differs.push(backend.to_string());
        }
    }
    Outcome::Compared(differs)
}

/// `label -> (backends, reason)` from the known list.
fn load_known() -> BTreeMap<String, (String, String)> {
    let text = std::fs::read_to_string(repo().join(KNOWN_PATH)).unwrap_or_default();
    let mut known = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (entry, reason) = match line.split_once('#') {
            Some((entry, reason)) => (entry.trim(), reason.trim().to_string()),
            None => (line, String::new()),
        };
        let mut parts = entry.split_whitespace();
        if let (Some(label), Some(backends)) = (parts.next(), parts.next()) {
            known.insert(label.to_string(), (backends.to_string(), reason));
        }
    }
    known
}

#[test]
fn docs_examples_agree_across_backends() {
    let root = repo();
    let mut mds = Vec::new();
    collect_md(&root.join("docs"), &mut mds);
    let mut blocks = Vec::new();
    for md in &mds {
        let text = std::fs::read_to_string(md).unwrap();
        let rel = md
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for (line, body) in taida_blocks(&text) {
            blocks.push((format!("{rel}:{line}"), body));
        }
    }
    assert!(blocks.len() > 100, "found only {} blocks", blocks.len());

    let scratch = unique_temp_dir("f65_docs_parity");
    let wasmtime = wasmtime_bin();
    let next = AtomicUsize::new(0);
    let results: Mutex<BTreeMap<String, Outcome>> = Mutex::new(BTreeMap::new());
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    let Some((label, body)) = blocks.get(i) else {
                        break;
                    };
                    let dir = scratch.join(format!("b{i}"));
                    let outcome = check_block(&dir, body, wasmtime.as_deref());
                    let _ = std::fs::remove_dir_all(&dir);
                    results.lock().unwrap().insert(label.clone(), outcome);
                }
            });
        }
    });
    let _ = std::fs::remove_dir_all(&scratch);
    let results = results.into_inner().unwrap();

    let known = load_known();
    if std::env::var("TAIDA_DOCS_PARITY_RECORD").is_ok() {
        let mut text = String::from(
            "# Runnable docs blocks whose compiled output differs from the interpreter's.\n\
             # <doc>:<line of the opening fence> <backends that differ>  # reason\n",
        );
        for (label, outcome) in &results {
            if let Outcome::Compared(differs) = outcome
                && !differs.is_empty()
            {
                let reason = known
                    .get(label)
                    .map(|(_, reason)| reason.as_str())
                    .unwrap_or("");
                text.push_str(&format!("{label} {}  # {reason}\n", differs.join(",")));
            }
        }
        std::fs::write(root.join(KNOWN_PATH), text).unwrap();
        return;
    }

    let mut problems = Vec::new();
    let mut compared = 0;
    for (label, outcome) in &results {
        let Outcome::Compared(differs) = outcome else {
            continue;
        };
        compared += 1;
        let listed = known.get(label).map(|(backends, _)| backends.as_str());
        let now = differs.join(",");
        match (differs.is_empty(), listed) {
            (true, Some(_)) => {
                problems.push(format!("{label}: matches now; remove it from {KNOWN_PATH}"))
            }
            (false, None) => problems.push(format!("{label}: {now} differ")),
            (false, Some(listed)) if listed != now => {
                problems.push(format!("{label}: {now} differ (listed: {listed})"))
            }
            _ => {}
        }
    }
    assert!(compared > 50, "compared only {compared} blocks");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
