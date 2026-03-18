/// Parity tests: verify that all three backends (Interpreter, JS transpiler, Native compiler)
/// produce identical output for the same .td files.
///
/// This is the authoritative test for backend parity. The interpreter is the reference
/// implementation; all other backends must match its output exactly.
///
/// Test categories:
///   1. Interpreter vs JS -- all non-module, non-stdin examples
///   2. Interpreter vs Native -- all compile_*.td examples
///   3. Three-way parity -- compile_*.td files tested across all three backends
mod common;

use common::{normalize, run_interpreter_normalized, taida_bin};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// Run a .td file with the interpreter and return normalized stdout.
///
/// Delegates to `common::run_interpreter_normalized` which applies per-line
/// `normalize()` to tolerate minor trailing-whitespace differences between backends.
fn run_interpreter(td_path: &Path) -> Option<String> {
    run_interpreter_normalized(td_path)
}

/// Transpile a .td file to JS and execute with node, returning stdout.
fn run_js(td_path: &Path) -> Option<String> {
    run_js_with_env(td_path, &[])
}

fn run_js_with_env(td_path: &Path, envs: &[(&str, &str)]) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let js_path = unique_temp_path("taida_parity_js", &stem, "mjs");

    // Transpile
    let transpile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .ok()?;

    if !transpile_output.status.success() {
        let _ = fs::remove_file(&js_path);
        return None;
    }

    // Execute with node
    let mut node = Command::new("node");
    for (k, v) in envs {
        node.env(k, v);
    }
    let run_output = node.arg(&js_path).output().ok()?;

    let _ = fs::remove_file(&js_path);

    if !run_output.status.success() {
        return None;
    }

    Some(normalize(&String::from_utf8_lossy(&run_output.stdout)))
}

/// Build a multi-module .td entrypoint to JS and execute the emitted main module.
fn run_js_project(td_path: &Path, label: &str) -> Option<String> {
    run_js_project_with_env(td_path, label, &[])
}

fn run_js_project_with_env(td_path: &Path, label: &str, envs: &[(&str, &str)]) -> Option<String> {
    let stem = td_path.file_stem()?.to_string_lossy().to_string();
    let out_dir = unique_temp_path("taida_parity_js_project", label, "dir");
    fs::create_dir_all(&out_dir).ok()?;
    let main_out = out_dir.join(format!("{}.mjs", stem));

    let build_output = match Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(td_path)
        .arg("-o")
        .arg(&main_out)
        .output()
    {
        Ok(output) => output,
        Err(_) => {
            let _ = fs::remove_dir_all(&out_dir);
            return None;
        }
    };

    if !build_output.status.success() {
        let _ = fs::remove_dir_all(&out_dir);
        return None;
    }

    let mut node = Command::new("node");
    for (k, v) in envs {
        node.env(k, v);
    }
    let run_output = node.arg(&main_out).output();

    let _ = fs::remove_dir_all(&out_dir);

    let run_output = run_output.ok()?;
    if !run_output.status.success() {
        return None;
    }

    Some(normalize(&String::from_utf8_lossy(&run_output.stdout)))
}

/// Compile a .td file to a native binary, execute it, and return stdout.
fn run_native(td_path: &Path) -> Option<String> {
    run_native_with_error(td_path).ok()
}

fn run_native_with_error(td_path: &Path) -> Result<String, String> {
    let stem = td_path
        .file_stem()
        .ok_or_else(|| format!("missing file stem for {}", td_path.display()))?
        .to_string_lossy()
        .to_string();
    let binary_path = unique_temp_path("taida_parity_native", &stem, "bin");

    // Compile (no global lock needed -- FL-7 ensures unique .o paths)
    let compile_output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(td_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .map_err(|e| format!("failed to invoke native build: {}", e))?;

    if !compile_output.status.success() {
        return Err(String::from_utf8_lossy(&compile_output.stderr)
            .trim()
            .to_string());
    }

    // Execute
    let run_output = Command::new(&binary_path)
        .output()
        .map_err(|e| format!("failed to execute native binary: {}", e))?;

    let _ = fs::remove_file(&binary_path);

    if !run_output.status.success() {
        return Err(String::from_utf8_lossy(&run_output.stderr)
            .trim()
            .to_string());
    }

    Ok(normalize(&String::from_utf8_lossy(&run_output.stdout)))
}

// normalize() is provided by common::normalize (RCB-26).
// LIMITATION (AT-1): This hides trailing-space differences between backends.
// For structured output (jsonPretty, indented strings), meaningful whitespace
// differences may be masked. Consider using exact comparison for specific tests
// where whitespace semantics matter.

fn unique_temp_path(prefix: &str, label: &str, ext: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{}_{}_{}_{}.{}",
        prefix,
        label,
        std::process::id(),
        nanos,
        ext
    ))
}

/// Check if node is available on this system.
fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if cc is available (needed for native compilation).
fn cc_available() -> bool {
    Command::new("cc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn openssl_available() -> bool {
    Command::new("openssl")
        .arg("version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn curl_available() -> bool {
    Command::new("curl")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_interpreter_src(source: &str, label: &str) -> Option<String> {
    let tmp = unique_temp_path("taida_net_interp", label, "td");
    fs::write(&tmp, source).ok()?;
    let out = run_interpreter(&tmp);
    let _ = fs::remove_file(&tmp);
    out
}

fn run_js_src(source: &str, label: &str) -> Option<String> {
    run_js_src_with_env(source, label, &[])
}

fn run_js_src_with_env(source: &str, label: &str, envs: &[(&str, &str)]) -> Option<String> {
    let tmp = unique_temp_path("taida_net_js", label, "td");
    fs::write(&tmp, source).ok()?;
    let out = run_js_with_env(&tmp, envs);
    let _ = fs::remove_file(&tmp);
    out
}

fn run_native_src(source: &str, label: &str) -> Option<String> {
    let tmp = unique_temp_path("taida_net_native", label, "td");
    fs::write(&tmp, source).ok()?;
    let out = run_native(&tmp);
    let _ = fs::remove_file(&tmp);
    out
}

fn assert_backend_parity_for_source(source: &str, label: &str) {
    let interp = run_interpreter_src(source, label)
        .unwrap_or_else(|| panic!("interpreter failed for {}", label));
    let native = run_native_src(source, label)
        .unwrap_or_else(|| panic!("native backend failed for {}", label));
    assert_eq!(
        interp, native,
        "interpreter/native mismatch for source case {}",
        label
    );

    if node_available() {
        let js =
            run_js_src(source, label).unwrap_or_else(|| panic!("js backend failed for {}", label));
        assert_eq!(
            interp, js,
            "interpreter/js mismatch for source case {}",
            label
        );
    }
}

fn assert_backends_reject_source(source: &str, label: &str) {
    assert!(
        run_interpreter_src(source, label).is_none(),
        "interpreter unexpectedly accepted {}",
        label
    );
    assert!(
        run_native_src(source, label).is_none(),
        "native backend unexpectedly accepted {}",
        label
    );
    if node_available() {
        assert!(
            run_js_src(source, label).is_none(),
            "js backend unexpectedly accepted {}",
            label
        );
    }
}

fn run_interpreter_error(td_path: &Path) -> Option<String> {
    let output = Command::new(taida_bin()).arg(td_path).output().ok()?;
    if output.status.success() {
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&output.stderr)))
}

fn run_native_build_error(td_path: &Path, label: &str) -> Option<String> {
    let bin_path = unique_temp_path("taida_parity_native_err", label, "bin");
    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .ok()?;
    let _ = fs::remove_file(&bin_path);
    if output.status.success() {
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&output.stderr)))
}

fn run_js_build_error(td_path: &Path, label: &str) -> Option<String> {
    let out_dir = unique_temp_path("taida_parity_js_err", label, "dir");
    fs::create_dir_all(&out_dir).ok()?;
    let js_path = out_dir.join("main.mjs");
    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .ok()?;
    let _ = fs::remove_dir_all(&out_dir);
    if output.status.success() {
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&output.stderr)))
}

fn spawn_http_echo_server() -> (u16, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind http loopback");
    listener.set_nonblocking(false).expect("set blocking");
    let port = listener.local_addr().expect("local addr").port();
    let (tx, rx) = mpsc::channel();

    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept http");

        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        if let Ok(n) = socket.read(&mut buf) {
            req.extend_from_slice(&buf[..n]);
        }
        let req_text = String::from_utf8_lossy(&req).to_string();
        let _ = tx.send(req_text);

        let body = "ok";
        let resp = format!(
            "HTTP/1.1 201 Created\r\nContent-Length: {}\r\nx_reply: yes\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(resp.as_bytes());
    });

    (port, rx, handle)
}

fn spawn_tcp_echo_server() -> (u16, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind tcp loopback");
    listener.set_nonblocking(false).expect("set blocking");
    let port = listener.local_addr().expect("local addr").port();
    let (tx, rx) = mpsc::channel();

    let handle = thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("accept tcp");
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("set write timeout");

        let mut buf = [0u8; 4096];
        let n = socket.read(&mut buf).unwrap_or(0);
        let req_text = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = tx.send(req_text);
        let _ = socket.write_all(b"pong");
    });

    (port, rx, handle)
}

fn find_free_loopback_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind free loopback port");
    listener.local_addr().expect("local addr").port()
}

fn spawn_tcp_client_for_accept(port: u16) -> (mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let mut stream = None;
        for _ in 0..500 {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }

        let Some(mut socket) = stream else {
            let _ = tx.send("CONNECT_FAIL".to_string());
            return;
        };
        let _ = socket.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = socket.set_write_timeout(Some(Duration::from_secs(5)));

        if socket.write_all(b"ping").is_err() {
            let _ = tx.send("WRITE_FAIL".to_string());
            return;
        }

        let mut buf = [0u8; 4];
        if socket.read_exact(&mut buf).is_err() {
            let _ = tx.send("READ_FAIL".to_string());
            return;
        }

        let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
    });

    (rx, handle)
}

fn spawn_tcp_idle_server(wait: Duration) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind tcp idle loopback");
    listener.set_nonblocking(false).expect("set blocking");
    let port = listener.local_addr().expect("local addr").port();

    let handle = thread::spawn(move || {
        let (_socket, _) = listener.accept().expect("accept idle tcp");
        thread::sleep(wait);
    });

    (port, handle)
}

fn spawn_udp_echo_server() -> (u16, mpsc::Receiver<String>, thread::JoinHandle<()>) {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind udp loopback");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set udp read timeout");
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .expect("set udp write timeout");
    let port = socket.local_addr().expect("local udp addr").port();
    let (tx, rx) = mpsc::channel();

    let handle = thread::spawn(move || {
        let mut buf = [0u8; 65535];
        let (n, peer) = match socket.recv_from(&mut buf) {
            Ok(result) => result,
            Err(err) => {
                let _ = tx.send(format!("UDP_RECV_FAIL: {}", err));
                return;
            }
        };
        let req_text = String::from_utf8_lossy(&buf[..n]).to_string();
        let _ = tx.send(req_text);
        let _ = socket.send_to(b"pong", peer);
    });

    (port, rx, handle)
}

struct HttpsServer {
    port: u16,
    child: Child,
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl Drop for HttpsServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.cert_path);
        let _ = fs::remove_file(&self.key_path);
    }
}

fn spawn_https_server(label: &str) -> Option<HttpsServer> {
    let cert_path = unique_temp_path("taida_https_cert", label, "pem");
    let key_path = unique_temp_path("taida_https_key", label, "pem");

    let cert_gen = Command::new("openssl")
        .arg("req")
        .arg("-x509")
        .arg("-newkey")
        .arg("rsa:2048")
        .arg("-nodes")
        .arg("-subj")
        .arg("/CN=127.0.0.1")
        .arg("-keyout")
        .arg(&key_path)
        .arg("-out")
        .arg(&cert_path)
        .arg("-days")
        .arg("1")
        .output()
        .ok()?;
    if !cert_gen.status.success() {
        let _ = fs::remove_file(&cert_path);
        let _ = fs::remove_file(&key_path);
        return None;
    }

    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    drop(listener);

    let child = Command::new("openssl")
        .arg("s_server")
        .arg("-accept")
        .arg(port.to_string())
        .arg("-cert")
        .arg(&cert_path)
        .arg("-key")
        .arg(&key_path)
        .arg("-www")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    for _ in 0..40 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(HttpsServer {
                port,
                child,
                cert_path,
                key_path,
            });
        }
        thread::sleep(Duration::from_millis(50));
    }

    let mut failed = HttpsServer {
        port,
        child,
        cert_path,
        key_path,
    };
    let _ = failed.child.kill();
    None
}

/// Examples that should be skipped for JS parity testing.
/// These use features that the JS transpiler does not support or require
/// module resolution / stdin / specific file paths.
fn js_skip_list() -> Vec<&'static str> {
    vec![
        "09_modules",                    // requires module import resolution
        "module_math",                   // helper module, not standalone
        "module_utils",                  // helper module, not standalone
        "helper_val",                    // helper module, not standalone
        "transpile_j2",                  // transpile-specific test
        "transpile_npm",                 // transpile-specific test
        "wasm_wasi_stderr",              // wasm-wasi specific (requires wasmtime)
        "wasm_wasi_env",                 // wasm-wasi specific (requires wasmtime)
        "wasm_wasi_file_io",             // wasm-wasi specific (requires wasmtime)
        "wasm_wasi_exists",              // wasm-wasi specific (requires wasmtime)
        "wasm_wasi_write_failure",       // wasm-wasi specific (requires wasmtime)
        "wasm_wasi_write_failure_shape", // wasm-wasi specific (shape validation)
    ]
}

/// Examples that are expected to fail in the interpreter.
/// Only these files are allowed to be skipped when the interpreter fails.
/// All other interpreter failures are treated as test failures.
fn interpreter_skip_list() -> Vec<&'static str> {
    vec![
        "module_math",   // helper module, not standalone
        "module_utils",  // helper module, not standalone
        "helper_val",    // helper module, not standalone
        "transpile_npm", // npm: imports only work in JS transpiler
    ]
}

/// Examples that are expected to be rejected by the native backend.
fn native_expected_reject_list() -> Vec<&'static str> {
    vec![
        "compile_stream", // Stream[T] is outside the native backend capability set
    ]
}

// =========================================================================
// Test 1: Interpreter vs JS parity (all eligible examples)
// =========================================================================
#[test]
fn test_interpreter_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available, skipping JS parity tests");
        return;
    }

    let skip = js_skip_list();
    let dir = examples_dir();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("examples/ directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.ends_with(".td")
                && !name.starts_with("compile_")  // compile_* are native-focused
                && !skip.iter().any(|s| name == format!("{}.td", s))
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut passed = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();

    let interp_skip = interpreter_skip_list();

    for entry in &entries {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();

        let interp = match run_interpreter(&path) {
            Some(o) => o,
            None => {
                if interp_skip.contains(&name.as_str()) {
                    skipped += 1;
                    continue;
                }
                // AT-3: Record interpreter failure AND capture JS output for visibility.
                let js_note = match run_js(&path) {
                    Some(js_out) => format!(
                        "  js output: {:?}",
                        js_out.lines().take(3).collect::<Vec<_>>()
                    ),
                    None => "  js: also failed".to_string(),
                };
                failures.push(format!(
                    "{}: interpreter failed (reference impl error)\n{}",
                    name, js_note
                ));
                continue;
            }
        };

        let js = match run_js(&path) {
            Some(o) => o,
            None => {
                failures.push(format!("{}: JS transpile/execution failed", name));
                continue;
            }
        };

        if interp == js {
            passed += 1;
        } else {
            failures.push(format!(
                "{}: Interpreter vs JS output mismatch\n  interp: {:?}\n  js:     {:?}",
                name,
                interp.lines().take(3).collect::<Vec<_>>(),
                js.lines().take(3).collect::<Vec<_>>(),
            ));
        }
    }

    eprintln!(
        "Interpreter-JS parity: {}/{} passed, {} skipped",
        passed,
        passed + failures.len(),
        skipped,
    );

    if !failures.is_empty() {
        panic!(
            "{} JS parity test(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n"),
        );
    }
}

// =========================================================================
// Test 2: Three-way parity for compile_*.td (Interpreter vs JS vs Native)
// =========================================================================
#[test]
fn test_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();

    if !has_cc {
        eprintln!("SKIP: cc not available, skipping three-way parity tests");
        return;
    }

    let native_expected_rejects = native_expected_reject_list();
    let dir = examples_dir();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("examples/ directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("compile_") && name.ends_with(".td")
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    assert!(!entries.is_empty(), "No compile_*.td files found");

    let mut passed = 0;
    let mut native_rejected = Vec::new();
    let mut failures = Vec::new();

    for entry in &entries {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();

        let interp = match run_interpreter(&path) {
            Some(o) => o,
            None => {
                failures.push(format!("{}: interpreter failed", name));
                continue;
            }
        };

        // Native check
        let native = match run_native_with_error(&path) {
            Ok(o) => o,
            Err(err) => {
                if native_expected_rejects.contains(&name.as_str()) {
                    assert!(
                        err.contains("unsupported mold type: Stream"),
                        "{}: expected Stream capability reject, got: {}",
                        name,
                        err
                    );
                    if has_node {
                        let js = run_js(&path)
                            .unwrap_or_else(|| panic!("{}: JS transpile/execution failed", name));
                        if interp != js {
                            failures.push(format!(
                                "{}: Interpreter vs JS mismatch\n  interp: {:?}\n  js:     {:?}",
                                name,
                                interp.lines().take(3).collect::<Vec<_>>(),
                                js.lines().take(3).collect::<Vec<_>>(),
                            ));
                            continue;
                        }
                    }
                    native_rejected.push(name.clone());
                    continue;
                }
                failures.push(format!("{}: native compile/run failed\n  {}", name, err));
                continue;
            }
        };

        if interp != native {
            failures.push(format!(
                "{}: Interpreter vs Native mismatch\n  interp: {:?}\n  native: {:?}",
                name,
                interp.lines().take(3).collect::<Vec<_>>(),
                native.lines().take(3).collect::<Vec<_>>(),
            ));
            continue;
        }

        // JS check (if node available)
        if has_node
            && let Some(js) = run_js(&path)
            && interp != js
        {
            failures.push(format!(
                "{}: Interpreter vs JS mismatch\n  interp: {:?}\n  js:     {:?}",
                name,
                interp.lines().take(3).collect::<Vec<_>>(),
                js.lines().take(3).collect::<Vec<_>>(),
            ));
            continue;
        }
        // If JS transpile fails for compile_* files, that's OK -- they are
        // primarily Native-focused tests

        passed += 1;
    }

    eprintln!(
        "Three-way parity: {}/{} passed, {} expected native rejected",
        passed,
        passed + failures.len() + native_rejected.len(),
        native_rejected.len(),
    );

    let expected_rejected: Vec<String> = native_expected_rejects
        .iter()
        .map(|name| name.to_string())
        .collect();
    assert_eq!(
        native_rejected, expected_rejected,
        "native expected-reject allowlist drifted"
    );

    if !failures.is_empty() {
        panic!(
            "{} three-way parity test(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n"),
        );
    }
}

// =========================================================================
// Test 3: Interpreter vs Native parity for numbered examples (FL-17)
// =========================================================================

/// Numbered examples with known native backend output mismatches.
/// These are tracked as native backend issues and should be fixed eventually.
/// When fixed, remove from this list so the parity test catches regressions.
///
/// Tracked as TF-6 through TF-11 in `.dev/FIX_PROGRESS.md`.
/// TF-6/7/8/9/10/11 + 06_lists all fixed.
/// 06_lists: Reverse mold was misidentified as string-returning in lower.rs.
///
/// Fixed root causes:
///   - TF-6: Template literal now parses expressions via full parser (03, 15)
///   - TF-7: Recursive calls in template literals now work (04)
///   - TF-8: Method calls in template literals now work (06 partial)
///   - TF-9: Closure calls in template literals now work (07)
///   - TF-10: typeof() now implemented in native backend (26)
///   - TF-11: Error toString extracts message field correctly (27)
///   - Reverse: Removed from string-returning molds (polymorphic: Str or List)
fn native_numbered_known_failures() -> Vec<&'static str> {
    vec![]
}

#[test]
fn test_numbered_examples_native_parity() {
    if !cc_available() {
        eprintln!("SKIP: cc not available, skipping numbered examples native parity");
        return;
    }

    let known_failures = native_numbered_known_failures();
    let skip = interpreter_skip_list();
    let dir = examples_dir();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("examples/ directory should exist")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // Numbered examples: start with a digit, end with .td
            name.ends_with(".td")
                && name.starts_with(|c: char| c.is_ascii_digit())
                && !skip.iter().any(|s| name == format!("{}.td", s))
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    assert!(
        !entries.is_empty(),
        "No numbered example .td files found in examples/"
    );

    let mut passed = 0;
    let mut expected_failed = 0;
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();

    for entry in &entries {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let is_known_failure = known_failures.contains(&name.as_str());

        let interp = match run_interpreter(&path) {
            Some(o) => o,
            None => {
                // AT-3: Record interpreter failures instead of silently skipping.
                // Interpreter is the reference implementation; failures must be visible.
                if !is_known_failure {
                    unexpected_failures.push(format!(
                        "{}: interpreter failed (reference implementation error)",
                        name,
                    ));
                } else {
                    expected_failed += 1;
                }
                continue;
            }
        };

        let native = match run_native(&path) {
            Some(o) => o,
            None => {
                if is_known_failure {
                    expected_failed += 1;
                    continue;
                }
                unexpected_failures.push(format!("{}: native compile/run failed", name,));
                continue;
            }
        };

        if interp == native {
            if is_known_failure {
                unexpected_passes.push(name.clone());
            }
            passed += 1;
        } else if is_known_failure {
            expected_failed += 1;
        } else {
            unexpected_failures.push(format!(
                "{}: output mismatch\n  interp:  {:?}\n  native:  {:?}",
                name,
                interp.lines().take(3).collect::<Vec<_>>(),
                native.lines().take(3).collect::<Vec<_>>(),
            ));
        }
    }

    eprintln!(
        "Numbered-examples native parity: {}/{} passed, {} known failures",
        passed,
        passed + unexpected_failures.len() + expected_failed,
        expected_failed,
    );

    if !unexpected_passes.is_empty() {
        panic!(
            "{} examples in known-failures list now PASS -- remove from allowlist: {:?}",
            unexpected_passes.len(),
            unexpected_passes,
        );
    }

    if !unexpected_failures.is_empty() {
        panic!(
            "{} numbered-example native parity test(s) failed:\n\n{}",
            unexpected_failures.len(),
            unexpected_failures.join("\n\n"),
        );
    }
}

#[test]
fn test_http_request_headers_body_loopback_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping http loopback parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let (port, rx, handle) = spawn_http_echo_server();
        let source = format!(
            r#"
resp <= HttpRequest["POST", "http://127.0.0.1:{port}/echo"](headers <= @(x_test <= "abc"), body <= "ping")
resp ]=> out
stdout(out.__value.status.toString())
stdout(out.__value.body)
"#
        );

        let out = match backend {
            "interp" => run_interpreter_src(&source, "http_interp"),
            "js" => run_js_src(&source, "http_js"),
            "native" => run_native_src(&source, "http_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for http loopback", backend));

        assert_eq!(
            out, "201\nok",
            "{} backend output mismatch for http loopback",
            backend
        );

        let req = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{} backend: no request captured", backend));
        assert!(
            req.contains("x_test: abc"),
            "{} backend request missing custom header: {:?}",
            backend,
            req
        );
        assert!(
            req.ends_with("ping"),
            "{} backend request missing body: {:?}",
            backend,
            req
        );
        handle.join().expect("join http server");
    }
}

#[test]
fn test_file_bytes_read_write_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping file bytes parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let path = unique_temp_path("taida_os_bytes", backend, "bin");
        let path_s = path.to_string_lossy().replace('\\', "\\\\");
        let source = format!(
            r#"
payloadLax <= Bytes["pong"]()
payloadLax ]=> payload
writeRes <= writeBytes("{path}", payload)
stdout(writeRes.__value.ok.toString())
readRes <= readBytes("{path}")
stdout(readRes.hasValue.toString())
decoded <= Utf8Decode[readRes.__value]()
decoded ]=> text
stdout(text)
"#,
            path = path_s
        );

        let out = match backend {
            "interp" => run_interpreter_src(&source, "file_bytes_interp"),
            "js" => run_js_src(&source, "file_bytes_js"),
            "native" => run_native_src(&source, "file_bytes_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for file bytes parity", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "{} backend output shape mismatch for file bytes parity: {:?}",
            backend,
            out
        );
        assert!(
            lines[0] == "true" || lines[0] == "1",
            "{} backend expected writeBytes success marker, got {:?}",
            backend,
            lines[0]
        );
        assert!(
            lines[1] == "true" || lines[1] == "1",
            "{} backend expected readBytes success marker, got {:?}",
            backend,
            lines[1]
        );
        assert_eq!(
            lines[2], "pong",
            "{} backend expected decoded bytes payload",
            backend
        );

        let raw =
            fs::read(&path).unwrap_or_else(|_| panic!("{} backend did not create file", backend));
        assert_eq!(
            raw, b"pong",
            "{} backend wrote unexpected file payload",
            backend
        );
        let _ = fs::remove_file(&path);
    }
}

#[test]
fn test_tcp_send_recv_loopback_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping tcp loopback parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let (port, rx, handle) = spawn_tcp_echo_server();
        let source = format!(
            r#"
conn <= tcpConnect("127.0.0.1", {port})
conn ]=> c
sendRes <= socketSend(c.__value.socket, "ping")
sendRes ]=> s
recvRes <= socketRecv(c.__value.socket)
recvRes ]=> r
stdout(s.__value.bytesSent.toString())
stdout(r.__value)
"#
        );

        let out = match backend {
            "interp" => run_interpreter_src(&source, "tcp_interp"),
            "js" => run_js_src(&source, "tcp_js"),
            "native" => run_native_src(&source, "tcp_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for tcp loopback", backend));

        assert_eq!(
            out, "4\npong",
            "{} backend output mismatch for tcp loopback",
            backend
        );

        let req = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{} backend: no tcp payload captured", backend));
        assert_eq!(
            req, "ping",
            "{} backend sent unexpected tcp payload",
            backend
        );
        handle.join().expect("join tcp server");
    }
}

#[test]
fn test_tcp_send_recv_bytes_loopback_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping tcp bytes loopback parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let (port, rx, handle) = spawn_tcp_echo_server();
        let source = format!(
            r#"
conn <= tcpConnect("127.0.0.1", {port})
conn ]=> c
payloadLax <= Bytes["ping"]()
payloadLax ]=> payload
sendRes <= socketSendBytes(c.__value.socket, payload)
sendRes ]=> s
recvRes <= socketRecvBytes(c.__value.socket)
recvRes ]=> r
stdout(s.__value.bytesSent.toString())
stdout(r.hasValue.toString())
decoded <= Utf8Decode[r.__value]()
decoded ]=> msg
stdout(msg)
"#
        );

        let out = match backend {
            "interp" => run_interpreter_src(&source, "tcp_bytes_interp"),
            "js" => run_js_src(&source, "tcp_bytes_js"),
            "native" => run_native_src(&source, "tcp_bytes_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for tcp bytes loopback", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "{} backend output shape mismatch for tcp bytes loopback: {:?}",
            backend,
            out
        );
        assert_eq!(
            lines[0], "4",
            "{} backend expected bytesSent=4 for tcp bytes loopback",
            backend
        );
        assert!(
            lines[1] == "true" || lines[1] == "1",
            "{} backend expected socketRecvBytes success marker, got {:?}",
            backend,
            lines[1]
        );
        assert_eq!(
            lines[2], "pong",
            "{} backend expected decoded tcp bytes payload",
            backend
        );

        let req = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{} backend: no tcp payload captured", backend));
        assert_eq!(
            req, "ping",
            "{} backend sent unexpected tcp bytes payload",
            backend
        );
        handle.join().expect("join tcp bytes server");
    }
}

#[test]
fn test_tcp_accept_sendall_recvexact_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping tcp accept/sendAll/recvExact parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let mut last_error = None;
        for _attempt in 0..3 {
            let port = find_free_loopback_port();
            let (rx, client_handle) = spawn_tcp_client_for_accept(port);
            let source = format!(
                r#"
listenerRes <= tcpListen({port}, 1000)
listenerRes ]=> l
acceptRes <= tcpAccept(l.__value.listener, 5000)
acceptRes ]=> a
recvRes <= socketRecvExact(a.__value.socket, 4, 5000)
recvRes ]=> r
decoded <= Utf8Decode[r.__value]()
decoded ]=> msg
sendPayload <= Bytes["pong"]()
sendPayload ]=> payload
sendRes <= socketSendAll(a.__value.socket, payload, 1000)
sendRes ]=> s
closeClient <= socketClose(a.__value.socket)
closeClient ]=> c
closeListener <= listenerClose(l.__value.listener)
closeListener ]=> lc
stdout(r.hasValue.toString())
stdout(msg)
stdout(s.__value.bytesSent.toString())
stdout(c.__value.ok.toString())
stdout(lc.__value.ok.toString())
"#
            );

            let out = match backend {
                "interp" => run_interpreter_src(&source, "tcp_accept_interp"),
                "js" => run_js_src(&source, "tcp_accept_js"),
                "native" => run_native_src(&source, "tcp_accept_native"),
                _ => None,
            };

            let outcome = (|| -> Result<(), String> {
                let out = out.ok_or_else(|| {
                    format!(
                        "{} backend failed for tcp accept/sendAll/recvExact parity",
                        backend
                    )
                })?;

                let lines: Vec<&str> = out.lines().collect();
                if lines.len() != 5 {
                    return Err(format!(
                        "{} backend output shape mismatch for tcp accept/sendAll/recvExact parity: {:?}",
                        backend, out
                    ));
                }
                if !(lines[0] == "true" || lines[0] == "1") {
                    return Err(format!(
                        "{} backend expected socketRecvExact success marker, got {:?}",
                        backend, lines[0]
                    ));
                }
                if lines[1] != "ping" {
                    return Err(format!(
                        "{} backend expected decoded request payload, got {:?}",
                        backend, lines[1]
                    ));
                }
                if lines[2] != "4" {
                    return Err(format!(
                        "{} backend expected bytesSent=4 for socketSendAll, got {:?}",
                        backend, lines[2]
                    ));
                }
                if !(lines[3] == "true" || lines[3] == "1") {
                    return Err(format!(
                        "{} backend expected socketClose success marker, got {:?}",
                        backend, lines[3]
                    ));
                }
                if !(lines[4] == "true" || lines[4] == "1") {
                    return Err(format!(
                        "{} backend expected listenerClose success marker, got {:?}",
                        backend, lines[4]
                    ));
                }

                let echoed = rx
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| format!("{} backend: no tcp reply captured by client", backend))?;
                if echoed != "pong" {
                    return Err(format!(
                        "{} backend returned unexpected response payload to client: {:?}",
                        backend, echoed
                    ));
                }

                Ok(())
            })();

            let _ = client_handle.join();
            match outcome {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        if let Some(err) = last_error {
            panic!("{}", err);
        }
    }
}

#[test]
fn test_udp_send_recv_loopback_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping udp loopback parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let mut last_error = None;
        for _attempt in 0..3 {
            let (port, rx, handle) = spawn_udp_echo_server();
            let source = format!(
                r#"
sock <= udpBind("127.0.0.1", 0, 200)
sock ]=> s
payloadLax <= Bytes["ping"]()
payloadLax ]=> payload
sendRes <= udpSendTo(s.__value.socket, "127.0.0.1", {port}, payload, 1000)
sendRes ]=> sent
recvRes <= udpRecvFrom(s.__value.socket, 1000)
recvRes ]=> recv
decoded <= Utf8Decode[recv.__value.data]()
decoded ]=> msg
closeRes <= udpClose(s.__value.socket)
closeRes ]=> closed
stdout(sent.__value.bytesSent.toString())
stdout(recv.hasValue.toString())
stdout(msg)
stdout(closed.__value.ok.toString())
"#
            );

            let out = match backend {
                "interp" => run_interpreter_src(&source, "udp_interp"),
                "js" => run_js_src(&source, "udp_js"),
                "native" => run_native_src(&source, "udp_native"),
                _ => None,
            };

            let outcome = (|| -> Result<(), String> {
                let out =
                    out.ok_or_else(|| format!("{} backend failed for udp loopback", backend))?;
                let lines: Vec<&str> = out.lines().collect();
                if lines.len() != 4 {
                    return Err(format!(
                        "{} backend output shape mismatch for udp loopback: {:?}",
                        backend, out
                    ));
                }
                if lines[0] != "4" {
                    return Err(format!(
                        "{} backend expected bytesSent=4 for udp loopback, got {:?}",
                        backend, lines[0]
                    ));
                }
                if !(lines[1] == "true" || lines[1] == "1") {
                    return Err(format!(
                        "{} backend expected truthy udp recv marker, got {:?}",
                        backend, lines[1]
                    ));
                }
                if lines[2] != "pong" {
                    return Err(format!(
                        "{} backend expected udp echoed payload, got {:?}",
                        backend, lines[2]
                    ));
                }
                if !(lines[3] == "true" || lines[3] == "1") {
                    return Err(format!(
                        "{} backend expected udpClose success marker, got {:?}",
                        backend, lines[3]
                    ));
                }

                let req = rx
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|_| format!("{} backend: no udp payload captured", backend))?;
                if req != "ping" {
                    return Err(format!(
                        "{} backend sent unexpected udp payload: {:?}",
                        backend, req
                    ));
                }

                Ok(())
            })();

            let _ = handle.join();
            match outcome {
                Ok(()) => {
                    last_error = None;
                    break;
                }
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        if let Some(err) = last_error {
            panic!("{}", err);
        }
    }
}

#[test]
fn test_socket_listener_close_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping close parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let (port, _rx, handle) = spawn_tcp_echo_server();
        let source = format!(
            r#"
conn <= tcpConnect("127.0.0.1", {port})
conn ]=> c
closedByAlias <= udpClose(c.__value.socket)
closedByAlias ]=> c1
stdout(c1.__value.ok.toString())

closeAgain <= socketClose(c.__value.socket)
closeAgain ]=> c2
stdout(c2.__value.ok.toString())

listenerRes <= tcpListen(0)
listenerRes ]=> l
listenerClosed <= listenerClose(l.__value.listener)
listenerClosed ]=> l1
stdout(l1.__value.ok.toString())

listenerCloseAgain <= listenerClose(l.__value.listener)
listenerCloseAgain ]=> l2
stdout(l2.__value.ok.toString())
"#
        );

        let out = match backend {
            "interp" => run_interpreter_src(&source, "close_interp"),
            "js" => run_js_src(&source, "close_js"),
            "native" => run_native_src(&source, "close_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for close parity", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            4,
            "{} backend output shape mismatch for close parity: {:?}",
            backend,
            out
        );
        assert_eq!(
            lines[0], lines[2],
            "{} backend should report the same success marker for udpClose/listenerClose",
            backend
        );
        assert_eq!(
            lines[1], lines[3],
            "{} backend should report the same failure marker for second close",
            backend
        );
        assert_ne!(
            lines[0], lines[1],
            "{} backend should distinguish success and failure close markers",
            backend
        );

        handle.join().expect("join tcp close server");
    }
}

#[test]
fn test_socket_recv_timeout_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping recv timeout parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let (port, handle) = spawn_tcp_idle_server(Duration::from_millis(300));
        let source = format!(
            r#"
conn <= tcpConnect("127.0.0.1", {port}, 200)
conn ]=> c
recvRes <= socketRecv(c.__value.socket, 50)
recvRes ]=> r
stdout(r.hasValue.toString())
"#
        );

        let out = match backend {
            "interp" => run_interpreter_src(&source, "timeout_interp"),
            "js" => run_js_src(&source, "timeout_js"),
            "native" => run_native_src(&source, "timeout_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for recv timeout parity", backend));

        assert!(
            out == "false" || out == "0",
            "{} backend expected recv timeout false marker, got {:?}",
            backend,
            out
        );

        handle.join().expect("join tcp idle server");
    }
}

#[test]
fn test_socket_error_kind_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping socket error-kind parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let refused_port = find_free_loopback_port();
        let source = format!(
            r#"
listenerRes <= tcpListen(0)
listenerRes ]=> l
acceptRes <= tcpAccept(l.__value.listener, 20)
acceptRes ]=> a
stdout(a.__value.ok.toString())
stdout(a.__value.kind)
closeRes <= listenerClose(l.__value.listener)
closeRes ]=> c
stdout(c.__value.ok.toString())

connRes <= tcpConnect("127.0.0.1", {refused_port}, 200)
connRes ]=> conn
stdout(conn.__value.ok.toString())
stdout(conn.__value.kind)
"#
        );

        let out = match backend {
            "interp" => run_interpreter_src(&source, "socket_error_kind_interp"),
            "js" => run_js_src(&source, "socket_error_kind_js"),
            "native" => run_native_src(&source, "socket_error_kind_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for socket error-kind parity", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            5,
            "{} backend output shape mismatch for socket error-kind parity: {:?}",
            backend,
            out
        );
        assert!(
            lines[0] == "false" || lines[0] == "0",
            "{} backend expected tcpAccept timeout failure marker, got {:?}",
            backend,
            lines[0]
        );
        assert_eq!(
            lines[1], "timeout",
            "{} backend expected tcpAccept timeout kind",
            backend
        );
        assert!(
            lines[2] == "true" || lines[2] == "1",
            "{} backend expected listenerClose success marker, got {:?}",
            backend,
            lines[2]
        );
        assert!(
            lines[3] == "false" || lines[3] == "0",
            "{} backend expected tcpConnect refused failure marker, got {:?}",
            backend,
            lines[3]
        );
        assert_eq!(
            lines[4], "refused",
            "{} backend expected tcpConnect refused kind",
            backend
        );
    }
}

#[test]
fn test_dns_resolve_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping dnsResolve parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let source = r#"
res <= dnsResolve("localhost", 1000)
res ]=> r
stdout((r.__value.addresses.length() > 0).toString())
"#;

        let out = match backend {
            "interp" => run_interpreter_src(source, "dns_resolve_interp"),
            "js" => run_js_src(source, "dns_resolve_js"),
            "native" => run_native_src(source, "dns_resolve_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for dnsResolve parity", backend));

        assert!(
            out == "true" || out == "1",
            "{} backend expected non-empty dnsResolve result, got {:?}",
            backend,
            out
        );
    }
}

#[test]
fn test_pool_lifecycle_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping pool parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    let source = r#"
create <= poolCreate(@(maxSize <= 1, maxIdle <= 1, acquireTimeoutMs <= 25))
create ]=> c
p <= c.pool

h0 <= poolHealth(p)
stdout(h0.open.toString())
stdout(h0.idle.toString())
stdout(h0.inUse.toString())
stdout(h0.waiting.toString())

a1 <= poolAcquire(p, 25)
a1 ]=> r1
stdout((r1.__value.token > 0).toString())
t1 <= r1.__value.token

rel1 <= poolRelease(p, t1, "conn-1")
rel1 ]=> rr1
stdout(rr1.reused.toString())

a2 <= poolAcquire(p, 25)
a2 ]=> r2
stdout((r2.__value.token == t1).toString())
t2 <= r2.__value.token

a3 <= poolAcquire(p, 25)
a3 ]=> r3
stdout(r3.__value.ok.toString())
stdout(r3.__value.kind)

rel2 <= poolRelease(p, t2, "conn-2")
rel2 ]=> _ignored

closeRes <= poolClose(p)
closeRes ]=> cl
stdout(cl.__value.ok.toString())

a4 <= poolAcquire(p, 25)
a4 ]=> r4
stdout(r4.__value.kind)
"#;

    for backend in backends {
        let out = match backend {
            "interp" => run_interpreter_src(source, "pool_lifecycle_interp"),
            "js" => run_js_src(source, "pool_lifecycle_js"),
            "native" => run_native_src(source, "pool_lifecycle_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for pool lifecycle parity", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            11,
            "{} backend output shape mismatch for pool lifecycle parity: {:?}",
            backend,
            out
        );
        assert!(
            lines[0] == "true" || lines[0] == "1",
            "{} backend expected health.open true, got {:?}",
            backend,
            lines[0]
        );
        assert_eq!(
            lines[1], "0",
            "{} backend expected health.idle=0, got {:?}",
            backend, lines[1]
        );
        assert_eq!(
            lines[2], "0",
            "{} backend expected health.inUse=0, got {:?}",
            backend, lines[2]
        );
        assert_eq!(
            lines[3], "0",
            "{} backend expected health.waiting=0, got {:?}",
            backend, lines[3]
        );
        assert!(
            lines[4] == "true" || lines[4] == "1",
            "{} backend expected first acquire success, got {:?}",
            backend,
            lines[4]
        );
        assert!(
            lines[5] == "true" || lines[5] == "1",
            "{} backend expected first release reused=true, got {:?}",
            backend,
            lines[5]
        );
        assert!(
            lines[6] == "true" || lines[6] == "1",
            "{} backend expected resource reuse on second acquire, got {:?}",
            backend,
            lines[6]
        );
        assert!(
            lines[7] == "false" || lines[7] == "0",
            "{} backend expected third acquire failure marker, got {:?}",
            backend,
            lines[7]
        );
        assert_eq!(
            lines[8], "timeout",
            "{} backend expected third acquire kind=timeout, got {:?}",
            backend, lines[8]
        );
        assert!(
            lines[9] == "true" || lines[9] == "1",
            "{} backend expected poolClose success marker, got {:?}",
            backend,
            lines[9]
        );
        assert_eq!(
            lines[10], "closed",
            "{} backend expected acquire-after-close kind=closed, got {:?}",
            backend, lines[10]
        );
    }
}

#[test]
fn test_https_get_loopback_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping https tls parity");
        return;
    }
    if !openssl_available() {
        eprintln!("SKIP: openssl not available, skipping https tls parity");
        return;
    }
    if !curl_available() {
        eprintln!("SKIP: curl not available, skipping https tls parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    for backend in backends {
        let server = spawn_https_server(backend)
            .unwrap_or_else(|| panic!("{} backend: failed to spawn https server", backend));
        let source = format!(
            r#"
resp <= HttpGet["https://127.0.0.1:{}/"]()
resp ]=> out
stdout(out.hasValue.toString())
stdout(out.__value.status.toString())
"#,
            server.port
        );

        let out = match backend {
            "interp" => run_interpreter_src(&source, "https_interp"),
            "js" => run_js_src(&source, "https_js"),
            "native" => run_native_src(&source, "https_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for https tls parity", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "{} backend output shape mismatch for https tls parity: {:?}",
            backend,
            out
        );
        assert!(
            lines[0] == "false" || lines[0] == "0",
            "{} backend expected self-signed TLS rejection marker, got {:?}",
            backend,
            lines[0]
        );
        assert_eq!(
            lines[1], "0",
            "{} backend expected default status for rejected HTTPS response",
            backend
        );
    }
}

#[test]
fn test_os_process_gorillax_three_way_parity() {
    let source = r#"
okRun <= run("echo", @["hello"])
stdout(okRun.hasValue().toString())
stdout(okRun.__value.code.toString())

badRun <= run("/nonexistent_program_xyz", @[])
stdout(badRun.hasValue().toString())

okShell <= execShell("echo shell")
stdout(okShell.hasValue().toString())
stdout(okShell.__value.code.toString())

badShell <= execShell("exit 7")
stdout(badShell.hasValue().toString())
"#;
    assert_backend_parity_for_source(source, "os_process_gorillax");
}

#[test]
fn test_cage_molten_three_way_parity() {
    let source = r#"
m <= Molten[]()

toInt x = 7 => :Int

boom x =
  Result[0](throw <= "boom") ]=> y
  0
=> :Int

ok <= Cage[m, toInt]()
stdout(ok.hasValue().toString())
ok ]=> value
stdout(value.toString())

bad <= Cage[m, boom]()
stdout(bad.hasValue().toString())
"#;
    assert_backend_parity_for_source(source, "cage_molten");
}

#[test]
fn test_molten_type_args_rejected_three_way() {
    let source = r#"
m <= Molten[1]()
stdout(m.__type)
"#;
    assert_backends_reject_source(source, "molten_type_args_rejected");
}

#[test]
fn test_race_empty_three_way_parity() {
    let source = r#"
r <= Race[@[]]()
stdout(r.toString())
"#;
    assert_backend_parity_for_source(source, "race_empty");
}

#[test]
fn test_async_aggregation_shape_three_way_parity() {
    let cases = vec![
        (
            "all_empty_shape",
            r#"
a <= All[@[]]()
stdout(a.toString())
"#,
        ),
        (
            "all_values_shape",
            r#"
a <= All[@[1, 2, 3]]()
stdout(a.toString())
"#,
        ),
        (
            "race_values_shape",
            r#"
r <= Race[@[Async[1](), Async[2]()]]()
stdout(r.toString())
"#,
        ),
        (
            "timeout_async_shape",
            r#"
t <= Timeout[Async[1](), 10]()
stdout(t.toString())
"#,
        ),
    ];

    for (label, source) in cases {
        assert_backend_parity_for_source(source, label);
    }
}

#[test]
fn test_async_cancel_three_way_parity() {
    let source = r#"
handleCancel unused =
  |== e: Error =
    stdout(e.type)
  => :Unit

  c <= Cancel[sleep(1000)]()
  c ]=> ignored
=> :Unit

handleCancel(0)
"#;
    assert_backend_parity_for_source(source, "async_cancel");
    let out = run_interpreter_src(source, "async_cancel_expected")
        .expect("interpreter output should exist for async cancel");
    assert_eq!(out, "CancelledError");
}

#[test]
fn test_argv_three_way_parity() {
    let source = r#"
args <= argv()
stdout(args.isEmpty().toString())
"#;
    assert_backend_parity_for_source(source, "argv_no_extra_args");
    let out = run_interpreter_src(source, "argv_expected")
        .expect("interpreter output should exist for argv");
    assert_eq!(out, "true");
}

#[test]
fn test_sha256_three_way_parity() {
    let project_dir = std::env::temp_dir().join(format!(
        "taida_sha256_pkg_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos()
    ));
    std::fs::create_dir_all(&project_dir).expect("create temp project dir");
    let main_path = project_dir.join("main.td");
    let packages_path = project_dir.join("packages.tdm");

    let source = r#"
>>> taida-lang/crypto => @(sha256)
stdout(sha256("abc"))
"#;
    std::fs::write(&main_path, source).expect("write main.td");
    std::fs::write(&packages_path, ">>> taida-lang/crypto@a.1\n").expect("write packages.tdm");

    let deps_output = Command::new(taida_bin())
        .current_dir(&project_dir)
        .arg("deps")
        .output()
        .expect("run taida deps");
    assert!(
        deps_output.status.success(),
        "taida deps failed: {}",
        String::from_utf8_lossy(&deps_output.stderr)
    );

    let interp = run_interpreter(&main_path).expect("interpreter output should exist for sha256");
    let native = run_native(&main_path).expect("native output should exist for sha256");
    assert_eq!(interp, native, "interpreter/native mismatch for sha256");

    if node_available() {
        let js = run_js(&main_path).expect("js output should exist for sha256");
        assert_eq!(interp, js, "interpreter/js mismatch for sha256");
    }

    assert_eq!(
        interp,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );

    let _ = std::fs::remove_dir_all(&project_dir);
}

#[test]
fn test_hashmap_variable_non_string_key_three_way_parity() {
    let source = r#"
k <= 1
m <= hashMap().set(k, 2)
stdout(m.get(1).hasValue().toString())
stdout(m.get(1).getOrDefault(99).toString())
"#;
    assert_backend_parity_for_source(source, "hashmap_variable_non_string_key");
}

#[test]
fn test_num_bitwise_radix_three_way_parity() {
    let source = r#"
n1 <= 0x10
stdout(n1.toString())
n2 <= 0o17
stdout(n2.toString())
n3 <= 0b1010
stdout(n3.toString())
n4 <= 1_000
stdout(n4.toString())
f <= Int[1e3]()
stdout(f.__value.toString())

stdout(BitAnd[6, 3]().toString())

s1 <= ShiftL[1, 40]()
stdout(s1.getOrDefault(-1).toString())
s2 <= ShiftL[1, 64]()
stdout(s2.getOrDefault(999).toString())

r1 <= ToRadix[255, 16]()
stdout(r1.getOrDefault("bad"))
r2 <= ToRadix[10, 1]()
stdout(r2.getOrDefault("bad"))

i1 <= Int["ff", 16]()
stdout(i1.getOrDefault(-1).toString())
i2 <= Int["2", 2]()
stdout(i2.getOrDefault(-1).toString())
"#;
    assert_backend_parity_for_source(source, "num_bitwise_radix");
    let out = run_interpreter_src(source, "num_bitwise_radix_expected")
        .expect("interpreter output should exist");
    assert_eq!(
        out,
        "16\n15\n10\n1000\n1000\n2\n1099511627776\n999\nff\nbad\n255\n-1"
    );
}

#[test]
fn test_endian_pack_unpack_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping endian pack/unpack parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    let source = r#"
u16be <= U16BE[513]()
stdout(u16be.hasValue.toString())
u16be ]=> b16be
u16be_dec <= U16BEDecode[b16be]()
u16be_dec ]=> v16be
stdout(v16be.toString())

u16le <= U16LE[513]()
u16le ]=> b16le
u16le_dec <= U16LEDecode[b16le]()
u16le_dec ]=> v16le
stdout(v16le.toString())

u32be <= U32BE[16909060]()
stdout(u32be.hasValue.toString())
u32be ]=> b32be
u32be_dec <= U32BEDecode[b32be]()
u32be_dec ]=> v32be
stdout(v32be.toString())

u32le <= U32LE[16909060]()
u32le ]=> b32le
u32le_dec <= U32LEDecode[b32le]()
u32le_dec ]=> v32le
stdout(v32le.toString())

bad16 <= U16BE[-1]()
stdout(bad16.hasValue.toString())
bad32 <= U32BE[-1]()
stdout(bad32.hasValue.toString())

shortLax <= Bytes[@[1]]()
shortLax ]=> short
badDec16 <= U16BEDecode[short]()
stdout(badDec16.hasValue.toString())

threeLax <= Bytes[@[1, 2, 3]]()
threeLax ]=> three
badDec32 <= U32LEDecode[three]()
stdout(badDec32.hasValue.toString())
"#;

    for backend in backends {
        let out = match backend {
            "interp" => run_interpreter_src(source, "endian_pack_unpack_interp"),
            "js" => run_js_src(source, "endian_pack_unpack_js"),
            "native" => run_native_src(source, "endian_pack_unpack_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for endian pack/unpack parity", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            10,
            "{} backend output shape mismatch for endian pack/unpack parity: {:?}",
            backend,
            out
        );
        assert!(
            lines[0] == "true" || lines[0] == "1",
            "{} backend expected U16BE success marker, got {:?}",
            backend,
            lines[0]
        );
        assert_eq!(
            lines[1], "513",
            "{} backend expected U16BEDecode value",
            backend
        );
        assert_eq!(
            lines[2], "513",
            "{} backend expected U16LEDecode value",
            backend
        );
        assert!(
            lines[3] == "true" || lines[3] == "1",
            "{} backend expected U32BE success marker, got {:?}",
            backend,
            lines[3]
        );
        assert_eq!(
            lines[4], "16909060",
            "{} backend expected U32BEDecode value",
            backend
        );
        assert_eq!(
            lines[5], "16909060",
            "{} backend expected U32LEDecode value",
            backend
        );
        assert!(
            lines[6] == "false" || lines[6] == "0",
            "{} backend expected U16BE failure marker for negative input, got {:?}",
            backend,
            lines[6]
        );
        assert!(
            lines[7] == "false" || lines[7] == "0",
            "{} backend expected U32BE failure marker for negative input, got {:?}",
            backend,
            lines[7]
        );
        assert!(
            lines[8] == "false" || lines[8] == "0",
            "{} backend expected U16BEDecode failure marker for short bytes, got {:?}",
            backend,
            lines[8]
        );
        assert!(
            lines[9] == "false" || lines[9] == "0",
            "{} backend expected U32LEDecode failure marker for short bytes, got {:?}",
            backend,
            lines[9]
        );
    }
}

#[test]
fn test_bytes_cursor_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping bytes cursor parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    let source = r#"
packetLax <= Bytes[@[2, 79, 75, 4, 80, 73, 78, 71]]()
packetLax ]=> packet
cursor0 <= BytesCursor[packet]()
stdout(cursor0.offset.toString())
stdout(BytesCursorRemaining[cursor0]().toString())

len1Step <= BytesCursorU8[cursor0]()
len1Step ]=> len1Pair
len1 <= len1Pair.value
cursor1 <= len1Pair.cursor
stdout(len1.toString())
stdout(cursor1.offset.toString())

msg1Step <= BytesCursorTake[cursor1, len1]()
msg1Step ]=> msg1Pair
msg1Bytes <= msg1Pair.value
cursor2 <= msg1Pair.cursor
msg1 <= Utf8Decode[msg1Bytes]()
stdout(msg1.getOrDefault("bad"))
stdout(cursor2.offset.toString())

negTake <= BytesCursorTake[cursor2, -1]()
stdout(negTake.__default.cursor.offset.toString())

len2Step <= BytesCursorU8[cursor2]()
len2Step ]=> len2Pair
len2 <= len2Pair.value
cursor3 <= len2Pair.cursor
msg2Step <= BytesCursorTake[cursor3, len2]()
msg2Step ]=> msg2Pair
msg2Bytes <= msg2Pair.value
cursor4 <= msg2Pair.cursor
msg2 <= Utf8Decode[msg2Bytes]()
stdout(msg2.getOrDefault("bad"))
stdout(BytesCursorRemaining[cursor4]().toString())

overflow <= BytesCursorTake[cursor4, 1]()
stdout(overflow.hasValue.toString())
"#;

    for backend in backends {
        let out = match backend {
            "interp" => run_interpreter_src(source, "bytes_cursor_interp"),
            "js" => run_js_src(source, "bytes_cursor_js"),
            "native" => run_native_src(source, "bytes_cursor_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for bytes cursor parity", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            10,
            "{} backend output shape mismatch for bytes cursor parity: {:?}",
            backend,
            out
        );
        assert_eq!(
            lines[0], "0",
            "{} backend expected initial offset 0",
            backend
        );
        assert_eq!(lines[1], "8", "{} backend expected remaining 8", backend);
        assert_eq!(lines[2], "2", "{} backend expected len1 2", backend);
        assert_eq!(
            lines[3], "1",
            "{} backend expected cursor1 offset 1",
            backend
        );
        assert_eq!(
            lines[4], "OK",
            "{} backend expected first frame payload",
            backend
        );
        assert_eq!(
            lines[5], "3",
            "{} backend expected cursor2 offset 3",
            backend
        );
        assert_eq!(
            lines[6], "3",
            "{} backend expected negative take default cursor offset to stay at 3",
            backend
        );
        assert_eq!(
            lines[7], "PING",
            "{} backend expected second frame payload",
            backend
        );
        assert_eq!(
            lines[8], "0",
            "{} backend expected remaining 0 after parse",
            backend
        );
        assert!(
            lines[9] == "false" || lines[9] == "0",
            "{} backend expected overflow take failure marker, got {:?}",
            backend,
            lines[9]
        );
    }
}

#[test]
fn test_bytes_three_way_parity() {
    let source = r#"
emptyLax <= Bytes[@[]]()
emptyLax ]=> emptyBytes
b0 <= Bytes[4](fill <= 65)
stdout(b0.getOrDefault(emptyBytes).length().toString())
b0 ]=> b
stdout(b.length().toString())
g <= b.get(1)
stdout(g.getOrDefault(-1).toString())

badFill <= Bytes[2](fill <= 300)
stdout(badFill.getOrDefault(emptyBytes).length().toString())

okSet <= ByteSet[b, 2, 66]()
b2 <= okSet.getOrDefault(emptyBytes)
stdout(b2.length().toString())
stdout(b2.get(2).getOrDefault(-1).toString())

badIdx <= ByteSet[b, 9, 1]()
stdout(badIdx.getOrDefault(emptyBytes).length().toString())
badVal <= ByteSet[b, 0, 999]()
stdout(badVal.getOrDefault(emptyBytes).length().toString())

slice <= Slice[b](start <= 1, end <= 3)
stdout(slice.length().toString())
joined <= Concat[b, slice]()
stdout(joined.length().toString())
lst <= BytesToList[b]()
stdout(lst.length().toString())
"#;
    assert_backend_parity_for_source(source, "bytes_molds");
    let out = run_interpreter_src(source, "bytes_molds_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "4\n4\n65\n0\n4\n66\n0\n0\n2\n6\n4");
}

#[test]
fn test_char_codepoint_three_way_parity() {
    let source = r#"
c1 <= Char[65]()
stdout(c1.getOrDefault("?"))

c2 <= Char["A"]()
stdout(c2.getOrDefault("?"))
cp1 <= CodePoint["A"]()
stdout(cp1.getOrDefault(-1).toString())

sur <= Char[55296]()
stdout(sur.getOrDefault("?"))
cpBad <= CodePoint["AB"]()
stdout(cpBad.getOrDefault(-1).toString())
comb <= CodePoint["é"]()
stdout(comb.getOrDefault(-1).toString())

emoji <= Char[128512]()
stdout(emoji.getOrDefault("?"))
emojiCp <= CodePoint["😀"]()
stdout(emojiCp.getOrDefault(-1).toString())
"#;
    assert_backend_parity_for_source(source, "char_codepoint");
    let out = run_interpreter_src(source, "char_codepoint_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "A\nA\n65\n?\n-1\n-1\n😀\n128512");
}

#[test]
fn test_utf8_molds_three_way_parity() {
    let source = r#"
emptyLax <= Bytes[@[]]()
emptyLax ]=> emptyBytes
enc <= Utf8Encode["pong"]()
stdout(enc.getOrDefault(emptyBytes).length().toString())
enc ]=> eb
stdout(eb.toString())

dec <= Utf8Decode[eb]()
stdout(dec.getOrDefault("bad"))

badLax <= Bytes[@[255, 255]]()
badLax ]=> badBytes
badDec <= Utf8Decode[badBytes]()
stdout(badDec.getOrDefault("bad"))
"#;
    assert_backend_parity_for_source(source, "utf8_molds");
    let out = run_interpreter_src(source, "utf8_molds_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "4\nBytes[@[112, 111, 110, 103]]\npong\nbad");
}

#[test]
fn test_time_nowms_sleep_three_way_parity() {
    let has_node = node_available();
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping time parity");
        return;
    }

    let backends = if has_node {
        vec!["interp", "js", "native"]
    } else {
        vec!["interp", "native"]
    };

    let source = r#"
a <= nowMs()
s <= sleep(20)
s ]=> waited
b <= nowMs()
stdout(a.toString())
stdout(b.toString())
stdout((b >= a).toString())
"#;

    for backend in backends {
        let out = match backend {
            "interp" => run_interpreter_src(source, "time_nowms_sleep_interp"),
            "js" => run_js_src(source, "time_nowms_sleep_js"),
            "native" => run_native_src(source, "time_nowms_sleep_native"),
            _ => None,
        }
        .unwrap_or_else(|| panic!("{} backend failed for time parity", backend));

        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "{} backend output shape mismatch for time parity: {:?}",
            backend,
            lines
        );

        let a: i64 = lines[0]
            .parse()
            .unwrap_or_else(|_| panic!("{} backend a is not Int: {:?}", backend, lines[0]));
        let b: i64 = lines[1]
            .parse()
            .unwrap_or_else(|_| panic!("{} backend b is not Int: {:?}", backend, lines[1]));
        assert_eq!(lines[2], "true", "{} backend expected b >= a", backend);
        assert!(
            b >= a,
            "{} backend expected monotonic nowMs: {} -> {}",
            backend,
            a,
            b
        );

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_millis() as i64;
        let drift = (b - now).abs();
        assert!(
            drift < 300_000,
            "{} backend nowMs drift too large: b={}, host_now={}, drift_ms={}",
            backend,
            b,
            now,
            drift
        );
    }
}

#[test]
fn test_time_sleep_all_inside_function_three_way_parity() {
    let source = r#"
waitBoth p =
  all <= All[@[sleep(0), sleep(0)]]()
  all ]=> vals
  vals.length().toString()
=> :Str

out <= waitBoth(0)
out ]=> value
stdout(value)
"#;

    assert_backend_parity_for_source(source, "time_sleep_all_function");
    let out = run_interpreter_src(source, "time_sleep_all_function_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "2");
}

#[test]
fn test_time_sleep_timeout_via_var_three_way_parity() {
    let source = r#"
waitWithTimeout p =
  s <= sleep(0)
  t <= Timeout[s, 100]()
  t ]=> _done
  "ok"
=> :Str

out <= waitWithTimeout(0)
out ]=> value
stdout(value)
"#;

    assert_backend_parity_for_source(source, "time_sleep_timeout_via_var");
    let out = run_interpreter_src(source, "time_sleep_timeout_via_var_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "ok");
}

#[test]
fn test_sleep_boundary_errors_three_way_parity() {
    let source = r#"
ok <= sleep(0)
stdout(ok.isRejected().toString())
neg <= sleep(-1)
stdout(neg.isRejected().toString())
big <= sleep(2147483648)
stdout(big.isRejected().toString())
"#;
    assert_backend_parity_for_source(source, "sleep_boundary_errors");
    let out = run_interpreter_src(source, "sleep_boundary_errors_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "false\ntrue\ntrue");
}

#[test]
fn test_custom_mold_solidify_override_three_way_parity() {
    let source = r#"
Mold[T] => PlusOne[T] = @(
  solidify =
    filling + 1
  => :Int
)
stdout(PlusOne[41]().toString())
"#;
    assert_backend_parity_for_source(source, "custom_mold_solidify_override_three_way");
    let out = run_interpreter_src(source, "custom_mold_solidify_override_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "42");
}

#[test]
fn test_custom_mold_required_positional_binding_three_way_parity() {
    let source = r#"
Mold[T] => Pair[T, U] = @(
  second: U
  solidify =
    filling + second
  => :Int
)
stdout(Pair[40, 2]().toString())
"#;
    assert_backend_parity_for_source(source, "custom_mold_required_positional_binding_three_way");
    let out = run_interpreter_src(source, "custom_mold_required_positional_binding_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "42");
}

#[test]
fn test_inherited_custom_mold_required_positional_binding_three_way_parity() {
    let source = r#"
Mold[T] => PairBase[T] = @()
PairBase[T] => Pair[T, U] = @(
  second: U
  solidify =
    filling + second
  => :Int
)
stdout(Pair[40, 2]().toString())
"#;
    assert_backend_parity_for_source(
        source,
        "inherited_custom_mold_required_positional_binding_three_way",
    );
    let out = run_interpreter_src(
        source,
        "inherited_custom_mold_required_positional_binding_expected",
    )
    .expect("interpreter output should exist");
    assert_eq!(out, "42");
}

#[test]
fn test_inherited_custom_mold_override_parent_field_three_way_parity() {
    let source = r#"
Mold[T] => Base[T] = @(
  bonus: Int <= 0
  solidify =
    filling + bonus
  => :Int
)
Base[T] => Child[T] = @(
  bonus: Int
)
stdout(Child[40, 2]().toString())
"#;
    assert_backend_parity_for_source(
        source,
        "inherited_custom_mold_override_parent_field_three_way",
    );
    let out = run_interpreter_src(
        source,
        "inherited_custom_mold_override_parent_field_expected",
    )
    .expect("interpreter output should exist");
    assert_eq!(out, "42");
}

#[test]
fn test_custom_mold_solidify_throw_caught_three_way_parity() {
    let source = r#"
Mold[T] => AlwaysFail[T] = @(
  solidify =
    Error(type <= "SolidifyError", message <= "fail").throw()
  => :Int
)
check x: Int =
  |== err: Error =
    -1
  => :Int
  AlwaysFail[x]()
=> :Int
stdout(check(7).toString())
"#;
    assert_backend_parity_for_source(source, "custom_mold_solidify_throw_caught_three_way");
    let out = run_interpreter_src(source, "custom_mold_solidify_throw_caught_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "-1");
}

#[test]
fn test_custom_mold_definition_errors_rejected_three_way() {
    let cases = [
        (
            "custom_mold_def_missing_type_or_default",
            r#"
Mold[T] => BadField[T] = @(
  raw
)
"#,
        ),
        (
            "custom_mold_def_unbound_type_param",
            r#"
Mold[T] => BadBind[T, U] = @()
"#,
        ),
    ];

    for (label, source) in cases {
        assert_backends_reject_source(source, label);
    }
}

#[test]
fn test_function_default_args_three_way_parity() {
    let source = r#"
sum3 a: Int b: Int <= 10 c: Int <= a + b =
  a + b + c
=> :Int

stdout(sum3().toString())
stdout(sum3(1).toString())
stdout(sum3(1, 2).toString())
"#;
    assert_backend_parity_for_source(source, "function_default_args_three_way");
    let out = run_interpreter_src(source, "function_default_args_three_way_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "20\n22\n6");
}

#[test]
fn test_function_too_many_args_rejected_three_way() {
    let source = r#"
id x: Int =
  x
=> :Int

stdout(id(1, 2).toString())
"#;
    assert_backends_reject_source(source, "function_too_many_args_rejected_three_way");
}

#[test]
fn test_todo_stub_three_way_parity() {
    let source = r#"
a <= TODO[Int](id <= "TASK-1", task <= "use unm", sol <= 7, unm <= 9)
a ]=> av
stdout(av.toString())

b <= TODO[Int](sol <= 5)
b ]=> bv
stdout(bv.toString())

c <= TODO[Stub["User data TBD"]]()
c ]=> cv
g <= Cage[cv, _ m = 1]()
g ]=> gv
stdout(gv.toString())
"#;
    assert_backend_parity_for_source(source, "todo_stub_three_way");
    let out = run_interpreter_src(source, "todo_stub_three_way_expected")
        .expect("interpreter output should exist");
    assert_eq!(out, "9\n0\n1");
}

#[test]
fn test_string_contains_field_access_in_lambda_three_way_parity() {
    let source = r#"
Todo = @(id: Int, title: Str, done: Bool)
items <= @[
  @(id <= 1, title <= "renamed task", done <= false)
]
matched <= Filter[items, _ x = x.title.contains("renamed")]()
stdout(matched.length().toString())
"#;
    assert_backend_parity_for_source(source, "contains_field_access_lambda_three_way");
}

#[test]
fn test_three_way_parity_boundary_abnormal_and_complex_cases() {
    let cases = [
        (
            "flatten_empty",
            r#"
flat <= Flatten[@[]]()
stdout(flat)
"#,
        ),
        (
            "flatten_nested_empty",
            r#"
flat <= Flatten[@[@[], @[1], @[]]]()
stdout(flat)
"#,
        ),
        (
            "flatten_nested_lists",
            r#"
flat <= Flatten[@[@[1], @[2], @[3], @[4, 5]]]()
stdout(flat)
"#,
        ),
        (
            "unmold_negative_boundary",
            r#"
x <= -2147483648
x ]=> y
stdout(y.toString())
"#,
        ),
        (
            "hashmap_empty_lookup",
            r#"
m <= hashMap()
stdout(m.get("missing").hasValue().toString())
stdout(m.get("missing").getOrDefault(99).toString())
"#,
        ),
        (
            "hashmap_set_remove_cycle",
            r#"
m0 <= hashMap()
m1 <= m0.set("k", 1)
m2 <= m1.remove("k")
stdout(m2.get("k").hasValue().toString())
stdout(m2.get("k").getOrDefault(99).toString())
"#,
        ),
        (
            "hashmap_many_entries",
            r#"
m <= hashMap()
m1 <= m.set("k1", 1)
m2 <= m1.set("k2", 2)
m3 <= m2.set("k3", 3)
m4 <= m3.set("k4", 4)
m5 <= m4.set("k5", 5)
m6 <= m5.set("k6", 6)
m7 <= m6.set("k7", 7)
m8 <= m7.set("k8", 8)
m9 <= m8.set("k9", 9)
m10 <= m9.set("k10", 10)
m11 <= m10.set("k11", 11)
m12 <= m11.set("k12", 12)
m13 <= m12.set("k13", 13)
m14 <= m13.set("k14", 14)
m15 <= m14.set("k15", 15)
m16 <= m15.set("k16", 16)
m17 <= m16.set("k17", 17)
m18 <= m17.set("k18", 18)
m19 <= m18.set("k19", 19)
m20 <= m19.set("k20", 20)
m21 <= m20.set("k21", 21)
m22 <= m21.set("k22", 22)
m23 <= m22.set("k23", 23)
m24 <= m23.set("k24", 24)
m25 <= m24.set("k25", 25)
m26 <= m25.set("k26", 26)
m27 <= m26.set("k27", 27)
m28 <= m27.set("k28", 28)
m29 <= m28.set("k29", 29)
m30 <= m29.set("k30", 30)
stdout(m30.get("k1").getOrDefault(0).toString())
stdout(m30.get("k15").getOrDefault(0).toString())
stdout(m30.get("k30").getOrDefault(0).toString())
"#,
        ),
        (
            "hashmap_long_key",
            r#"
m <= hashMap().set("kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk", 77)
stdout(m.get("kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk").getOrDefault(0).toString())
"#,
        ),
        (
            "all_mixed_sync_async",
            r#"
a <= All[@[1, Async[2](), 3, Async[4]()]]()
a ]=> r
stdout(r)
"#,
        ),
    ];

    for (label, source) in cases {
        assert_backend_parity_for_source(source, label);
    }
}

#[test]
fn test_typedef_implicit_type_default_injection_three_way_parity() {
    let source = r#"
Pilot = @(
  name: Str
  age: Int
  callSign: Str
)

pilot <= Pilot(name <= "Rei")
stdout(pilot.name)
stdout(pilot.age.toString())
stdout(pilot.callSign.length().toString())
"#;
    assert_backend_parity_for_source(source, "typedef_implicit_type_default_injection");
}

#[test]
fn test_inheritance_implicit_type_default_injection_three_way_parity() {
    let source = r#"
Pilot = @(
  name: Str
  age: Int
)

Pilot => Officer = @(
  rank: Int
  department: Str
)

officer <= Officer(name <= "Asuka")
stdout(officer.name)
stdout(officer.age.toString())
stdout(officer.rank.toString())
stdout(officer.department)
"#;
    assert_backend_parity_for_source(source, "inheritance_implicit_type_default_injection");
}

// ── F-44 regression: web server HTTP endpoints across all 3 backends ──

/// Minimal web server source that handles /health, /issues, POST /issues
/// and outputs the response bodies to stdout (one request per invocation).
fn web_server_single_request_source(port: u16) -> String {
    format!(
        r#"
respondAndClose socket body =
  wire <= "HTTP/1.1 200 OK\r\nContent-Length: " + body.length().toString() + "\r\nConnection: close\r\n\r\n" + body
  sendAsync <= socketSendAll(socket, wire, 5000)
  sendAsync ]=> _sent
  closeAsync <= socketClose(socket)
  closeAsync ]=> _closed
  0
=> :Int

main dummy =
  |== error: Error =
    stderr("fatal: " + error.type + " - " + error.message)
    1
  => :Int
  listenAsync <= tcpListen({port}, 1000)
  listenAsync ]=> listener
  stdout("listening")
  acceptAsync <= tcpAccept(listener.__value.listener, 5000)
  acceptAsync ]=> accepted
  recvAsync <= socketRecv(accepted.__value.socket, 5000)
  recvAsync ]=> _req
  jsonBody <= jsonEncode(@(ok <= true))
  respondAndClose(accepted.__value.socket, jsonBody)
  lcloseAsync <= listenerClose(listener.__value.listener)
  lcloseAsync ]=> _lclosed
  0
=> :Int

main(0)
"#
    )
}

/// Helper: start a backend process, wait for "listening", send an HTTP request, return response body.
fn run_web_backend(backend: &str, source: &str, port: u16) -> Option<String> {
    let has_node = node_available();
    let has_cc = cc_available();

    let tmp_td = unique_temp_path("taida_web", backend, "td");
    fs::write(&tmp_td, source).ok()?;

    let mut child: Child = match backend {
        "interp" => Command::new(taida_bin())
            .arg(&tmp_td)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .ok()?,
        "js" => {
            if !has_node {
                let _ = fs::remove_file(&tmp_td);
                return None;
            }
            let tmp_mjs = unique_temp_path("taida_web_js", backend, "mjs");
            let build_out = Command::new(taida_bin())
                .args(["build", "--target", "js"])
                .arg(&tmp_td)
                .arg("-o")
                .arg(&tmp_mjs)
                .output()
                .ok()?;
            if !build_out.status.success() {
                let _ = fs::remove_file(&tmp_td);
                let _ = fs::remove_file(&tmp_mjs);
                return None;
            }
            let c = Command::new("node")
                .arg(&tmp_mjs)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .ok()?;
            let _ = fs::remove_file(&tmp_td);
            // Keep tmp_mjs for now, clean after
            c
        }
        "native" => {
            if !has_cc {
                let _ = fs::remove_file(&tmp_td);
                return None;
            }
            let tmp_bin = unique_temp_path("taida_web_native", backend, "bin");
            let build_out = Command::new(taida_bin())
                .args(["build", "--target", "native"])
                .arg(&tmp_td)
                .arg("-o")
                .arg(&tmp_bin)
                .output()
                .ok()?;
            if !build_out.status.success() {
                let _ = fs::remove_file(&tmp_td);
                let _ = fs::remove_file(&tmp_bin);
                return None;
            }
            let c = Command::new(&tmp_bin)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .ok()?;
            let _ = fs::remove_file(&tmp_td);
            c
        }
        _ => return None,
    };

    // Wait for server to start listening
    thread::sleep(Duration::from_millis(1500));

    // Send GET /health request
    let response = match TcpStream::connect(("127.0.0.1", port)) {
        Ok(mut stream) => {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            let _ = stream.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
            let mut buf = Vec::new();
            let _ = stream.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).to_string()
        }
        Err(_) => {
            let _ = child.kill();
            return None;
        }
    };

    // Wait for process to exit (it handles only 1 request)
    // Give it a few seconds then kill
    thread::sleep(Duration::from_secs(2));
    let _ = child.kill();
    let _ = child.wait();

    // Extract body from HTTP response
    if let Some(pos) = response.find("\r\n\r\n") {
        Some(response[pos + 4..].to_string())
    } else {
        Some(response)
    }
}

#[test]
fn test_f44_web_server_health_three_way_parity() {
    let has_cc = cc_available();
    if !has_cc {
        eprintln!("SKIP: cc not available, skipping F-44 web server parity");
        return;
    }

    let mut results: Vec<(String, String)> = Vec::new();

    for backend in &["interp", "native", "js"] {
        let port = find_free_loopback_port();
        let source = web_server_single_request_source(port);
        let body = run_web_backend(backend, &source, port);
        match body {
            Some(b) => results.push((backend.to_string(), b)),
            None => {
                if *backend == "js" && !node_available() {
                    continue;
                }
                panic!("{} backend failed for F-44 web server parity", backend);
            }
        }
    }

    // All backends must return {"ok":true}
    for (backend, body) in &results {
        assert_eq!(
            body.trim(),
            r#"{"ok":true}"#,
            "F-44: {} backend /health response mismatch",
            backend
        );
    }

    // All backends must match each other
    if results.len() >= 2 {
        let first_body = &results[0].1;
        for (backend, body) in &results[1..] {
            assert_eq!(
                body.trim(),
                first_body.trim(),
                "F-44: {}/{} parity mismatch",
                results[0].0,
                backend
            );
        }
    }
}

// ── F-45: jsonPretty must not include __type in output ────────────────
// Typed BuchiPack in a list should serialize without __type field.
// See: https://github.com/taida-lang/taida/issues/F-45

#[test]
fn f45_json_pretty_typed_list_no_type_field() {
    let source = r#"
Todo = @(id: Int, title: Str, done: Bool)
items <= @[Todo(id <= 1, title <= "hello", done <= false)]
stdout(jsonPretty(items))
"#;
    // All 3 backends must produce the same output without __type
    assert_backend_parity_for_source(source, "f45_typed_list");

    // Additionally verify __type is NOT present in the output
    let interp =
        run_interpreter_src(source, "f45_typed_list_check").expect("interpreter should succeed");
    assert!(
        !interp.contains("__type"),
        "F-45: jsonPretty output must not contain __type, got: {}",
        interp
    );
}

#[test]
fn f45_json_pretty_typed_single_no_type_field() {
    let source = r#"
Todo = @(id: Int, title: Str, done: Bool)
item <= Todo(id <= 1, title <= "hello", done <= false)
stdout(jsonPretty(item))
"#;
    assert_backend_parity_for_source(source, "f45_typed_single");

    let interp =
        run_interpreter_src(source, "f45_typed_single_check").expect("interpreter should succeed");
    assert!(
        !interp.contains("__type"),
        "F-45: jsonPretty output must not contain __type, got: {}",
        interp
    );
}

#[test]
fn f45_json_encode_typed_list_no_type_field() {
    let source = r#"
Todo = @(id: Int, title: Str, done: Bool)
items <= @[Todo(id <= 1, title <= "hello", done <= false)]
stdout(jsonEncode(items))
"#;
    assert_backend_parity_for_source(source, "f45_encode_typed_list");

    let interp = run_interpreter_src(source, "f45_encode_typed_list_check")
        .expect("interpreter should succeed");
    assert!(
        !interp.contains("__type"),
        "F-45: jsonEncode output must not contain __type, got: {}",
        interp
    );
}

/// F-47 regression: collect_free_vars_inner did not traverse BuchiPack, TypeInst,
/// ListLit, MoldInst, Unmold, Throw, or nested Lambda expressions, causing
/// captured variables to be silently replaced with 0/default in Native closures.
#[test]
fn test_f47_lambda_capture_into_buchi_pack_three_way_parity() {
    // BuchiPack capture
    let source1 = r#"
Todo = @(id: Int, title: Str, done: Bool)
doUpdate reqId reqTitle reqDone =
  items <= @[@(id <= 1, title <= "original", done <= false)]
  mapper <= _ item = | item.id == reqId |> @(id <= reqId, title <= reqTitle, done <= reqDone) | _ |> item
  newItems <= Map[items, mapper]()
  found <= Find[newItems, _ item = item.id == reqId]()
  jsonPretty(found.__value)
=> :Str
stdout(doUpdate(1, "updated", true))
"#;
    assert_backend_parity_for_source(source1, "f47_buchi_pack_capture");

    // ListLit capture
    let source2 = r#"
test1 a b =
  items <= @[1]
  mapper <= _ item = @[a, b, item]
  Map[items, mapper]() => result
  stdout(jsonPretty(result))
  0
=> :Int
test1(10, 20)
"#;
    assert_backend_parity_for_source(source2, "f47_list_lit_capture");

    // Nested lambda capture
    let source3 = r#"
test1 x =
  items <= @[1, 2, 3]
  mapper <= _ item = Map[@[item], _ inner = inner + x]()
  Map[items, mapper]() => result
  stdout(jsonPretty(Flatten[result]()))
  0
=> :Int
test1(100)
"#;
    assert_backend_parity_for_source(source3, "f47_nested_lambda_capture");
}

/// F-52 regression: jsonEncode via module-imported closure must produce identical
/// output across all 3 backends. Library modules that use BuchiPack literals must
/// register their field names at runtime so native jsonEncode can look them up.
#[test]
fn test_f52_json_encode_via_module_closure_three_way_parity() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("taida_f52_parity_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    // handlers.td — library module that exports a function using jsonEncode
    fs::write(
        dir.join("handlers.td"),
        r#"<<< @(handleRoot)

handleRoot dummy =
  jsonEncode(@(ok <= true, service <= "taida.dev"))
=> :Str
"#,
    )
    .expect("write handlers.td");

    // router.td — library module that re-exports via import chain
    fs::write(
        dir.join("router.td"),
        r#">>> ./handlers.td => @(handleRoot)
<<< @(route)

route path =
  handleRoot(0)
=> :Str
"#,
    )
    .expect("write router.td");

    // main.td — main module that imports and calls
    fs::write(
        dir.join("main.td"),
        r#">>> ./router.td => @(route)

result <= route("/")
stdout(result)
"#,
    )
    .expect("write main.td");

    let main_path = dir.join("main.td");

    // Interpreter (reference)
    let interp = run_interpreter(&main_path).expect("F-52 parity: interpreter should succeed");

    // Native
    let native = run_native(&main_path).expect("F-52 parity: native should succeed");
    assert_eq!(
        interp, native,
        "F-52 parity: interpreter/native mismatch — interp='{}', native='{}'",
        interp, native
    );

    if node_available() {
        let js = run_js_project(&main_path, "f52_module_closure")
            .expect("F-52 parity: js should succeed");
        assert_eq!(
            interp, js,
            "F-52 parity: interpreter/js mismatch — interp='{}', js='{}'",
            interp, js
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

/// F-55 regression: Native string equality uses strcmp, not pointer comparison
#[test]
fn test_f55_string_equality_three_way_parity() {
    let src = r#"
x <= "GET"
stdout(x == "GET")
stdout(x != "POST")

check a = a == "hello" => :Bool
stdout(check("hello"))
stdout(check("world"))

eq a b = a == b => :Bool
stdout(eq("foo", "foo"))
stdout(eq("foo", "bar"))

route method path =
  | method == "GET" && path == "/" |> "root"
  | _ |> "other"
=> :Str
stdout(route("GET", "/"))
stdout(route("POST", "/"))

stdout(42 == 42)
stdout(42 == 0)
"#;
    let interp = run_interpreter_src(src, "f55_str_eq").expect("interpreter should succeed");
    let native = run_native_src(src, "f55_str_eq").expect("native should succeed");
    assert_eq!(
        interp, native,
        "F-55: string equality interp/native mismatch\ninterp: {}\nnative: {}",
        interp, native
    );
}

/// F-56 regression: Module export function closures should include all module-level symbols.
/// When a module exports `createDefaultKv`, its internal calls to `makeKv` should work
/// even if the importer does not explicitly import `makeKv`.
#[test]
fn test_f56_module_closure_captures_sibling_exports() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("taida_f56_parity_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    // kvlib.td — exports makeKv and createDefaultKv
    // createDefaultKv calls makeKv, and makeKv's returned lambdas call makeKv recursively
    fs::write(
        dir.join("kvlib.td"),
        r#"<<< @(createDefaultKv)

makeKv data =
  @(fetch <= _ key = data.get(key),
    store <= _ key value = makeKv(data.set(key, value)))

createDefaultKv dummy =
  makeKv(hashMap())
"#,
    )
    .expect("write kvlib.td");

    // main.td — imports ONLY createDefaultKv (not makeKv)
    fs::write(
        dir.join("main.td"),
        r#">>> ./kvlib.td => @(createDefaultKv)

kv <= createDefaultKv(0)
kv2 <= kv.store("x", "hello")
result <= kv2.fetch("x")
result ]=> val
stdout(val)
"#,
    )
    .expect("write main.td");

    let main_path = dir.join("main.td");

    // Interpreter should succeed — makeKv is in createDefaultKv's enriched closure
    let interp = run_interpreter(&main_path)
        .expect("F-56: interpreter should succeed with enriched closure");
    assert_eq!(
        interp.trim(),
        "hello",
        "F-56: expected 'hello' from kv store/fetch, got '{}'",
        interp.trim()
    );

    let _ = fs::remove_dir_all(&dir);
}

/// F-58 regression: Native backend should dispatch BuchiPack field calls over built-in methods
/// when the receiver is known to be a BuchiPack.
#[test]
fn test_f58_native_pack_field_over_builtin_method() {
    let src = r#"
obj <= @(get <= _ key = "got:" + key, set <= _ key val = "set:" + key + "=" + val, keys <= _ = "mykeys", has <= _ key = true)
stdout(obj.get("x"))
stdout(obj.set("y", "42"))
stdout(obj.keys())
stdout(obj.has("z"))
"#;
    let interp =
        run_interpreter_src(src, "f58_pack_field").expect("F-58: interpreter should succeed");
    let native = run_native_src(src, "f58_pack_field").expect("F-58: native should succeed");
    assert_eq!(
        interp, native,
        "F-58: interp/native mismatch for BuchiPack field call\ninterp: {}\nnative: {}",
        interp, native
    );
}

/// F-59 regression: JS transpiler should emit `.hasValue()` (function call) instead of
/// `.hasValue` (property access) for Lax field access.
#[test]
fn test_f59_js_lax_has_value_property() {
    if !node_available() {
        return;
    }
    let src = r#"
x <= @[1, 2, 3]
lax <= x.get(0)
| lax.hasValue |>
  stdout("has value: true")
| _ |>
  stdout("has value: false")

empty <= x.get(99)
| empty.hasValue |>
  stdout("empty has value: true")
| _ |>
  stdout("empty has value: false")
"#;
    let interp =
        run_interpreter_src(src, "f59_has_value").expect("F-59: interpreter should succeed");
    let js = run_js_src(src, "f59_has_value").expect("F-59: JS should succeed");
    assert_eq!(
        interp, js,
        "F-59: interp/JS mismatch for Lax.hasValue\ninterp: {}\njs: {}",
        interp, js
    );
}

/// F-60 regression: Native BuchiPack with 4+ function fields should not segfault.
/// The 4th field call should work correctly.
#[test]
fn test_f60_native_pack_four_plus_function_fields() {
    let src = r#"
obj <= @(
  a <= _ = "first",
  b <= _ = "second",
  c <= _ = "third",
  d <= _ = "fourth",
  e <= _ = "fifth"
)
stdout(obj.a())
stdout(obj.b())
stdout(obj.c())
stdout(obj.d())
stdout(obj.e())
"#;
    let interp =
        run_interpreter_src(src, "f60_pack_fields").expect("F-60: interpreter should succeed");
    let native =
        run_native_src(src, "f60_pack_fields").expect("F-60: native should succeed (no segfault)");
    assert_eq!(
        interp, native,
        "F-60: interp/native mismatch for 4+ field BuchiPack\ninterp: {}\nnative: {}",
        interp, native
    );
}

#[test]
fn test_phase_c1_backend_parity() {
    let cases = [
        (
            "c1a_list_pack_multiref",
            r#"makePeople =
  @[
    @(name <= "Taro", age <= 25),
    @(name <= "Hana", age <= 30),
    @(name <= "Ken", age <= 18)
  ]
=> :@[@(name: Str, age: Int)]

people <= makePeople()
stdout(people.get(0).getOrDefault(@(name <= "", age <= 0)).name)
stdout(people.get(0).getOrDefault(@(name <= "", age <= 0)).age.toString())
stdout(people.get(1).getOrDefault(@(name <= "", age <= 0)).name)
stdout(people.get(2).getOrDefault(@(name <= "", age <= 0)).name)
stdout(people.get(2).getOrDefault(@(name <= "", age <= 0)).age.toString())
"#,
        ),
        (
            "c1b_hof_nested_capture",
            r#"threshold <= 2
prefix <= "v:"

data <= @[1, 2, 3, 4, 5]
filtered <= Filter[data, _ x = x > threshold]()
filtered ]=> fList
mapped <= Map[fList, _ x = prefix + x.toString()]()
mapped ]=> mList
stdout(mList)
"#,
        ),
        (
            "c1c_errceil_nested_capture",
            r#"Error => Inner = @(code: Int)
Error => Outer = @(code: Int)
tag <= "CTX"

innerOp x =
  |== innerErr: Error =
    tag + ":inner:" + innerErr.message
  => :Str
  | x < 0 |> Inner(type <= "Inner", message <= "neg", code <= 1).throw()
  | _ |> tag + ":ok:" + x.toString()
=> :Str

outerOp x =
  |== outerErr: Error =
    tag + ":outer:" + outerErr.message
  => :Str
  | x == 0 |> Outer(type <= "Outer", message <= "zero", code <= 2).throw()
  | _ |> innerOp(x)
=> :Str

stdout(outerOp(5))
stdout(outerOp(0))
stdout(outerOp(-1))
"#,
        ),
        (
            "c1d_cond_lambda_outer",
            r#"outer <= "OUT"

fmt x label = label + ":" + outer + ":" + x.toString() => :Str

classify x =
  mid <= "MID"
  | x > 10 |> fmt(x, mid)
  | x > 0 |> mid + ":" + x.toString()
  | _ |> outer + ":zero"
=> :Str

stdout(classify(20))
stdout(classify(5))
stdout(classify(0))
"#,
        ),
        // NOTE: c1e is a module test (directory structure) and is tested separately
        // in test_phase_c1e_module_backend_parity below.
        (
            "c1f_typedef_hashmap_lax",
            r#"Registry = @(
  data: HashMap[Str, Int]
  lookup key =
    lax <= data.get(key)
    | lax.hasValue() |> lax.getOrDefault(0).toString()
    | _ |> "not_found"
  => :Str
)

m <= hashMap().set("a", 10).set("b", 20)
reg <= Registry(data <= m)
stdout(reg.lookup("a"))
stdout(reg.lookup("b"))
stdout(reg.lookup("z"))
"#,
        ),
        (
            "c1g_errceil_map_throw",
            r#"Error => BadVal = @(code: Int)

safeDouble x =
  | x < 0 |> BadVal(type <= "BadVal", message <= "negative", code <= 1).throw()
  | _ |> x * 2
=> :Int

processOk items =
  |== err: Error =
    "caught:" + err.message
  => :Str
  Map[items, _ x = safeDouble(x)]() ]=> result
  Join[result, ","]() ]=> joined
  joined
=> :Str

processFail items =
  |== err: Error =
    "caught:" + err.message
  => :Str
  Map[items, _ x = safeDouble(x)]() ]=> result
  Join[result, ","]() ]=> joined
  joined
=> :Str

stdout(processOk(@[1, 2, 3]))
stdout(processFail(@[1, -1, 3]))
"#,
        ),
    ];

    for (label, source) in cases {
        assert_backend_parity_for_source(source, label);
    }
}

/// C-1e: BuchiPack field function calling recursive function in another module (F-56 + B-6d).
/// This is a module test requiring directory structure, so it cannot be inlined.
#[test]
fn test_phase_c1e_module_backend_parity() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("taida_c1e_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("main.td"),
        r#">>> ./helper => @(tools)

stdout(tools.run(3))
stdout(tools.run(1))
"#,
    )
    .expect("write main");
    fs::write(
        dir.join("helper.td"),
        r#"countdown n acc =
  | n == 0 |> acc
  | _ |> countdown(n - 1, acc + n.toString() + ",")
=> :Str

tools <= @(
  run <= _ n = countdown(n, "")
)

<<< @(tools)
"#,
    )
    .expect("write helper");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("c1e: interpreter should succeed");
    let native = run_native(&main_path).expect("c1e: native should succeed");
    assert_eq!(
        interp, native,
        "c1e: interpreter/native mismatch\ninterp: {}\nnative: {}",
        interp, native
    );

    if node_available() {
        let js = run_js(&main_path).expect("c1e: JS should succeed");
        assert_eq!(
            interp, js,
            "c1e: interpreter/JS mismatch\ninterp: {}\njs: {}",
            interp, js
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_phase_c2_backend_parity() {
    let cases = [
        (
            "c2a_func_5deep",
            r#"f1 a =
  f2 b =
    f3 c =
      f4 d =
        f5 e =
          a.toString() + ":" + b.toString() + ":" + c.toString() + ":" + d.toString() + ":" + e.toString()
        => :Str
        f5(5)
      => :Str
      f4(4)
    => :Str
    f3(3)
  => :Str
  f2(2)
=> :Str

stdout(f1(1))
"#,
        ),
        (
            "c2b_pack_5deep",
            r#"deep <= @(a <= @(b <= @(c <= @(d <= @(e <= 1)))))
stdout(deep.a.b.c.d.e.toString())
stdout(jsonEncode(deep))
"#,
        ),
        (
            "c2c_cond_5deep",
            r#"classify x =
  | x > 100 |>
    | x > 500 |>
      | x > 900 |>
        | x > 950 |>
          | x > 990 |> "S+"
          | _ |> "S"
        | _ |> "A+"
      | _ |> "A"
    | _ |> "B"
  | _ |> "C"
=> :Str

stdout(classify(995))
stdout(classify(960))
stdout(classify(910))
stdout(classify(200))
stdout(classify(50))
"#,
        ),
        (
            "c2d_errceil_3deep",
            r#"Error => E1 = @(code: Int)
Error => E2 = @(code: Int)

deepThrow x =
  |== outerErr: Error =
    "L3:" + outerErr.message
  => :Str
  |== midErr: Error =
    E2(type <= "E2", message <= "re:" + midErr.message, code <= 2).throw()
  => :Str
  | x < 0 |> E1(type <= "E1", message <= "orig", code <= 1).throw()
  | _ |> "ok:" + x.toString()
=> :Str

stdout(deepThrow(10))
stdout(deepThrow(-1))
"#,
        ),
        (
            "c2e_lambda_5deep",
            r#"a <= 1
f1 <= _ = _ = _ = _ = _ = a + a + a + a + a
stdout(f1()()()()().toString())
"#,
        ),
    ];

    for (label, source) in cases {
        assert_backend_parity_for_source(source, label);
    }
}

/// C-3: Regression tests for past blockers.
/// These are already covered by existing tests in parity.rs and native_compile.rs:
///   - C-3a (F-52): test_f52_module_closure_jsonencode_parity (parity.rs)
///   - C-3b (QF-26): test_qf26_imported_value_visible_in_main_function (native_compile.rs)
///   - C-3c (QF-27): test_qf27_library_init_resolves_imported_values_and_dependency_init (native_compile.rs)
///   - C-3d (QF-28): test_qf28_duplicate_export_names_across_modules_do_not_collide (native_compile.rs)
///   - C-3e (QF-29): test_qf29_same_stem_modules_do_not_collide (native_compile.rs)
///   - C-3f (F-57): test_f58_pack_field_call_over_builtin (parity.rs)
///   - C-3g (F-58, F-60): test_f58_pack_field_call_over_builtin, test_f60_pack_4plus_function_fields (parity.rs)
///   - C-3h (F-59): test_f59_js_lax_has_value_bool (parity.rs)
///
/// No additional test functions needed for C-3.

#[test]
fn test_phase_c4_backend_parity() {
    let cases = [
        (
            "c4a_hof_throw_variants",
            r#"Error => NegErr = @(code: Int)

guardMap x =
  | x < 0 |> NegErr(type <= "NegErr", message <= "neg", code <= 1).throw()
  | _ |> x * 10
=> :Int

guardFilter x =
  | x < 0 |> NegErr(type <= "NegErr", message <= "neg", code <= 1).throw()
  | _ |> true
=> :Bool

guardFold acc x =
  | x < 0 |> NegErr(type <= "NegErr", message <= "neg", code <= 1).throw()
  | _ |> acc + x
=> :Int

safeMap items =
  |== err: Error =
    "map:" + err.message
  => :Str
  Map[items, _ x = guardMap(x)]() ]=> r
  Join[r, ","]() ]=> v
  v
=> :Str

safeFilter items =
  |== err: Error =
    "filter:" + err.message
  => :Str
  Filter[items, _ x = guardFilter(x)]() ]=> r
  Join[r, ","]() ]=> v
  v
=> :Str

safeFold items =
  |== err: Error =
    "fold:" + err.message
  => :Str
  Fold[items, 0, _ acc x = guardFold(acc, x)]() ]=> v
  v.toString()
=> :Str

stdout(safeMap(@[1, -1, 3]))
stdout(safeFilter(@[1, -1, 3]))
stdout(safeFold(@[1, -1, 3]))
"#,
        ),
        (
            "c4b_hof_rethrow_2deep",
            r#"Error => E1 = @(code: Int)
Error => E2 = @(code: Int)

innerGuard x =
  |== innerErr: Error =
    E2(type <= "E2", message <= "re:" + innerErr.message, code <= 2).throw()
  => :Int
  | x < 0 |> E1(type <= "E1", message <= "neg", code <= 1).throw()
  | _ |> x * 10
=> :Int

test items =
  |== outerErr: Error =
    "outer:" + outerErr.message
  => :Str
  Map[items, _ x = innerGuard(x)]() ]=> r
  Join[r, ","]() ]=> v
  v
=> :Str

stdout(test(@[1, 2, 3]))
stdout(test(@[1, -1, 3]))
"#,
        ),
        (
            "c4c_mixed_5deep_capture",
            r#"doubleIt x = x * 2 => :Int

f1 a =
  outer <= 100
  f3 c =
    local <= 200
    f4 d =
      f5 e =
        doubleIt(a + outer).toString() + ":" + local.toString() + ":" + c.toString() + ":" + d.toString() + ":" + e.toString()
      => :Str
      f5(5)
    => :Str
    f4(4)
  => :Str
  f3(3)
=> :Str

stdout(f1(1))
"#,
        ),
        (
            "c4d_deep_nest_678",
            r#"f1 a =
  local <= 10
  f2 b =
    f3 c =
      deep <= 20
      f4 d =
        f5 e =
          f6 f =
            a.toString() + ":" + local.toString() + ":" + b.toString() + ":" + c.toString() + ":" + deep.toString() + ":" + d.toString() + ":" + e.toString() + ":" + f.toString()
          => :Str
          f6(6)
        => :Str
        f5(5)
      => :Str
      f4(4)
    => :Str
    f3(3)
  => :Str
  f2(2)
=> :Str

stdout(f1(1))
"#,
        ),
        (
            "c4e_errceil_4deep_var",
            r#"Error => E1 = @(code: Int)
Error => E2 = @(code: Int)
Error => E3 = @(code: Int)

test x tag =
  |== e4: Error =
    "L4:" + tag + ":" + e4.message
  => :Str
  |== e3: Error =
    E3(type <= "E3", message <= "r3:" + e3.message, code <= 3).throw()
  => :Str
  |== e2: Error =
    E2(type <= "E2", message <= "r2:" + e2.message, code <= 2).throw()
  => :Str
  | x < 0 |> E1(type <= "E1", message <= "orig", code <= 1).throw()
  | _ |> "ok:" + x.toString()
=> :Str

stdout(test(10, "T"))
stdout(test(-1, "T"))
"#,
        ),
        (
            "c4f_errceil_pack_rethrow",
            r#"Error => E1 = @(code: Int)
Error => E2 = @(code: Int)

buildMsg err =
  @(tag <= "mid", detail <= err) ]=> info
  info.tag + ":" + info.detail
=> :Str

test x =
  |== outerErr: Error =
    "outer:" + outerErr.message
  => :Str
  |== midErr: Error =
    E2(type <= "E2", message <= buildMsg(midErr.message), code <= 2).throw()
  => :Str
  | x < 0 |> E1(type <= "E1", message <= "neg", code <= 1).throw()
  | _ |> "ok:" + x.toString()
=> :Str

stdout(test(5))
stdout(test(-1))
"#,
        ),
    ];

    for (label, source) in cases {
        assert_backend_parity_for_source(source, label);
    }
}

#[test]
fn test_qf21_circular_import_rejected_all_backends() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("taida_qf21_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a => @(hello)
stdout(hello("test"))
"#,
    )
    .expect("write main");
    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b => @(world)
hello x = "Hello:" + world(x) => :Str
<<< @(hello)
"#,
    )
    .expect("write mod_a");
    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_a => @(hello)
world x = "World:" + x => :Str
<<< @(world)
"#,
    )
    .expect("write mod_b");

    let main_path = dir.join("main.td");
    let expected_path = dir
        .join("mod_a.td")
        .canonicalize()
        .expect("canonical mod_a");
    let expected_fragment = format!("Circular import detected: '{}'", expected_path.display());

    let interp_err = run_interpreter_error(&main_path).expect("interpreter should reject cycle");
    let native_err =
        run_native_build_error(&main_path, "qf21").expect("native build should reject cycle");
    let js_err = run_js_build_error(&main_path, "qf21").expect("js build should reject cycle");

    assert!(
        interp_err.contains(&expected_fragment),
        "unexpected interpreter error: {}",
        interp_err
    );
    assert!(
        native_err.contains(&expected_fragment),
        "unexpected native error: {}",
        native_err
    );
    assert!(
        js_err.contains(&expected_fragment),
        "unexpected js error: {}",
        js_err
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_qf24_js_build_writes_transitive_modules() {
    if !node_available() {
        return;
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("taida_qf24_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_b => @(fromB)
>>> ./mod_c => @(fromC)
stdout(fromB("x"))
stdout(fromC("y"))
"#,
    )
    .expect("write main");
    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_d => @(shared)
fromB x = "B:" + shared(x) => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");
    fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d => @(shared)
fromC x = "C:" + shared(x) => :Str
<<< @(fromC)
"#,
    )
    .expect("write mod_c");
    fs::write(
        dir.join("mod_d.td"),
        r#"shared x = "shared:" + x => :Str
<<< @(shared)
"#,
    )
    .expect("write mod_d");

    let out_dir = dir.join("out");
    fs::create_dir_all(&out_dir).expect("create out dir");
    let main_out = out_dir.join("main.mjs");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(dir.join("main.td"))
        .arg("-o")
        .arg(&main_out)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "js build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    assert!(main_out.exists(), "main output missing");
    assert!(out_dir.join("mod_b.mjs").exists(), "mod_b output missing");
    assert!(out_dir.join("mod_c.mjs").exists(), "mod_c output missing");
    assert!(out_dir.join("mod_d.mjs").exists(), "mod_d output missing");

    let interp = run_interpreter(&dir.join("main.td")).expect("interpreter should succeed");
    let js = Command::new("node")
        .arg(&main_out)
        .output()
        .expect("run node");
    assert!(
        js.status.success(),
        "node run failed: {}",
        String::from_utf8_lossy(&js.stderr)
    );
    let js_out = normalize(&String::from_utf8_lossy(&js.stdout));
    assert_eq!(interp, js_out);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_qf22_verify_detects_circular_imports() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("taida_qf22_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a => @(hello)
stdout(hello("test"))
"#,
    )
    .expect("write main");
    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b => @(world)
hello x = "Hello:" + world(x) => :Str
<<< @(hello)
"#,
    )
    .expect("write mod_a");
    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_a => @(hello)
world x = "World:" + x => :Str
<<< @(world)
"#,
    )
    .expect("write mod_b");

    let output = Command::new(taida_bin())
        .arg("verify")
        .arg("--check")
        .arg("no-circular-deps")
        .arg("--format")
        .arg("jsonl")
        .arg(dir.join("main.td"))
        .output()
        .expect("run verify");
    assert!(
        !output.status.success(),
        "verify unexpectedly passed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Circular dependency:"),
        "verify output missing circular dependency finding: {}",
        stdout
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_qf25_self_import_error_message_matches_across_backends() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("taida_qf25_{}_{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("main.td"),
        r#">>> ./main => @(greet)
stdout(greet())
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let expected_path = main_path.canonicalize().expect("canonical main");
    let expected_fragment = format!("Circular import detected: '{}'", expected_path.display());

    let interp_err =
        run_interpreter_error(&main_path).expect("interpreter should reject self import");
    let native_err =
        run_native_build_error(&main_path, "qf25").expect("native build should reject self import");
    let js_err =
        run_js_build_error(&main_path, "qf25").expect("js build should reject self import");

    assert!(interp_err.contains(&expected_fragment));
    assert!(native_err.contains(&expected_fragment));
    assert!(js_err.contains(&expected_fragment));

    let _ = fs::remove_dir_all(&dir);
}

// ── RC-1D-ii: Module Inline Quality Tests ──────────────────────────────
//
// RC-1m: Deep nested dependency (A -> B -> C -> D, 4 levels)
// RC-1n: Diamond dependency (A -> B, A -> C, B -> D, C -> D)
// RC-1o: Symbol collision (same-named internal functions across modules)
// RC-1p: Circular detection (direct, indirect, self-import)

/// RC-1m: Deep nested dependency — main -> A -> B -> C -> D (4 levels).
/// Each module has internal (non-exported) functions. Verifies that:
/// 1. Transitive imports are correctly resolved at all depths
/// 2. Internal functions are properly namespaced (no collision)
/// 3. All 3 backends produce identical output
#[test]
fn test_rc1m_deep_nested_dependency_four_levels() {
    let dir = unique_temp_path("taida_rc1m", "deep_nest", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_d.td"),
        r#"_internal_d x = "D(" + x + ")" => :Str
wrap_d x = _internal_d(x) => :Str
<<< @(wrap_d)
"#,
    )
    .expect("write mod_d");

    fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(wrap_d)
_internal_c x = "C(" + wrap_d(x) + ")" => :Str
wrap_c x = _internal_c(x) => :Str
<<< @(wrap_c)
"#,
    )
    .expect("write mod_c");

    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_c.td => @(wrap_c)
_internal_b x = "B(" + wrap_c(x) + ")" => :Str
wrap_b x = _internal_b(x) => :Str
<<< @(wrap_b)
"#,
    )
    .expect("write mod_b");

    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b.td => @(wrap_b)
wrap_a x = "A(" + wrap_b(x) + ")" => :Str
<<< @(wrap_a)
"#,
    )
    .expect("write mod_a");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a.td => @(wrap_a)
stdout(wrap_a("hello"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("RC-1m: interpreter should succeed");
    assert_eq!(
        interp, "A(B(C(D(hello))))",
        "RC-1m: unexpected interpreter output"
    );

    let native = run_native(&main_path).expect("RC-1m: native should succeed");
    assert_eq!(interp, native, "RC-1m: interpreter/native mismatch");

    if node_available() {
        let js = run_js_project(&main_path, "rc1m_deep").expect("RC-1m: js should succeed");
        assert_eq!(interp, js, "RC-1m: interpreter/js mismatch");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// RC-1m variant: deep nested with value passing (BuchiPack through module chain).
/// Verifies that structured data survives 4-level transitive import chain.
#[test]
fn test_rc1m_deep_nested_value_passing() {
    let dir = unique_temp_path("taida_rc1m", "value_pass", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_d.td"),
        r#"tag_d x = "D:" + x => :Str
<<< @(tag_d)
"#,
    )
    .expect("write mod_d");

    fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(tag_d)
tag_c x = "C:" + tag_d(x) => :Str
<<< @(tag_c)
"#,
    )
    .expect("write mod_c");

    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_c.td => @(tag_c)
tag_b x = "B:" + tag_c(x) => :Str
<<< @(tag_b)
"#,
    )
    .expect("write mod_b");

    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b.td => @(tag_b)
tag_a x = "A:" + tag_b(x) => :Str
<<< @(tag_a)
"#,
    )
    .expect("write mod_a");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a.td => @(tag_a)
stdout(tag_a("v1"))
stdout(tag_a("v2"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("RC-1m-val: interpreter should succeed");
    assert_eq!(
        interp, "A:B:C:D:v1\nA:B:C:D:v2",
        "RC-1m-val: unexpected interpreter output"
    );

    let native = run_native(&main_path).expect("RC-1m-val: native should succeed");
    assert_eq!(interp, native, "RC-1m-val: interpreter/native mismatch");

    if node_available() {
        let js = run_js_project(&main_path, "rc1m_val").expect("RC-1m-val: js should succeed");
        assert_eq!(interp, js, "RC-1m-val: interpreter/js mismatch");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// RC-1n: Diamond dependency — main imports B and C, both import shared module D.
/// Verifies that:
/// 1. Shared module D is resolved correctly (not duplicated)
/// 2. Internal functions in B, C, D don't collide
/// 3. Both paths through the diamond produce correct results
#[test]
fn test_rc1n_diamond_dependency_with_private_helpers() {
    let dir = unique_temp_path("taida_rc1n", "diamond", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_d.td"),
        r#"_secret x = "[" + x + "]" => :Str
shared x = "D:" + _secret(x) => :Str
<<< @(shared)
"#,
    )
    .expect("write mod_d");

    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_d.td => @(shared)
_b_helper x = shared(x) => :Str
fromB x = "B:" + _b_helper(x) => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");

    fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(shared)
_c_helper x = shared(x) => :Str
fromC x = "C:" + _c_helper(x) => :Str
<<< @(fromC)
"#,
    )
    .expect("write mod_c");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_b.td => @(fromB)
>>> ./mod_c.td => @(fromC)
stdout(fromB("x"))
stdout(fromC("y"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("RC-1n: interpreter should succeed");
    assert_eq!(
        interp, "B:D:[x]\nC:D:[y]",
        "RC-1n: unexpected interpreter output"
    );

    let native = run_native(&main_path).expect("RC-1n: native should succeed");
    assert_eq!(interp, native, "RC-1n: interpreter/native mismatch");

    if node_available() {
        let js = run_js_project(&main_path, "rc1n_diamond").expect("RC-1n: js should succeed");
        assert_eq!(interp, js, "RC-1n: interpreter/js mismatch");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// RC-1n variant: Diamond with global values in shared module.
/// D exports a value (not just functions). B and C both use it.
#[test]
fn test_rc1n_diamond_with_shared_value() {
    let dir = unique_temp_path("taida_rc1n", "diamond_val", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_d.td"),
        r#"prefix <= "shared"
get_prefix dummy = prefix => :Str
<<< @(get_prefix)
"#,
    )
    .expect("write mod_d");

    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_d.td => @(get_prefix)
fromB x = get_prefix(0) + ":B:" + x => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");

    fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(get_prefix)
fromC x = get_prefix(0) + ":C:" + x => :Str
<<< @(fromC)
"#,
    )
    .expect("write mod_c");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_b.td => @(fromB)
>>> ./mod_c.td => @(fromC)
stdout(fromB("1"))
stdout(fromC("2"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("RC-1n-val: interpreter should succeed");
    assert_eq!(
        interp, "shared:B:1\nshared:C:2",
        "RC-1n-val: unexpected interpreter output"
    );

    let native = run_native(&main_path).expect("RC-1n-val: native should succeed");
    assert_eq!(interp, native, "RC-1n-val: interpreter/native mismatch");

    if node_available() {
        let js = run_js_project(&main_path, "rc1n_val").expect("RC-1n-val: js should succeed");
        assert_eq!(interp, js, "RC-1n-val: interpreter/js mismatch");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// RCB-43: Diamond dependency with different symbols from shared module.
/// B imports funcX from D, C imports funcY from D (distinct symbol sets).
/// Exercises the diff-symbol code path in the WASM module inliner's IR cache.
/// Verifies correctness across all backends.
#[test]
fn test_rcb43_diamond_different_symbols() {
    let dir = unique_temp_path("taida_rcb43", "diamond_diff", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    // D exports two distinct functions
    fs::write(
        dir.join("mod_d.td"),
        r#"funcX x = "X:" + x => :Str
funcY y = "Y:" + y => :Str
<<< @(funcX, funcY)
"#,
    )
    .expect("write mod_d");

    // B imports only funcX from D
    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_d.td => @(funcX)
fromB x = "B(" + funcX(x) + ")" => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");

    // C imports only funcY from D (different symbol from B)
    fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(funcY)
fromC y = "C(" + funcY(y) + ")" => :Str
<<< @(fromC)
"#,
    )
    .expect("write mod_c");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_b.td => @(fromB)
>>> ./mod_c.td => @(fromC)
stdout(fromB("hello"))
stdout(fromC("world"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("RCB-43: interpreter should succeed");
    assert_eq!(
        interp, "B(X:hello)\nC(Y:world)",
        "RCB-43: unexpected interpreter output"
    );

    let native = run_native(&main_path).expect("RCB-43: native should succeed");
    assert_eq!(interp, native, "RCB-43: interpreter/native mismatch");

    if node_available() {
        let js = run_js_project(&main_path, "rcb43_diff").expect("RCB-43: js should succeed");
        assert_eq!(interp, js, "RCB-43: interpreter/js mismatch");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// RC-1o: Symbol collision — two modules with identically-named internal functions.
/// Verifies that module_key namespacing prevents _helper collision.
/// mod_a._helper does x * 2, mod_b._helper does x * 3.
/// Without namespacing, one _helper would overwrite the other.
#[test]
fn test_rc1o_symbol_collision_same_named_internals() {
    let dir = unique_temp_path("taida_rc1o", "collision", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_a.td"),
        r#"_helper x = x * 2 => :Int
compute x = _helper(x) + 1 => :Int
<<< @(compute)
"#,
    )
    .expect("write mod_a");

    fs::write(
        dir.join("mod_b.td"),
        r#"_helper x = x * 3 => :Int
compute x = _helper(x) + 2 => :Int
<<< @(compute)
"#,
    )
    .expect("write mod_b");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a.td => @(compute => computeA)
>>> ./mod_b.td => @(compute => computeB)
stdout(computeA(5).toString())
stdout(computeB(5).toString())
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("RC-1o: interpreter should succeed");
    // computeA(5) = _helper_a(5) + 1 = 10 + 1 = 11
    // computeB(5) = _helper_b(5) + 2 = 15 + 2 = 17
    assert_eq!(interp, "11\n17", "RC-1o: unexpected interpreter output");

    let native = run_native(&main_path).expect("RC-1o: native should succeed");
    assert_eq!(interp, native, "RC-1o: interpreter/native mismatch");

    if node_available() {
        let js = run_js_project(&main_path, "rc1o_collision").expect("RC-1o: js should succeed");
        assert_eq!(interp, js, "RC-1o: interpreter/js mismatch");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// RC-1o variant: three modules with same-named functions and same-named exports.
/// Tests that aliased imports correctly dispatch to different module_key namespaces.
#[test]
fn test_rc1o_triple_collision_with_aliases() {
    let dir = unique_temp_path("taida_rc1o", "triple", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_x.td"),
        r#"_process v = "X:" + v => :Str
run v = _process(v) => :Str
<<< @(run)
"#,
    )
    .expect("write mod_x");

    fs::write(
        dir.join("mod_y.td"),
        r#"_process v = "Y:" + v => :Str
run v = _process(v) => :Str
<<< @(run)
"#,
    )
    .expect("write mod_y");

    fs::write(
        dir.join("mod_z.td"),
        r#"_process v = "Z:" + v => :Str
run v = _process(v) => :Str
<<< @(run)
"#,
    )
    .expect("write mod_z");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_x.td => @(run => runX)
>>> ./mod_y.td => @(run => runY)
>>> ./mod_z.td => @(run => runZ)
stdout(runX("a"))
stdout(runY("b"))
stdout(runZ("c"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let interp = run_interpreter(&main_path).expect("RC-1o-triple: interpreter should succeed");
    assert_eq!(
        interp, "X:a\nY:b\nZ:c",
        "RC-1o-triple: unexpected interpreter output"
    );

    let native = run_native(&main_path).expect("RC-1o-triple: native should succeed");
    assert_eq!(interp, native, "RC-1o-triple: interpreter/native mismatch");

    if node_available() {
        let js =
            run_js_project(&main_path, "rc1o_triple").expect("RC-1o-triple: js should succeed");
        assert_eq!(interp, js, "RC-1o-triple: interpreter/js mismatch");
    }

    let _ = fs::remove_dir_all(&dir);
}

/// RC-1p: Direct circular dependency (A <-> B) is rejected by all backends.
#[test]
fn test_rc1p_direct_circular_rejected() {
    let dir = unique_temp_path("taida_rc1p", "direct", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b.td => @(fromB)
fromA x = "A:" + fromB(x) => :Str
<<< @(fromA)
"#,
    )
    .expect("write mod_a");

    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_a.td => @(fromA)
fromB x = "B:" + x => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a.td => @(fromA)
stdout(fromA("test"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let expected_path = dir
        .join("mod_a.td")
        .canonicalize()
        .expect("canonical mod_a");
    let expected_fragment = format!("Circular import detected: '{}'", expected_path.display());

    let interp_err =
        run_interpreter_error(&main_path).expect("RC-1p: interpreter should reject direct cycle");
    assert!(
        interp_err.contains(&expected_fragment),
        "RC-1p: interpreter error should contain circular message: {}",
        interp_err
    );

    let native_err = run_native_build_error(&main_path, "rc1p_direct")
        .expect("RC-1p: native should reject direct cycle");
    assert!(
        native_err.contains(&expected_fragment),
        "RC-1p: native error should contain circular message: {}",
        native_err
    );

    let js_err = run_js_build_error(&main_path, "rc1p_direct")
        .expect("RC-1p: js should reject direct cycle");
    assert!(
        js_err.contains(&expected_fragment),
        "RC-1p: js error should contain circular message: {}",
        js_err
    );

    let _ = fs::remove_dir_all(&dir);
}

/// RC-1p: Indirect circular dependency (A -> B -> C -> A) is rejected by all backends.
#[test]
fn test_rc1p_indirect_circular_three_node_cycle() {
    let dir = unique_temp_path("taida_rc1p", "indirect", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b.td => @(fromB)
fromA x = "A:" + fromB(x) => :Str
<<< @(fromA)
"#,
    )
    .expect("write mod_a");

    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_c.td => @(fromC)
fromB x = "B:" + fromC(x) => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");

    fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_a.td => @(fromA)
fromC x = "C:" + fromA(x) => :Str
<<< @(fromC)
"#,
    )
    .expect("write mod_c");

    fs::write(
        dir.join("main.td"),
        r#">>> ./mod_a.td => @(fromA)
stdout(fromA("test"))
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let expected_path = dir
        .join("mod_a.td")
        .canonicalize()
        .expect("canonical mod_a");
    let expected_fragment = format!("Circular import detected: '{}'", expected_path.display());

    let interp_err =
        run_interpreter_error(&main_path).expect("RC-1p: interpreter should reject indirect cycle");
    assert!(
        interp_err.contains(&expected_fragment),
        "RC-1p: interpreter error should contain circular message: {}",
        interp_err
    );

    let native_err = run_native_build_error(&main_path, "rc1p_indirect")
        .expect("RC-1p: native should reject indirect cycle");
    assert!(
        native_err.contains(&expected_fragment),
        "RC-1p: native error should contain circular message: {}",
        native_err
    );

    let js_err = run_js_build_error(&main_path, "rc1p_indirect")
        .expect("RC-1p: js should reject indirect cycle");
    assert!(
        js_err.contains(&expected_fragment),
        "RC-1p: js error should contain circular message: {}",
        js_err
    );

    let _ = fs::remove_dir_all(&dir);
}

/// RC-1p: Self-import (file imports itself) is rejected by all backends.
#[test]
fn test_rc1p_self_import_rejected() {
    let dir = unique_temp_path("taida_rc1p", "self", "dir");
    fs::create_dir_all(&dir).expect("create temp dir");

    fs::write(
        dir.join("main.td"),
        r#">>> ./main.td => @(something)
stdout(something())
"#,
    )
    .expect("write main");

    let main_path = dir.join("main.td");
    let expected_path = main_path.canonicalize().expect("canonical main");
    let expected_fragment = format!("Circular import detected: '{}'", expected_path.display());

    let interp_err =
        run_interpreter_error(&main_path).expect("RC-1p: interpreter should reject self import");
    assert!(
        interp_err.contains(&expected_fragment),
        "RC-1p: interpreter error should contain circular message: {}",
        interp_err
    );

    let native_err = run_native_build_error(&main_path, "rc1p_self")
        .expect("RC-1p: native should reject self import");
    assert!(
        native_err.contains(&expected_fragment),
        "RC-1p: native error should contain circular message: {}",
        native_err
    );

    let js_err =
        run_js_build_error(&main_path, "rc1p_self").expect("RC-1p: js should reject self import");
    assert!(
        js_err.contains(&expected_fragment),
        "RC-1p: js error should contain circular message: {}",
        js_err
    );

    let _ = fs::remove_dir_all(&dir);
}

// ── RC-6: Type Inheritance Soundness ─────────────────────────

#[test]
fn rc6a_error_inheritance_basic() {
    let source = r#"
Error => AppError = @(code: Int)
err <= AppError(type <= "AppError", message <= "test", code <= 42)
Str[err.code]() ]=> code_str
stdout(err.__type + " " + code_str)
"#;
    assert_backend_parity_for_source(source, "rc6a_error_basic");
}

#[test]
fn rc6a_error_field_composition() {
    let source = r#"
Error => DetailedError = @(detail: Str, severity: Int)
err <= DetailedError(type <= "DetailedError", message <= "fail", detail <= "db", severity <= 3)
Str[err.severity]() ]=> s
stdout(err.type + " " + err.message + " " + err.detail + " " + s)
"#;
    assert_backend_parity_for_source(source, "rc6a_field_composition");
}

#[test]
fn rc6b_error_multilevel() {
    let source = r#"
Error => AppError = @(app_code: Int)
AppError => ValidationError = @(field_name: Str)
ve <= ValidationError(type <= "VE", message <= "bad", app_code <= 400, field_name <= "email")
Str[ve.app_code]() ]=> ac
stdout(ve.__type + " " + ac + " " + ve.field_name)
"#;
    assert_backend_parity_for_source(source, "rc6b_multilevel");
}

#[test]
fn rc6c_error_parent_fields() {
    let source = r#"
Error => SimpleError = @(tag: Str)
err <= SimpleError(type <= "SimpleError", message <= "oops")
stdout(err.type + " " + err.message + " tag=" + err.tag)
"#;
    assert_backend_parity_for_source(source, "rc6c_parent_fields");
}

#[test]
fn rc6d_throw_catch_child_as_parent() {
    let source = r#"
Error => ChildError = @(reason: Str)
catch_fn x =
  |== error: Error =
    "caught(" + error.type + "): " + error.message
  => :Str
  ChildError(type <= "ChildError", message <= "child fail", reason <= "timeout").throw()
  "unreachable"
=> :Str
stdout(catch_fn(1))
"#;
    assert_backend_parity_for_source(source, "rc6d_throw_catch");
}

#[test]
fn rc6d_throw_multilevel_catch() {
    let source = r#"
Error => L1Error = @(l1: Str)
L1Error => L2Error = @(l2: Str)
catch_fn x =
  |== error: Error =
    "caught(" + error.type + "): " + error.l1 + "+" + error.l2
  => :Str
  L2Error(type <= "L2Error", message <= "deep", l1 <= "one", l2 <= "two").throw()
  "unreachable"
=> :Str
stdout(catch_fn(1))
"#;
    assert_backend_parity_for_source(source, "rc6d_multilevel_catch");
}

#[test]
fn rc6f_custom_inheritance_basic() {
    let source = r#"
Vehicle = @(name: Str, speed: Int)
Vehicle => Car = @(doors: Int)
car <= Car(name <= "Sedan", speed <= 120, doors <= 4)
Str[car.speed]() ]=> sp
Str[car.doors]() ]=> dr
stdout(car.__type + " " + car.name + " " + sp + " " + dr)
"#;
    assert_backend_parity_for_source(source, "rc6f_custom_basic");
}

#[test]
fn rc6g_custom_field_defaults() {
    let source = r#"
Vehicle = @(name: Str, speed: Int)
Vehicle => Bike = @(hasPedals: Bool)
bike <= Bike(name <= "BMX")
Str[bike.speed]() ]=> sp
Str[bike.hasPedals]() ]=> pd
stdout(bike.name + " " + sp + " " + pd)
"#;
    assert_backend_parity_for_source(source, "rc6g_defaults");
}

#[test]
fn rc6h_custom_multilevel() {
    let source = r#"
Shape = @(color: Str)
Shape => Polygon = @(sides: Int)
Polygon => Rectangle = @(width: Int, height: Int)
rect <= Rectangle(color <= "blue", sides <= 4, width <= 10, height <= 5)
Str[rect.sides]() ]=> s
Str[rect.width]() ]=> w
Str[rect.height]() ]=> h
stdout(rect.__type + " " + rect.color + " " + s + " " + w + " " + h)
"#;
    assert_backend_parity_for_source(source, "rc6h_multilevel");
}

#[test]
fn rc6i_child_as_parent_param() {
    let source = r#"
Vehicle = @(name: Str, speed: Int)
Vehicle => Car = @(doors: Int)
describe v: Vehicle = v.name + " at " + Str[v.speed]().unmold() => :Str
car <= Car(name <= "Toyota", speed <= 100, doors <= 4)
stdout(describe(car))
"#;
    assert_backend_parity_for_source(source, "rc6i_subtype_param");
}

#[test]
fn rc6k_multiple_children_same_parent() {
    let source = r#"
Animal = @(species: Str, legs: Int)
Animal => Dog = @(breed: Str)
Animal => Cat = @(indoor: Bool)
dog <= Dog(species <= "Canine", legs <= 4, breed <= "Shiba")
cat <= Cat(species <= "Feline", legs <= 4, indoor <= true)
Str[cat.indoor]() ]=> ci
stdout(dog.__type + "=" + dog.breed + " " + cat.__type + "=" + ci)
"#;
    assert_backend_parity_for_source(source, "rc6k_multi_children");
}
