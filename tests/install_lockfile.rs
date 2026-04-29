mod common;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{}_{}_{}", prefix, std::process::id(), nanos))
}

#[test]
fn test_install_without_deps_writes_lockfile_under_dot_taida() {
    let project_dir = unique_temp_dir("taida_install_lockfile");
    fs::create_dir_all(&project_dir).expect("create project dir");
    fs::write(project_dir.join("packages.tdm"), "name <= \"demo-pkg\"\n")
        .expect("write packages.tdm");
    fs::write(project_dir.join("main.td"), "stdout(\"ok\")\n").expect("write main.td");

    let output = Command::new(common::taida_bin())
        .arg("ingot")
        .arg("install")
        .current_dir(&project_dir)
        .output()
        .expect("run taida ingot install");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "taida ingot install should succeed\nstdout:\n{}\nstderr:\n{}",
        stdout,
        stderr
    );
    assert!(
        project_dir.join(".taida").join("taida.lock").exists(),
        "lockfile should be written to .taida/taida.lock"
    );
    assert!(
        !project_dir.join("taida.lock").exists(),
        "root taida.lock should not be created"
    );

    let _ = fs::remove_dir_all(&project_dir);
}
