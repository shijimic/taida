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
fn test_deps_does_not_update_lock_or_install_on_resolve_error() {
    let project_dir = unique_temp_dir("taida_deps_txn");
    fs::create_dir_all(project_dir.join(".taida").join("deps").join("existing"))
        .expect("create deps sentinel dir");
    fs::write(
        project_dir
            .join(".taida")
            .join("deps")
            .join("existing")
            .join("keep.txt"),
        "keep",
    )
    .expect("write deps sentinel file");

    fs::write(
        project_dir.join("packages.tdm"),
        r#"
name <= "txn-test"
deps <= @(
  missing <= @(path <= "./no_such_dep")
)
"#,
    )
    .expect("write packages.tdm");

    let lock_path = project_dir.join(".taida").join("taida.lock");
    fs::create_dir_all(lock_path.parent().unwrap()).expect("create lockfile parent");
    fs::write(&lock_path, "LOCK_SENTINEL\n").expect("write lock sentinel");

    let output = Command::new(common::taida_bin())
        .arg("ingot")
        .arg("deps")
        .current_dir(&project_dir)
        .output()
        .expect("run taida ingot deps");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "taida ingot deps should fail on unresolved dependency\nstderr:\n{}",
        stderr
    );
    assert!(
        stderr.contains("Skipping install and lockfile update"),
        "deps strict mode message missing\nstderr:\n{}",
        stderr
    );

    let lock_after = fs::read_to_string(&lock_path).expect("read lockfile after deps");
    assert_eq!(
        lock_after, "LOCK_SENTINEL\n",
        "lockfile should remain unchanged"
    );
    assert!(
        project_dir
            .join(".taida")
            .join("deps")
            .join("existing")
            .join("keep.txt")
            .exists(),
        "existing deps contents should remain untouched"
    );

    let _ = fs::remove_dir_all(&project_dir);
}
