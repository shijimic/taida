//! RC1 Phase 4 -- end-to-end addon-backed package integration test.
//!
//! This test exercises the full pipeline from `.dev/RC1_DESIGN.md`
//! Phase 4 Lock §Resolution order:
//!
//! 1. Build a temporary Taida project on disk that imports an
//!    addon-backed package from its `.taida/deps/`.
//! 2. The addon-backed package directory contains
//!    `native/addon.toml` and **does not contain a `.td` source
//!    file** -- the manifest is the only contract.
//! 3. Run `taida` on the project's main `.td` file with the
//!    interpreter backend.
//! 4. The interpreter:
//!    - Locates the package directory via the existing resolver.
//!    - Detects `native/addon.toml`.
//!    - Calls `ensure_addon_supported(Native, ...)`.
//!    - Loads the manifest, locates the cdylib via the search order
//!      (`<pkg>/native/lib<stem>.so` -> workspace `target/`).
//!    - dlopens the cdylib, performs the ABI handshake, and loads
//!      the addon into the registry.
//!    - Binds `echo` as a sentinel into the env.
//!    - When the Taida program calls `echo("hello")`, the dispatcher
//!      routes through `LoadedAddon::call_function`, which exercises
//!      the Phase 3 value bridge end to end.
//!
//! The test deliberately does **not** use the JS or Cranelift native
//! backends -- those are validated by their own dedicated tests
//! (see `gen_import` / `Statement::Import` rejection paths). The
//! interpreter is the RC1 reference dispatch path.

#![cfg(feature = "native")]

use std::path::PathBuf;
use std::process::Command;

fn taida_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_taida"))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Locate the workspace's built `taida-addon-sample` cdylib. Returns
/// `None` if cargo has not built it yet (in which case the test
/// prints a `note:` and skips, matching the existing
/// `addon_loader_smoke.rs` behaviour).
fn find_sample_cdylib() -> Option<PathBuf> {
    let target_root = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir().join("target"));
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

/// Build a temp Taida project with an addon-backed package, run the
/// interpreter on it, and return stdout. The project layout is:
///
/// ```text
/// <tmp>/
///   main.td
///   .taida/
///     deps/
///       taida-lang/
///         addon-rs-sample/
///           native/
///             addon.toml
///             lib<stem>.so   (copied from workspace target)
/// ```
fn run_addon_example(stem: &str) -> Option<(String, std::process::ExitStatus)> {
    let cdylib = find_sample_cdylib()?;

    let project = std::env::temp_dir().join(format!("rc1_phase4_addon_e2e_{}", stem));
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).ok()?;

    // .taida/deps/taida-lang/addon-rs-sample/native/
    let pkg_dir = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("addon-rs-sample");
    let native_dir = pkg_dir.join("native");
    std::fs::create_dir_all(&native_dir).ok()?;

    // Copy the cdylib into the package's native/ directory so the
    // resolver hits the first search order entry.
    let cdylib_dest = native_dir.join(cdylib.file_name()?);
    std::fs::copy(&cdylib, &cdylib_dest).ok()?;

    // Write addon.toml. The library stem must be "taida_addon_sample"
    // (matches the workspace cdylib filename without `lib` prefix /
    // platform suffix).
    let addon_toml = r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/addon-rs-sample"
library = "taida_addon_sample"

[functions]
noop = 0
echo = 1
"#;
    std::fs::write(native_dir.join("addon.toml"), addon_toml).ok()?;

    // Optionally write a packages.tdm so the project_root resolver
    // walks up correctly. The interpreter's find_project_root looks
    // for `packages.tdm`, `taida.toml`, `.taida`, or `.git`. We
    // already have `.taida/`, so the project root is `<tmp>/`.

    // main.td imports the addon function and calls it.
    let main_td = r#">>> taida-lang/addon-rs-sample => @(echo)

result <= echo("hello from taida")
stdout(result)
"#;
    std::fs::write(project.join("main.td"), main_td).ok()?;

    let output = Command::new(taida_bin())
        .arg(project.join("main.td"))
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let status = output.status;

    let _ = std::fs::remove_dir_all(&project);

    Some((stdout, status))
}

#[test]
fn addon_backed_package_round_trips_string_through_interpreter() {
    let (stdout, status) = match run_addon_example("string") {
        Some(v) => v,
        None => {
            eprintln!(
                "note: skipping addon e2e test -- libtaida_addon_sample.{{so,dylib,dll}} not built"
            );
            return;
        }
    };
    assert!(
        status.success(),
        "interpreter must succeed on addon-backed package, stdout={}",
        stdout
    );
    assert!(
        stdout.contains("hello from taida"),
        "echo must round-trip the input, got: {}",
        stdout
    );
}

#[test]
fn addon_backed_package_rejects_unknown_symbol_at_import_time() {
    let cdylib = match find_sample_cdylib() {
        Some(p) => p,
        None => {
            eprintln!("note: skipping addon unknown-symbol test -- cdylib not built");
            return;
        }
    };

    let project = std::env::temp_dir().join("rc1_phase4_addon_unknown_sym");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let pkg_dir = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("addon-rs-sample");
    let native_dir = pkg_dir.join("native");
    std::fs::create_dir_all(&native_dir).unwrap();
    let cdylib_dest = native_dir.join(cdylib.file_name().unwrap());
    std::fs::copy(&cdylib, &cdylib_dest).unwrap();
    std::fs::write(
        native_dir.join("addon.toml"),
        r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/addon-rs-sample"
library = "taida_addon_sample"

[functions]
echo = 1
"#,
    )
    .unwrap();

    // Try to import a symbol that is not in addon.toml.
    let main_td = r#">>> taida-lang/addon-rs-sample => @(notDeclared)

stdout(notDeclared())
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg(project.join("main.td"))
        .output()
        .expect("taida binary must run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "import of undeclared symbol must fail. stdout={}, stderr={}",
        stdout,
        stderr
    );
    let combined = format!("{}{}", stderr, stdout);
    assert!(
        combined.contains("not found in addon-backed package"),
        "diagnostic must classify the failure mode, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn malformed_addon_toml_produces_classifiable_error() {
    // No cdylib is required for this test -- the manifest parser
    // fails before we ever try to dlopen the library.
    let project = std::env::temp_dir().join("rc1_phase4_addon_bad_manifest");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let native_dir = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("addon-rs-sample")
        .join("native");
    std::fs::create_dir_all(&native_dir).unwrap();
    // Wrong abi value.
    std::fs::write(
        native_dir.join("addon.toml"),
        r#"
abi = 99
entry = "taida_addon_get_v1"
package = "taida-lang/addon-rs-sample"
library = "taida_addon_sample"

[functions]
echo = 1
"#,
    )
    .unwrap();
    let main_td = r#">>> taida-lang/addon-rs-sample => @(echo)
stdout(echo("x"))
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg(project.join("main.td"))
        .output()
        .expect("taida binary must run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "malformed addon.toml must fail. stdout={}, stderr={}",
        stdout,
        stderr
    );
    let combined = format!("{}{}", stderr, stdout);
    assert!(
        combined.contains("addon manifest error") || combined.contains("unsupported abi"),
        "diagnostic must classify the manifest failure, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn js_backend_rejects_addon_backed_package_at_compile_time() {
    let project = std::env::temp_dir().join("rc1_phase4_addon_js_reject");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    // The JS build path uses `find_packages_tdm_from` to discover the
    // project root (which is what populates `JsCodegen::project_root`).
    // We need a real `packages.tdm` for that walk to succeed; without
    // it, JS codegen falls back to the no-context path and the addon
    // detection helper has no project root to resolve against.
    std::fs::write(project.join("packages.tdm"), ">>> ./main.td => @(main)\n").unwrap();
    let native_dir = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("addon-rs-sample")
        .join("native");
    std::fs::create_dir_all(&native_dir).unwrap();
    std::fs::write(
        native_dir.join("addon.toml"),
        r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/addon-rs-sample"
library = "taida_addon_sample"

[functions]
echo = 1
"#,
    )
    .unwrap();
    let main_td = r#">>> taida-lang/addon-rs-sample => @(echo)
stdout(echo("x"))
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.mjs"))
        .output()
        .expect("taida binary must run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "JS codegen must reject addon-backed package. stdout={}, stderr={}",
        stdout,
        stderr
    );
    let combined = format!("{}{}", stderr, stdout);
    assert!(
        combined.contains("not supported on backend 'js'")
            || combined.contains("(RC1: native only)"),
        "diagnostic must classify the JS rejection, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn cranelift_native_compile_rejects_addon_backed_package() {
    let project = std::env::temp_dir().join("rc1_phase4_addon_cranelift_reject");
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let native_dir = project
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("addon-rs-sample")
        .join("native");
    std::fs::create_dir_all(&native_dir).unwrap();
    std::fs::write(
        native_dir.join("addon.toml"),
        r#"
abi = 1
entry = "taida_addon_get_v1"
package = "taida-lang/addon-rs-sample"
library = "taida_addon_sample"

[functions]
echo = 1
"#,
    )
    .unwrap();
    let main_td = r#">>> taida-lang/addon-rs-sample => @(echo)
stdout(echo("x"))
"#;
    std::fs::write(project.join("main.td"), main_td).unwrap();

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(project.join("main.td"))
        .arg("-o")
        .arg(project.join("main.bin"))
        .output()
        .expect("taida binary must run");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "Cranelift native compile must reject addon-backed package. stdout={}, stderr={}",
        stdout,
        stderr
    );
    let combined = format!("{}{}", stderr, stdout);
    assert!(
        combined.contains("Cranelift native backend in RC1")
            || combined.contains("interpreter dispatch only"),
        "diagnostic must classify the Cranelift rejection, got: {}",
        combined
    );

    let _ = std::fs::remove_dir_all(&project);
}
