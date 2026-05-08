mod common;

use std::process::Command;

#[test]
fn taida_binary_runtime_lookup_executes_cli() {
    // CI runs this test once directly after `cargo nextest archive` and once
    // from the archive itself. The direct run catches hard-coded target/debug
    // lookups before the archived run validates nextest's remapped path.
    let bin = common::taida_bin();
    let output = Command::new(&bin)
        .arg("--version")
        .output()
        .unwrap_or_else(|err| panic!("failed to run {}: {err}", bin.display()));

    assert!(
        output.status.success(),
        "taida --version failed for {}\nstdout:\n{}\nstderr:\n{}",
        bin.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Taida Lang"),
        "taida --version should identify the CLI, got stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
}
