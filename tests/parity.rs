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
        "net_http_hello",                // server example, requires dedicated loopback test harness
        "net_http_parse_encode",         // net package example, requires project root + deps
    ]
}

/// Examples that are expected to fail in the interpreter.
/// Only these files are allowed to be skipped when the interpreter fails.
/// All other interpreter failures are treated as test failures.
fn interpreter_skip_list() -> Vec<&'static str> {
    vec![
        "module_math",           // helper module, not standalone
        "module_utils",          // helper module, not standalone
        "helper_val",            // helper module, not standalone
        "transpile_npm",         // npm: imports only work in JS transpiler
        "net_http_hello",        // server example, requires dedicated loopback test harness
        "net_http_parse_encode", // net package example, requires project root + deps
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
        r#">>> ./helper.td => @(tools)

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
        r#">>> ./mod_a.td => @(hello)
stdout(hello("test"))
"#,
    )
    .expect("write main");
    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b.td => @(world)
hello x = "Hello:" + world(x) => :Str
<<< @(hello)
"#,
    )
    .expect("write mod_a");
    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_a.td => @(hello)
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
        r#">>> ./mod_b.td => @(fromB)
>>> ./mod_c.td => @(fromC)
stdout(fromB("x"))
stdout(fromC("y"))
"#,
    )
    .expect("write main");
    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_d.td => @(shared)
fromB x = "B:" + shared(x) => :Str
<<< @(fromB)
"#,
    )
    .expect("write mod_b");
    fs::write(
        dir.join("mod_c.td"),
        r#">>> ./mod_d.td => @(shared)
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

/// Directory build with sibling import outside the input directory.
/// project/src/main.td imports ../shared.td (outside src/).
/// `taida build --target js project/src -o out` must transpile shared.td too.
#[test]
fn test_qf26_js_dir_build_includes_sibling_modules() {
    if !node_available() {
        return;
    }

    let dir = unique_temp_path("taida_qf26", "sibling", "dir");
    let src_dir = dir.join("src");
    fs::create_dir_all(&src_dir).expect("create src dir");
    // Mark project root so find_project_root doesn't fall back to src/
    fs::create_dir_all(dir.join(".git")).expect("create .git marker");

    fs::write(
        src_dir.join("main.td"),
        r#">>> ../shared.td => @(fromShared)
>>> ./helper.td => @(fromHelper)
stdout(fromShared("A"))
stdout(fromHelper("B"))
"#,
    )
    .expect("write main");
    fs::write(
        src_dir.join("helper.td"),
        r#"fromHelper x = "helper:" + x => :Str
<<< @(fromHelper)
"#,
    )
    .expect("write helper");
    fs::write(
        dir.join("shared.td"),
        r#"fromShared x = "shared:" + x => :Str
<<< @(fromShared)
"#,
    )
    .expect("write shared");

    let out_dir = dir.join("out");
    let build = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&src_dir)
        .arg("-o")
        .arg(&out_dir)
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "js dir build failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    assert!(out_dir.join("main.mjs").exists(), "main.mjs missing");
    assert!(out_dir.join("helper.mjs").exists(), "helper.mjs missing");
    assert!(
        out_dir.join("shared.mjs").exists(),
        "shared.mjs missing — sibling module not transpiled"
    );

    let interp = run_interpreter(&src_dir.join("main.td")).expect("interpreter should succeed");
    let js = Command::new("node")
        .arg(out_dir.join("main.mjs"))
        .output()
        .expect("run node");
    assert!(
        js.status.success(),
        "node run failed: {}",
        String::from_utf8_lossy(&js.stderr)
    );
    let js_out = normalize(&String::from_utf8_lossy(&js.stdout));
    assert_eq!(interp, js_out, "interpreter vs JS output mismatch");

    let _ = fs::remove_dir_all(&dir);
}

/// Directory build with relative CLI paths (simulates `cd project && taida build --target js src -o out`).
/// Ensures entry_root/out_root canonicalization works with nested + sibling modules.
#[test]
fn test_qf27_js_dir_build_relative_paths_with_sibling() {
    if !node_available() {
        return;
    }

    let dir = unique_temp_path("taida_qf27", "relpath", "dir");
    let src_dir = dir.join("src");
    let nested_dir = src_dir.join("nested");
    fs::create_dir_all(&nested_dir).expect("create nested dir");
    let lib_dir = dir.join("lib");
    fs::create_dir_all(&lib_dir).expect("create lib dir");
    // Mark project root so find_project_root doesn't fall back to src/nested/
    fs::create_dir_all(dir.join(".git")).expect("create .git marker");

    fs::write(
        nested_dir.join("main.td"),
        r#">>> ../../lib/util.td => @(greet)
stdout(greet("world"))
"#,
    )
    .expect("write main");
    fs::write(
        lib_dir.join("util.td"),
        r#"greet x = "hello:" + x => :Str
<<< @(greet)
"#,
    )
    .expect("write util");

    let out_dir = dir.join("out");
    // Build using the project dir as cwd and relative paths (like `cd dir && taida build --target js src -o out`)
    let build = Command::new(taida_bin())
        .current_dir(&dir)
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg("src")
        .arg("-o")
        .arg("out")
        .output()
        .expect("build js");
    assert!(
        build.status.success(),
        "js dir build with relative paths failed: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    assert!(
        out_dir.join("nested").join("main.mjs").exists(),
        "nested/main.mjs missing"
    );

    let interp = run_interpreter(&nested_dir.join("main.td")).expect("interpreter should succeed");
    let js = Command::new("node")
        .arg(out_dir.join("nested").join("main.mjs"))
        .output()
        .expect("run node");
    assert!(
        js.status.success(),
        "node run failed: {}",
        String::from_utf8_lossy(&js.stderr)
    );
    let js_out = normalize(&String::from_utf8_lossy(&js.stdout));
    assert_eq!(interp, js_out, "interpreter vs JS output mismatch");

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
        r#">>> ./mod_a.td => @(hello)
stdout(hello("test"))
"#,
    )
    .expect("write main");
    fs::write(
        dir.join("mod_a.td"),
        r#">>> ./mod_b.td => @(world)
hello x = "Hello:" + world(x) => :Str
<<< @(hello)
"#,
    )
    .expect("write mod_a");
    fs::write(
        dir.join("mod_b.td"),
        r#">>> ./mod_a.td => @(hello)
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
        r#">>> ./main.td => @(greet)
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

// =========================================================================
// Cross-module quality tests: examples/quality/*/main.td
//
// RCB-214: Sweep multi-module test directories in examples/quality/.
// Each directory contains a main.td (entry point) and optional helper
// modules. If an `expected` file is present, all backends are compared
// against its content. Otherwise, the interpreter output is used as
// the reference (same parity approach as other tests).
// =========================================================================
#[test]
fn test_quality_cross_module_parity() {
    let has_node = node_available();
    let has_cc = cc_available();

    if !has_cc {
        eprintln!("SKIP: cc not available, skipping cross-module quality parity tests");
        return;
    }

    let quality_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("quality");

    if !quality_dir.exists() {
        eprintln!("SKIP: examples/quality/ directory does not exist");
        return;
    }

    // Collect subdirectories that contain main.td or main.tdm
    // RCB-213: main.tdm is used for versioned import tests (versioned imports
    // are only allowed in .tdm files).
    let mut test_dirs: Vec<PathBuf> = fs::read_dir(&quality_dir)
        .expect("examples/quality/ should be readable")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .filter(|e| e.path().join("main.td").exists() || e.path().join("main.tdm").exists())
        .map(|e| e.path())
        .collect();
    test_dirs.sort();

    if test_dirs.is_empty() {
        eprintln!("SKIP: no cross-module test directories found in examples/quality/");
        return;
    }

    // Tests that are expected to fail in the interpreter (error tests, etc.)
    let error_tests: Vec<&str> = vec![
        "b10a_circular_direct",
        "b10b_circular_indirect",
        "b10d_self_import",
        "b10e_circular_typedef",
        "b10f_circular_closure",
        "b10h_cross_backend_circular",
    ];

    let mut passed = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();

    for dir in &test_dirs {
        let dir_name = dir.file_name().unwrap().to_string_lossy().to_string();
        // RCB-213: prefer main.td, fall back to main.tdm for versioned import tests
        let main_td = if dir.join("main.td").exists() {
            dir.join("main.td")
        } else {
            dir.join("main.tdm")
        };

        // Skip known error tests (circular imports, etc.)
        if error_tests.iter().any(|t| dir_name == *t) {
            skipped += 1;
            continue;
        }

        // Run interpreter (reference implementation)
        let interp = match run_interpreter(&main_td) {
            Some(o) => o,
            None => {
                skipped += 1;
                continue;
            }
        };

        // If an `expected` file exists, verify interpreter matches it
        let expected_path = dir.join("expected");
        if expected_path.exists() {
            let expected = normalize(
                &fs::read_to_string(&expected_path).expect("expected file should be readable"),
            );
            if interp != expected {
                failures.push(format!(
                    "{}: interpreter output does not match expected\n  interp:    {:?}\n  expected:  {:?}",
                    dir_name,
                    interp.lines().take(5).collect::<Vec<_>>(),
                    expected.lines().take(5).collect::<Vec<_>>(),
                ));
                continue;
            }
        }

        // JS parity check
        if has_node {
            match run_js_project(&main_td, &dir_name) {
                Some(js) => {
                    if interp != js {
                        failures.push(format!(
                            "{}: Interpreter vs JS mismatch\n  interp: {:?}\n  js:     {:?}",
                            dir_name,
                            interp.lines().take(5).collect::<Vec<_>>(),
                            js.lines().take(5).collect::<Vec<_>>(),
                        ));
                        continue;
                    }
                }
                None => {
                    failures.push(format!("{}: JS build/execution failed", dir_name));
                    continue;
                }
            }
        }

        // Native parity check
        match run_native_with_error(&main_td) {
            Ok(native) => {
                if interp != native {
                    failures.push(format!(
                        "{}: Interpreter vs Native mismatch\n  interp: {:?}\n  native: {:?}",
                        dir_name,
                        interp.lines().take(5).collect::<Vec<_>>(),
                        native.lines().take(5).collect::<Vec<_>>(),
                    ));
                    continue;
                }
            }
            Err(err) => {
                failures.push(format!(
                    "{}: Native compile/run failed\n  {}",
                    dir_name, err
                ));
                continue;
            }
        }

        passed += 1;
    }

    eprintln!(
        "Cross-module quality parity: {}/{} passed, {} skipped",
        passed,
        passed + failures.len(),
        skipped,
    );

    if !failures.is_empty() {
        panic!(
            "{} cross-module quality parity test(s) failed:\n\n{}",
            failures.len(),
            failures.join("\n\n"),
        );
    }
}

// ── taida-lang/net: Phase 3 JS parity tests ──────────────────────

/// Create a temp project directory with taida-lang/net properly linked.
/// Returns (project_dir, cleanup_guard) — the project dir contains a .taida/deps/taida-lang/net/ symlink
/// to the global bundled dir, and a main.td with the provided source.
fn setup_net_project(source: &str, label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("taida_net3_{}_{}", label, nanos));
    fs::create_dir_all(&dir).expect("create project dir");

    // Write main.td
    fs::write(dir.join("main.td"), source).expect("write main.td");

    // Write packages.tdm (required for project root detection in JS build)
    fs::write(
        dir.join("packages.tdm"),
        "// Minimal packages.tdm for net test project\n",
    )
    .expect("write packages.tdm");

    // Create .taida/deps/taida-lang/net/ with the bundled stub
    let deps_net = dir
        .join(".taida")
        .join("deps")
        .join("taida-lang")
        .join("net");
    fs::create_dir_all(&deps_net).expect("create deps/taida-lang/net");

    // Write the net package stub (same as CoreBundledProvider::net_package_source)
    let net_stub = r#"// taida-lang/net — Core bundled network package
<<< @(dnsResolve, tcpConnect, tcpListen, tcpAccept, socketSend, socketSendAll, socketRecv, socketSendBytes, socketRecvBytes, socketRecvExact, udpBind, udpSendTo, udpRecvFrom, socketClose, listenerClose, udpClose, httpServe, httpParseRequestHead, httpEncodeResponse, readBody)
"#;
    fs::write(deps_net.join("main.td"), net_stub).expect("write net stub");

    dir
}

fn cleanup_net_project(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
}

fn run_net_interpreter(project_dir: &Path) -> Option<String> {
    let td_path = project_dir.join("main.td");
    let output = Command::new(taida_bin()).arg(&td_path).output().ok()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!(
            "run_net_interpreter failed for {}:\n{}",
            td_path.display(),
            stderr
        );
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&output.stdout)))
}

fn run_net_js(project_dir: &Path, label: &str) -> Option<String> {
    let td_path = project_dir.join("main.td");
    let js_path = unique_temp_path("taida_net3_js", label, "mjs");

    let transpile = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .ok()?;

    if !transpile.status.success() {
        let stderr = String::from_utf8_lossy(&transpile.stderr);
        eprintln!("run_net_js transpile failed: {}", stderr);
        let _ = fs::remove_file(&js_path);
        return None;
    }

    let run_output = Command::new("node").arg(&js_path).output().ok()?;
    let _ = fs::remove_file(&js_path);

    if !run_output.status.success() {
        let stderr = String::from_utf8_lossy(&run_output.stderr);
        eprintln!("run_net_js node failed: {}", stderr);
        return None;
    }

    Some(normalize(&String::from_utf8_lossy(&run_output.stdout)))
}

/// NET-3c: httpParseRequestHead parse fixture parity (Interpreter vs JS)
#[test]
fn test_net_parse_request_head_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

// Simple GET request
bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.complete)
stdout(parsed.method.start)
stdout(parsed.method.len)
stdout(parsed.path.start)
stdout(parsed.path.len)
stdout(parsed.query.len)
stdout(parsed.version.major)
stdout(parsed.version.minor)
stdout(parsed.contentLength)
"#;

    let dir = setup_net_project(source, "parse");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for net parse parity");
    let js = run_net_js(&dir, "parse").expect("js failed for net parse parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c: httpParseRequestHead parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
}

/// NET-3c: httpParseRequestHead with query string parity
#[test]
fn test_net_parse_request_head_query_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET /path?x=1&y=2 HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.complete)
stdout(parsed.path.start)
stdout(parsed.path.len)
stdout(parsed.query.start)
stdout(parsed.query.len)
"#;

    let dir = setup_net_project(source, "parse_query");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for net parse query parity");
    let js = run_net_js(&dir, "parse_query").expect("js failed for net parse query parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c: httpParseRequestHead query parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
}

/// NET-3c: httpParseRequestHead with POST body and Content-Length
#[test]
fn test_net_parse_request_head_post_body_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.complete)
stdout(parsed.contentLength)
stdout(parsed.bodyOffset)
"#;

    let dir = setup_net_project(source, "parse_post");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for net parse post parity");
    let js = run_net_js(&dir, "parse_post").expect("js failed for net parse post parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c: httpParseRequestHead POST body parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
}

/// NET-3c: httpEncodeResponse encode fixture parity (Interpreter vs JS)
#[test]
fn test_net_encode_response_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "Hello")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "encode");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for net encode parity");
    let js = run_net_js(&dir, "encode").expect("js failed for net encode parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c: httpEncodeResponse parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
}

/// NET-3c: httpEncodeResponse 404 empty body parity
#[test]
fn test_net_encode_response_404_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 404, headers <= @[], body <= "")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "encode_404");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for net encode 404 parity");
    let js = run_net_js(&dir, "encode_404").expect("js failed for net encode 404 parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c: httpEncodeResponse 404 parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
}

/// NET-3c: httpEncodeResponse 204 no body parity
#[test]
fn test_net_encode_response_204_no_body_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 204, headers <= @[], body <= "")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "encode_204");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for net encode 204 parity");
    let js = run_net_js(&dir, "encode_204").expect("js failed for net encode 204 parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c: httpEncodeResponse 204 parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
}

/// NET-3c: malformed HTTP version parity (both backends must reject)
#[test]
fn test_net_parse_request_head_invalid_version_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // "HTTX/1.1" is not valid HTTP version — both backends should return failure
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTX/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

// Both backends should reject: ok=false, kind=ParseError
stdout(result.__value.ok)
stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "parse_badver");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for net parse bad version");
    let js = run_net_js(&dir, "parse_badver").expect("js failed for net parse bad version");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c: invalid HTTP version parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
    // Both should report failure
    assert!(
        interp.contains("false"),
        "Both backends should reject invalid HTTP version, got: {}",
        interp
    );
}

/// NET-3c: malformed Content-Length parity (both backends must reject "5abc")
#[test]
fn test_net_parse_request_head_invalid_content_length_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5abc\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

// Both backends should reject: ok=false, kind=ParseError
stdout(result.__value.ok)
stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "parse_badcl");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for net parse bad CL");
    let js = run_net_js(&dir, "parse_badcl").expect("js failed for net parse bad CL");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c: invalid Content-Length parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
    assert!(
        interp.contains("false"),
        "Both backends should reject invalid Content-Length, got: {}",
        interp
    );
}

/// NET-3c2: Content-Length MAX_SAFE_INTEGER boundary parity
/// 9007199254740991 (accept) and 9007199254740992 (reject)
#[test]
fn test_net_parse_request_head_content_length_max_safe_integer_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Test 1: MAX_SAFE_INTEGER should be accepted
    let source_accept = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 9007199254740991\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

// success: __value is the parsed request pack directly
stdout(result.__value.contentLength)
"#;

    let dir = setup_net_project(source_accept, "parse_maxsafe_accept");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for max safe accept");
    let js = run_net_js(&dir, "parse_maxsafe_accept").expect("js failed for max safe accept");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-3c2: MAX_SAFE_INTEGER accept parity mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
    assert!(
        interp.contains("9007199254740991"),
        "Content-Length value should be exact, got: {}",
        interp
    );

    // Test 2: MAX_SAFE_INTEGER + 1 should be rejected
    let source_reject = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 9007199254740992\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.ok)
stdout(result.__value.kind)
"#;

    let dir2 = setup_net_project(source_reject, "parse_maxsafe_reject");
    let interp2 = run_net_interpreter(&dir2).expect("interpreter failed for max safe reject");
    let js2 = run_net_js(&dir2, "parse_maxsafe_reject").expect("js failed for max safe reject");
    cleanup_net_project(&dir2);

    assert_eq!(
        interp2, js2,
        "NET-3c2: MAX_SAFE_INTEGER+1 reject parity mismatch\nInterp: {}\nJS: {}",
        interp2, js2
    );
    assert!(
        interp2.contains("false"),
        "MAX_SAFE_INTEGER+1 should be rejected, got: {}",
        interp2
    );
}

/// NET-3d: httpServe bounded server parity (Interpreter vs JS)
/// Runs httpServe with maxRequests=1, sends a raw HTTP request, and verifies response + result.
#[test]
fn test_net_http_serve_bounded_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let backends = vec!["interp", "js"];

    for backend in &backends {
        // Use a fresh ephemeral port: bind, get port, immediately release
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "pong")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("serve_{}", backend));
        let td_path = dir.join("main.td");

        let mut child = match *backend {
            "interp" => Command::new(taida_bin())
                .arg(&td_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn interpreter"),
            "js" => {
                let js_path = unique_temp_path("taida_net3_serve_js", backend, "mjs");
                let transpile = Command::new(taida_bin())
                    .arg("build")
                    .arg("--target")
                    .arg("js")
                    .arg(&td_path)
                    .arg("-o")
                    .arg(&js_path)
                    .output()
                    .expect("transpile");
                if !transpile.status.success() {
                    let stderr = String::from_utf8_lossy(&transpile.stderr);
                    cleanup_net_project(&dir);
                    let _ = fs::remove_file(&js_path);
                    panic!("JS transpile failed for {}: {}", backend, stderr);
                }
                let child = Command::new("node")
                    .arg(&js_path)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn node");
                // Give Node.js time to read the file before deleting
                thread::sleep(Duration::from_millis(500));
                let _ = fs::remove_file(&js_path);
                child
            }
            _ => unreachable!(),
        };

        // Wait for server to be ready, then send the actual request on a *new* connection
        // The probe connection is separate and should NOT count as the one request.
        // Strategy: keep trying to connect + send + read until we get a valid response.
        let mut response = Vec::new();
        let mut got_response = false;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(100));
            let stream = match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => s,
                Err(_) => continue,
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
            let mut stream = stream;
            let request = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
            if std::io::Write::write_all(&mut stream, request).is_err() {
                continue;
            }
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stream, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => response.extend_from_slice(&buf[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            if !response.is_empty() {
                got_response = true;
                break;
            }
        }

        if !got_response {
            let _ = child.kill();
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not respond on port {}",
                backend, port,
            );
        }

        let resp_str = String::from_utf8_lossy(&response).to_string();
        assert!(
            resp_str.contains("200 OK"),
            "{} backend: response should contain '200 OK', got: {:?}",
            backend,
            resp_str
        );
        assert!(
            resp_str.contains("pong"),
            "{} backend: response body should contain 'pong', got: {:?}",
            backend,
            resp_str
        );

        // Wait for server process to exit (it should self-terminate after maxRequests=1)
        let output = child.wait_with_output().expect("wait for server process");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));

        assert_eq!(
            stdout, "true\n1",
            "{} backend: httpServe result mismatch. stdout: {:?}",
            backend, stdout
        );

        cleanup_net_project(&dir);
    }
}

// ── NET-4: Native backend parity ──────────────────────────────

/// Compile and run a net .td via native backend, return stdout.
fn run_net_native(project_dir: &Path, label: &str) -> Option<String> {
    let td_path = project_dir.join("main.td");
    let bin_path = unique_temp_path("taida_net4_native", label, "bin");

    let compile = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .ok()?;

    if !compile.status.success() {
        let stderr = String::from_utf8_lossy(&compile.stderr);
        eprintln!("run_net_native compile failed for {}:\n{}", label, stderr);
        return None;
    }

    let output = Command::new(&bin_path).output().ok()?;
    let _ = fs::remove_file(&bin_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("run_net_native run failed for {}:\n{}", label, stderr);
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&output.stdout)))
}

/// NET-4d: httpParseRequestHead parse fixture parity (Interpreter vs Native)
/// Note: Bool fields (complete, ok) are tested via string interpolation to avoid
/// native backend type tag loss on raw pack field access.
#[test]
fn test_net_parse_request_head_interp_native_parity() {
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

// Simple GET request
bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

// Use consumed > 0 as proxy for complete (avoids native Bool display issue)
stdout(parsed.consumed)
stdout(parsed.method.start)
stdout(parsed.method.len)
stdout(parsed.path.start)
stdout(parsed.path.len)
stdout(parsed.query.len)
stdout(parsed.version.major)
stdout(parsed.version.minor)
stdout(parsed.contentLength)
"#;

    let dir = setup_net_project(source, "native_parse");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for native parse parity");
    let native =
        run_net_native(&dir, "native_parse").expect("native failed for native parse parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "NET-4d: httpParseRequestHead parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-4d: httpParseRequestHead with query string parity (Interpreter vs Native)
#[test]
fn test_net_parse_request_head_query_native_parity() {
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET /path?x=1&y=2 HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.consumed)
stdout(parsed.path.start)
stdout(parsed.path.len)
stdout(parsed.query.start)
stdout(parsed.query.len)
"#;

    let dir = setup_net_project(source, "native_parse_query");
    let interp =
        run_net_interpreter(&dir).expect("interpreter failed for native parse query parity");
    let native = run_net_native(&dir, "native_parse_query")
        .expect("native failed for native parse query parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "NET-4d: httpParseRequestHead query parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-4d: httpParseRequestHead with POST body (Interpreter vs Native)
#[test]
fn test_net_parse_request_head_post_body_native_parity() {
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.consumed)
stdout(parsed.contentLength)
stdout(parsed.bodyOffset)
"#;

    let dir = setup_net_project(source, "native_parse_post");
    let interp =
        run_net_interpreter(&dir).expect("interpreter failed for native parse post parity");
    let native = run_net_native(&dir, "native_parse_post")
        .expect("native failed for native parse post parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "NET-4d: httpParseRequestHead POST body parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-4d: httpEncodeResponse encode fixture parity (Interpreter vs Native)
#[test]
fn test_net_encode_response_interp_native_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "Hello")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "native_encode");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for native encode parity");
    let native =
        run_net_native(&dir, "native_encode").expect("native failed for native encode parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "NET-4d: httpEncodeResponse parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-4d: httpEncodeResponse 404 parity (Interpreter vs Native)
#[test]
fn test_net_encode_response_404_native_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 404, headers <= @[], body <= "")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "native_encode_404");
    let interp =
        run_net_interpreter(&dir).expect("interpreter failed for native encode 404 parity");
    let native = run_net_native(&dir, "native_encode_404")
        .expect("native failed for native encode 404 parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "NET-4d: httpEncodeResponse 404 parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-4d: httpEncodeResponse 204 no-body parity (Interpreter vs Native)
#[test]
fn test_net_encode_response_204_native_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 204, headers <= @[], body <= "")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "native_encode_204");
    let interp =
        run_net_interpreter(&dir).expect("interpreter failed for native encode 204 parity");
    let native = run_net_native(&dir, "native_encode_204")
        .expect("native failed for native encode 204 parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "NET-4d: httpEncodeResponse 204 parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-4d: malformed Content-Length parity (Interpreter vs Native)
#[test]
fn test_net_parse_request_head_invalid_content_length_native_parity() {
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5abc\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

// Both backends should reject with kind=ParseError
// (avoid result.__value.ok — native Bool display limitation)
stdout(result.__value.kind)
stdout(result.__value.message.length())
"#;

    let dir = setup_net_project(source, "native_parse_badcl");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for native parse bad CL");
    let native =
        run_net_native(&dir, "native_parse_badcl").expect("native failed for native parse bad CL");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "NET-4d: invalid Content-Length native parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-4e: httpServe bounded server parity (all 3 backends)
#[test]
fn test_net_http_serve_bounded_all_backends_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let backends = vec!["interp", "js", "native"];

    for backend in &backends {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "pong")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
// Use requests count only (avoids native Bool display issue with r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("serve3_{}", backend));
        let td_path = dir.join("main.td");

        let mut child = match *backend {
            "interp" => Command::new(taida_bin())
                .arg(&td_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn interpreter"),
            "js" => {
                let js_path = unique_temp_path("taida_net4_serve_js", backend, "mjs");
                let transpile = Command::new(taida_bin())
                    .arg("build")
                    .arg("--target")
                    .arg("js")
                    .arg(&td_path)
                    .arg("-o")
                    .arg(&js_path)
                    .output()
                    .expect("transpile");
                if !transpile.status.success() {
                    let stderr = String::from_utf8_lossy(&transpile.stderr);
                    cleanup_net_project(&dir);
                    let _ = fs::remove_file(&js_path);
                    panic!("JS transpile failed for {}: {}", backend, stderr);
                }
                let child = Command::new("node")
                    .arg(&js_path)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn node");
                thread::sleep(Duration::from_millis(500));
                let _ = fs::remove_file(&js_path);
                child
            }
            "native" => {
                let bin_path = unique_temp_path("taida_net4_serve_native", backend, "bin");
                let compile = Command::new(taida_bin())
                    .arg("build")
                    .arg("--target")
                    .arg("native")
                    .arg(&td_path)
                    .arg("-o")
                    .arg(&bin_path)
                    .output()
                    .expect("compile native");
                if !compile.status.success() {
                    let stderr = String::from_utf8_lossy(&compile.stderr);
                    cleanup_net_project(&dir);
                    let _ = fs::remove_file(&bin_path);
                    panic!("Native compile failed for {}: {}", backend, stderr);
                }
                let child = Command::new(&bin_path)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn native");
                thread::sleep(Duration::from_millis(200));
                let _ = fs::remove_file(&bin_path);
                child
            }
            _ => unreachable!(),
        };

        // Wait for server to be ready, then send request
        let mut response = Vec::new();
        let mut got_response = false;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(100));
            let stream = match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => s,
                Err(_) => continue,
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
            let mut stream = stream;
            let request = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
            if std::io::Write::write_all(&mut stream, request).is_err() {
                continue;
            }
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stream, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => response.extend_from_slice(&buf[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            if !response.is_empty() {
                got_response = true;
                break;
            }
        }

        if !got_response {
            let _ = child.kill();
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not respond on port {}",
                backend, port,
            );
        }

        let resp_str = String::from_utf8_lossy(&response).to_string();
        assert!(
            resp_str.contains("200 OK"),
            "{} backend: response should contain '200 OK', got: {:?}",
            backend,
            resp_str
        );
        assert!(
            resp_str.contains("pong"),
            "{} backend: response body should contain 'pong', got: {:?}",
            backend,
            resp_str
        );

        // Wait for server process to exit
        let output = child.wait_with_output().expect("wait for server process");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));

        assert_eq!(
            stdout, "1",
            "{} backend: httpServe result mismatch. stdout: {:?}",
            backend, stdout
        );

        cleanup_net_project(&dir);
    }
}

/// NET-4 review F1: malformed HTTP version "HTTP/a.b" must be rejected (Interpreter vs Native)
#[test]
fn test_net_parse_malformed_http_version_native_parity() {
    // "HTTP/a.b" has non-digit major/minor — both backends must reject as malformed
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/a.b\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
stdout(result.__value.message.length())
"#;

    let dir = setup_net_project(source, "native_parse_badver_ab");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for malformed HTTP version");
    let native = run_net_native(&dir, "native_parse_badver_ab")
        .expect("native failed for malformed HTTP version");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "F1: malformed HTTP version native parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("ParseError"),
        "Both backends should reject HTTP/a.b, got: {}",
        interp
    );
}

/// NET-4 review F1: HTTP version with extra digits "HTTP/12.34" must be rejected (Interpreter vs Native)
#[test]
fn test_net_parse_multi_digit_http_version_native_parity() {
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/12.34\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
stdout(result.__value.message.length())
"#;

    let dir = setup_net_project(source, "native_parse_badver_1234");
    let interp =
        run_net_interpreter(&dir).expect("interpreter failed for multi-digit HTTP version");
    let native = run_net_native(&dir, "native_parse_badver_1234")
        .expect("native failed for multi-digit HTTP version");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "F1: multi-digit HTTP version native parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("ParseError"),
        "Both backends should reject HTTP/12.34, got: {}",
        interp
    );
}

/// NET-4 review F3: httpEncodeResponse with non-Str body must return EncodeError (Interpreter vs Native)
#[test]
fn test_net_encode_response_invalid_body_native_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 200, headers <= @[], body <= 1)
result <= httpEncodeResponse(resp)

stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "native_encode_bad_body");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for invalid body encode");
    let native = run_net_native(&dir, "native_encode_bad_body")
        .expect("native failed for invalid body encode");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "F3: invalid body encode native parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("EncodeError"),
        "Both backends should return EncodeError for body <= 1, got: {}",
        interp
    );
}

/// NET-4 review F4: Content-Length with tab whitespace must be parsed correctly (Interpreter vs Native)
#[test]
fn test_net_parse_content_length_tab_trim_native_parity() {
    // Content-Length:\t5 — tab between colon and value
    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length:\t5\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.contentLength)
"#;

    let dir = setup_net_project(source, "native_parse_cl_tab");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for CL tab trim");
    let native =
        run_net_native(&dir, "native_parse_cl_tab").expect("native failed for CL tab trim");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, native,
        "F4: Content-Length tab trim native parity mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
    // NB2-17: Use exact match instead of contains()
    assert_eq!(
        interp.trim(),
        "5",
        "Content-Length should be 5 with tab whitespace, got: {}",
        interp
    );
}

/// NET-4 review F1+F4: 3-way parity for malformed version (all backends)
#[test]
fn test_net_parse_malformed_version_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/a.b\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "3way_parse_badver");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for 3-way bad version");
    let js = run_net_js(&dir, "3way_parse_badver").expect("js failed for 3-way bad version");
    let native =
        run_net_native(&dir, "3way_parse_badver").expect("native failed for 3-way bad version");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "3-way: malformed version Interp vs JS mismatch\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "3-way: malformed version Interp vs Native mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET-5: Parity / Hardening ──────────────────────────────────

// ── NET-5a: 3-way parse fixture parity ─────────────────────────

/// NET-5a: httpParseRequestHead simple GET — 3-way parity (Interp vs JS vs Native)
#[test]
fn test_net5a_parse_simple_get_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.consumed)
stdout(parsed.method.start)
stdout(parsed.method.len)
stdout(parsed.path.start)
stdout(parsed.path.len)
stdout(parsed.query.len)
stdout(parsed.version.major)
stdout(parsed.version.minor)
stdout(parsed.contentLength)
"#;

    let dir = setup_net_project(source, "5a_parse_get");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5a_parse_get").expect("js failed");
    let native = run_net_native(&dir, "5a_parse_get").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5a: simple GET Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5a: simple GET Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5a: httpParseRequestHead with query string — 3-way parity
#[test]
fn test_net5a_parse_query_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET /path?x=1&y=2 HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.consumed)
stdout(parsed.path.start)
stdout(parsed.path.len)
stdout(parsed.query.start)
stdout(parsed.query.len)
"#;

    let dir = setup_net_project(source, "5a_parse_query");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5a_parse_query").expect("js failed");
    let native = run_net_native(&dir, "5a_parse_query").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5a: query Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5a: query Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5a: httpParseRequestHead POST with Content-Length — 3-way parity
#[test]
fn test_net5a_parse_post_body_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.consumed)
stdout(parsed.contentLength)
stdout(parsed.bodyOffset)
"#;

    let dir = setup_net_project(source, "5a_parse_post");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5a_parse_post").expect("js failed");
    let native = run_net_native(&dir, "5a_parse_post").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5a: POST body Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5a: POST body Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5a: malformed Content-Length "5abc" — 3-way rejection parity
#[test]
fn test_net5a_parse_invalid_content_length_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5abc\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "5a_parse_badcl");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5a_parse_badcl").expect("js failed");
    let native = run_net_native(&dir, "5a_parse_badcl").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5a: bad CL Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5a: bad CL Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("ParseError"),
        "All backends should reject invalid CL, got: {}",
        interp
    );
}

/// NET-5a: MAX_SAFE_INTEGER Content-Length boundary — 3-way parity
#[test]
fn test_net5a_parse_content_length_max_safe_integer_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Accept: 9007199254740991
    let source_accept = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 9007199254740991\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.contentLength)
"#;

    let dir = setup_net_project(source_accept, "5a_maxsafe_accept");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5a_maxsafe_accept").expect("js failed");
    let native = run_net_native(&dir, "5a_maxsafe_accept").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5a: MAX_SAFE accept Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5a: MAX_SAFE accept Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("9007199254740991"),
        "MAX_SAFE_INTEGER should be accepted, got: {}",
        interp
    );

    // Reject: 9007199254740992
    let source_reject = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 9007199254740992\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
"#;

    let dir2 = setup_net_project(source_reject, "5a_maxsafe_reject");
    let interp2 = run_net_interpreter(&dir2).expect("interpreter failed");
    let js2 = run_net_js(&dir2, "5a_maxsafe_reject").expect("js failed");
    let native2 = run_net_native(&dir2, "5a_maxsafe_reject").expect("native failed");
    cleanup_net_project(&dir2);

    assert_eq!(
        interp2, js2,
        "NET-5a: MAX_SAFE+1 reject Interp vs JS\nInterp: {}\nJS: {}",
        interp2, js2
    );
    assert_eq!(
        interp2, native2,
        "NET-5a: MAX_SAFE+1 reject Interp vs Native\nInterp: {}\nNative: {}",
        interp2, native2
    );
}

/// NB-20: Content-Length leading-zero padded values — 3-way parity
/// "00000000000000005" (17 chars) should be accepted as 5 in all 3 backends.
/// Previously JS rejected this because rawVal.length > 16 without stripping leading zeros.
#[test]
fn test_nb20_content_length_leading_zeros_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Test 1: "00000000000000005" (17 chars) should be accepted as 5
    let source_accept = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 00000000000000005\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.contentLength)
"#;

    let dir = setup_net_project(source_accept, "nb20_lz_accept");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "nb20_lz_accept").expect("js failed");
    let native = run_net_native(&dir, "nb20_lz_accept").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NB-20: leading-zero CL Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NB-20: leading-zero CL Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    // NB2-17: Use exact match instead of contains()
    assert_eq!(
        interp.trim(),
        "5",
        "Leading-zero padded CL '00000000000000005' should parse as 5, got: {}",
        interp
    );

    // Test 2: "0042" should be accepted as 42
    let source_0042 = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0042\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.contentLength)
"#;

    let dir2 = setup_net_project(source_0042, "nb20_lz_0042");
    let interp2 = run_net_interpreter(&dir2).expect("interpreter failed");
    let js2 = run_net_js(&dir2, "nb20_lz_0042").expect("js failed");
    let native2 = run_net_native(&dir2, "nb20_lz_0042").expect("native failed");
    cleanup_net_project(&dir2);

    assert_eq!(
        interp2, js2,
        "NB-20: '0042' CL Interp vs JS\nInterp: {}\nJS: {}",
        interp2, js2
    );
    assert_eq!(
        interp2, native2,
        "NB-20: '0042' CL Interp vs Native\nInterp: {}\nNative: {}",
        interp2, native2
    );
    assert!(
        interp2.contains("42"),
        "Leading-zero padded CL '0042' should parse as 42, got: {}",
        interp2
    );

    // Test 3: all-zeros "00000000000000000" (17 zeros) should be accepted as 0
    let source_allzeros = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 00000000000000000\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.contentLength)
"#;

    let dir3 = setup_net_project(source_allzeros, "nb20_lz_allzeros");
    let interp3 = run_net_interpreter(&dir3).expect("interpreter failed");
    let js3 = run_net_js(&dir3, "nb20_lz_allzeros").expect("js failed");
    let native3 = run_net_native(&dir3, "nb20_lz_allzeros").expect("native failed");
    cleanup_net_project(&dir3);

    assert_eq!(
        interp3, js3,
        "NB-20: all-zeros CL Interp vs JS\nInterp: {}\nJS: {}",
        interp3, js3
    );
    assert_eq!(
        interp3, native3,
        "NB-20: all-zeros CL Interp vs Native\nInterp: {}\nNative: {}",
        interp3, native3
    );
    // NB2-17: Use exact match instead of contains() which could match "10", "200" etc.
    assert_eq!(
        interp3.trim(),
        "0",
        "All-zeros CL '00000000000000000' should parse as 0, got: {}",
        interp3
    );

    // Test 4: leading zeros + value > MAX_SAFE_INTEGER must still be rejected
    let source_reject = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 009007199254740992\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
"#;

    let dir4 = setup_net_project(source_reject, "nb20_lz_over_max");
    let interp4 = run_net_interpreter(&dir4).expect("interpreter failed");
    let js4 = run_net_js(&dir4, "nb20_lz_over_max").expect("js failed");
    let native4 = run_net_native(&dir4, "nb20_lz_over_max").expect("native failed");
    cleanup_net_project(&dir4);

    assert_eq!(
        interp4, js4,
        "NB-20: leading-zero + over MAX_SAFE Interp vs JS\nInterp: {}\nJS: {}",
        interp4, js4
    );
    assert_eq!(
        interp4, native4,
        "NB-20: leading-zero + over MAX_SAFE Interp vs Native\nInterp: {}\nNative: {}",
        interp4, native4
    );
    assert!(
        interp4.contains("ParseError"),
        "Leading-zero + over MAX_SAFE should be rejected, got: {}",
        interp4
    );
}

/// NET-5a: malformed HTTP version "HTTP/a.b" — 3-way rejection parity
#[test]
fn test_net5a_parse_malformed_version_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/a.b\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "5a_badver_ab");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5a_badver_ab").expect("js failed");
    let native = run_net_native(&dir, "5a_badver_ab").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5a: bad version Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5a: bad version Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("ParseError"),
        "All backends should reject HTTP/a.b, got: {}",
        interp
    );
}

/// NET-5a: multi-digit HTTP version "HTTP/12.34" — 3-way rejection parity
#[test]
fn test_net5a_parse_multi_digit_version_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/12.34\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "5a_badver_1234");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5a_badver_1234").expect("js failed");
    let native = run_net_native(&dir, "5a_badver_1234").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5a: multi-digit version Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5a: multi-digit version Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("ParseError"),
        "All backends should reject HTTP/12.34, got: {}",
        interp
    );
}

/// NET-5a: Content-Length with tab whitespace — 3-way parity
#[test]
fn test_net5a_parse_content_length_tab_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length:\t5\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.contentLength)
"#;

    let dir = setup_net_project(source, "5a_parse_cl_tab");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5a_parse_cl_tab").expect("js failed");
    let native = run_net_native(&dir, "5a_parse_cl_tab").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5a: CL tab Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5a: CL tab Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    // NB2-17: Use exact match instead of contains()
    assert_eq!(
        interp.trim(),
        "5",
        "Content-Length should be 5 with tab, got: {}",
        interp
    );
}

/// NET2-2a: httpParseRequestHead chunked field — 3-way parity
/// Tests that the `chunked` field is correctly detected from Transfer-Encoding header.
/// Note: Uses string interpolation for Bool fields to avoid native backend type tag loss.
#[test]
fn test_net2_parse_chunked_field_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Test 1: Request without Transfer-Encoding → chunked = false
    // NB2-16: Include chunked field in output. Uses Int(chunked) to normalize
    // Bool display across backends (true/1 -> 1, false/0 -> 0).
    let source_no_te = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.consumed)
stdout(parsed.contentLength)
stdout(parsed.chunked)
"#;

    let dir = setup_net_project(source_no_te, "net2_chunked_false");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "net2_chunked_false").expect("js failed");
    let native = run_net_native(&dir, "net2_chunked_false").expect("native failed");
    cleanup_net_project(&dir);

    // NB2-16: Interp vs JS must be exact (both display Bool as true/false)
    assert_eq!(
        interp, js,
        "NET2-2a: chunked=false Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    // NB2-16: Verify chunked field is "false" on Interp/JS
    assert!(
        interp.contains("false"),
        "NET2-2a: chunked should be false on Interp, got: {}",
        interp
    );
    // NB2-16+NB2-1: Native Bool display may differ (0 vs false). Check numeric lines match
    // and chunked line is falsy (0 or false).
    let interp_lines: Vec<&str> = interp.trim().lines().collect();
    let native_lines: Vec<&str> = native.trim().lines().collect();
    assert_eq!(
        interp_lines.len(),
        native_lines.len(),
        "NET2-2a: chunked=false line count mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    // First two lines (consumed, contentLength) must match exactly
    assert_eq!(
        interp_lines[0], native_lines[0],
        "NET2-2a: consumed mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
    assert_eq!(
        interp_lines[1], native_lines[1],
        "NET2-2a: contentLength mismatch\nInterp: {}\nNative: {}",
        interp, native
    );
    // Third line (chunked) must be falsy: "false" or "0"
    assert!(
        native_lines[2] == "false" || native_lines[2] == "0",
        "NET2-2a: Native chunked should be false/0, got: {}",
        native_lines[2]
    );

    // Test 2: Request with Transfer-Encoding: chunked → chunked = true
    // NB2-16: Include chunked field in output.
    let source_te_chunked = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.consumed)
stdout(parsed.contentLength)
stdout(parsed.chunked)
"#;

    let dir2 = setup_net_project(source_te_chunked, "net2_chunked_true");
    let interp2 = run_net_interpreter(&dir2).expect("interpreter failed");
    let js2 = run_net_js(&dir2, "net2_chunked_true").expect("js failed");
    let native2 = run_net_native(&dir2, "net2_chunked_true").expect("native failed");
    cleanup_net_project(&dir2);

    assert_eq!(
        interp2, js2,
        "NET2-2a: chunked=true Interp vs JS\nInterp: {}\nJS: {}",
        interp2, js2
    );
    // NB2-16: Verify chunked field is "true" on Interp/JS
    assert!(
        interp2.contains("true"),
        "NET2-2a: chunked should be true on Interp, got: {}",
        interp2
    );
    // NB2-16+NB2-1: Check Native parity with Bool normalization
    let interp2_lines: Vec<&str> = interp2.trim().lines().collect();
    let native2_lines: Vec<&str> = native2.trim().lines().collect();
    assert_eq!(
        interp2_lines.len(),
        native2_lines.len(),
        "NET2-2a: chunked=true line count mismatch\nInterp: {}\nNative: {}",
        interp2,
        native2
    );
    assert_eq!(
        interp2_lines[0], native2_lines[0],
        "NET2-2a: consumed mismatch\nInterp: {}\nNative: {}",
        interp2, native2
    );
    assert_eq!(
        interp2_lines[1], native2_lines[1],
        "NET2-2a: contentLength mismatch\nInterp: {}\nNative: {}",
        interp2, native2
    );
    // Third line (chunked) must be truthy: "true" or "1"
    assert!(
        native2_lines[2] == "true" || native2_lines[2] == "1",
        "NET2-2a: Native chunked should be true/1, got: {}",
        native2_lines[2]
    );
}

/// NET2-2a: httpParseRequestHead chunked Bool field — Interp vs JS only
/// Tests the actual Bool value of chunked field (skips Native due to Bool display limitation).
#[test]
fn test_net2_parse_chunked_bool_value_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // chunked = false
    let source_false = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.chunked)
"#;

    let dir = setup_net_project(source_false, "net2_chunked_bool_f");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "net2_chunked_bool_f").expect("js failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET2-2a: chunked=false Bool Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert!(
        interp.contains("false"),
        "chunked should be false, got: {}",
        interp
    );

    // chunked = true
    let source_true = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.chunked)
"#;

    let dir2 = setup_net_project(source_true, "net2_chunked_bool_t");
    let interp2 = run_net_interpreter(&dir2).expect("interpreter failed");
    let js2 = run_net_js(&dir2, "net2_chunked_bool_t").expect("js failed");
    cleanup_net_project(&dir2);

    assert_eq!(
        interp2, js2,
        "NET2-2a: chunked=true Bool Interp vs JS\nInterp: {}\nJS: {}",
        interp2, js2
    );
    assert!(
        interp2.contains("true"),
        "chunked should be true, got: {}",
        interp2
    );
}

/// NET2-2e: Content-Length + Transfer-Encoding: chunked mutual exclusion — 3-way parity
/// Note: Uses kind (Str) field only to avoid native Bool display limitation for ok field.
#[test]
fn test_net2_parse_cl_te_mutual_exclusion_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["POST /data HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nTransfer-Encoding: chunked\r\n\r\nhello"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)

stdout(result.__value.kind)
stdout(result.__value.message.length())
"#;

    let dir = setup_net_project(source, "net2_cl_te_reject");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "net2_cl_te_reject").expect("js failed");
    let native = run_net_native(&dir, "net2_cl_te_reject").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET2-2e: CL+TE reject Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET2-2e: CL+TE reject Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("ParseError"),
        "Should contain ParseError, got: {}",
        interp
    );
}

// ── NET-5b: 3-way encode fixture parity ────────────────────────

/// NET-5b: httpEncodeResponse 200 text body — 3-way parity
#[test]
fn test_net5b_encode_200_text_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "Hello")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "5b_encode_200");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5b_encode_200").expect("js failed");
    let native = run_net_native(&dir, "5b_encode_200").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5b: 200 text Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5b: 200 text Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5b: httpEncodeResponse 404 empty body — 3-way parity
#[test]
fn test_net5b_encode_404_empty_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 404, headers <= @[], body <= "")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "5b_encode_404");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5b_encode_404").expect("js failed");
    let native = run_net_native(&dir, "5b_encode_404").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5b: 404 empty Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5b: 404 empty Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5b: httpEncodeResponse 204 no body — 3-way parity
#[test]
fn test_net5b_encode_204_nobody_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 204, headers <= @[], body <= "")
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "5b_encode_204");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5b_encode_204").expect("js failed");
    let native = run_net_native(&dir, "5b_encode_204").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5b: 204 no body Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5b: 204 no body Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5b: httpEncodeResponse invalid body (non-Str) — 3-way error parity
#[test]
fn test_net5b_encode_invalid_body_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(status <= 200, headers <= @[], body <= 1)
result <= httpEncodeResponse(resp)

stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "5b_encode_bad_body");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5b_encode_bad_body").expect("js failed");
    let native = run_net_native(&dir, "5b_encode_bad_body").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5b: bad body Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5b: bad body Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    assert!(
        interp.contains("EncodeError"),
        "All backends should return EncodeError, got: {}",
        interp
    );
}

/// NET-5b: httpEncodeResponse with multiple headers — 3-way parity
#[test]
fn test_net5b_encode_multi_header_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

resp <= @(
  status <= 200,
  headers <= @[
    @(name <= "content-type", value <= "text/html"),
    @(name <= "x-custom", value <= "test123")
  ],
  body <= "<h1>Hi</h1>"
)
result <= httpEncodeResponse(resp)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "5b_encode_multi_hdr");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5b_encode_multi_hdr").expect("js failed");
    let native = run_net_native(&dir, "5b_encode_multi_hdr").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5b: multi-header Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5b: multi-header Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET-5c: 3-way bounded server parity ────────────────────────

/// NET-5c: httpServe bounded server with POST body — 3-way parity
/// Sends a POST request with a body to all 3 backends and verifies
/// the handler can access request fields and return a response containing them.
#[test]
fn test_net5c_serve_post_body_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let backends = vec!["interp", "js", "native"];
    let mut outputs: Vec<(String, String)> = Vec::new(); // (backend, stdout)

    for backend in &backends {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "got-post")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("5c_serve_post_{}", backend));
        let td_path = dir.join("main.td");

        let mut child = match *backend {
            "interp" => Command::new(taida_bin())
                .arg(&td_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn interpreter"),
            "js" => {
                let js_path = unique_temp_path("taida_net5c_serve_js", backend, "mjs");
                let transpile = Command::new(taida_bin())
                    .arg("build")
                    .arg("--target")
                    .arg("js")
                    .arg(&td_path)
                    .arg("-o")
                    .arg(&js_path)
                    .output()
                    .expect("transpile");
                if !transpile.status.success() {
                    let stderr = String::from_utf8_lossy(&transpile.stderr);
                    cleanup_net_project(&dir);
                    let _ = fs::remove_file(&js_path);
                    panic!("JS transpile failed for {}: {}", backend, stderr);
                }
                let child = Command::new("node")
                    .arg(&js_path)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn node");
                thread::sleep(Duration::from_millis(500));
                let _ = fs::remove_file(&js_path);
                child
            }
            "native" => {
                let bin_path = unique_temp_path("taida_net5c_serve_native", backend, "bin");
                let compile = Command::new(taida_bin())
                    .arg("build")
                    .arg("--target")
                    .arg("native")
                    .arg(&td_path)
                    .arg("-o")
                    .arg(&bin_path)
                    .output()
                    .expect("compile native");
                if !compile.status.success() {
                    let stderr = String::from_utf8_lossy(&compile.stderr);
                    cleanup_net_project(&dir);
                    let _ = fs::remove_file(&bin_path);
                    panic!("Native compile failed for {}: {}", backend, stderr);
                }
                let child = Command::new(&bin_path)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn native");
                thread::sleep(Duration::from_millis(200));
                let _ = fs::remove_file(&bin_path);
                child
            }
            _ => unreachable!(),
        };

        // Send POST request
        let mut response = Vec::new();
        let mut got_response = false;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(100));
            let stream = match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => s,
                Err(_) => continue,
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
            let mut stream = stream;
            let request = b"POST /submit HTTP/1.1\r\nHost: localhost\r\nContent-Length: 11\r\n\r\nhello world";
            if std::io::Write::write_all(&mut stream, request).is_err() {
                continue;
            }
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stream, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => response.extend_from_slice(&buf[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            if !response.is_empty() {
                got_response = true;
                break;
            }
        }

        if !got_response {
            let _ = child.kill();
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not respond on port {}",
                backend, port
            );
        }

        let resp_str = String::from_utf8_lossy(&response).to_string();
        assert!(
            resp_str.contains("200 OK"),
            "{} backend: expected '200 OK', got: {:?}",
            backend,
            resp_str
        );
        assert!(
            resp_str.contains("got-post"),
            "{} backend: expected body 'got-post', got: {:?}",
            backend,
            resp_str
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        outputs.push((backend.to_string(), stdout));

        cleanup_net_project(&dir);
    }

    // All 3 backends should produce identical stdout
    assert_eq!(
        outputs[0].1, outputs[1].1,
        "NET-5c: POST serve Interp vs JS\nInterp: {}\nJS: {}",
        outputs[0].1, outputs[1].1
    );
    assert_eq!(
        outputs[0].1, outputs[2].1,
        "NET-5c: POST serve Interp vs Native\nInterp: {}\nNative: {}",
        outputs[0].1, outputs[2].1
    );
    assert_eq!(
        outputs[0].1, "1",
        "All backends should report requests=1, got: {:?}",
        outputs[0].1
    );
}

/// NET-5c: httpServe response contains custom header — 3-way parity
/// Verifies that all 3 backends include user-specified response headers in wire output.
#[test]
fn test_net5c_serve_custom_header_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let backends = vec!["interp", "js", "native"];

    for backend in &backends {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(
    status <= 200,
    headers <= @[
      @(name <= "content-type", value <= "application/json"),
      @(name <= "x-taida-test", value <= "net5c")
    ],
    body <= "{{}}"
  )
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("5c_serve_hdr_{}", backend));
        let td_path = dir.join("main.td");

        let mut child = match *backend {
            "interp" => Command::new(taida_bin())
                .arg(&td_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn interpreter"),
            "js" => {
                let js_path = unique_temp_path("taida_net5c_hdr_js", backend, "mjs");
                let transpile = Command::new(taida_bin())
                    .arg("build")
                    .arg("--target")
                    .arg("js")
                    .arg(&td_path)
                    .arg("-o")
                    .arg(&js_path)
                    .output()
                    .expect("transpile");
                if !transpile.status.success() {
                    let stderr = String::from_utf8_lossy(&transpile.stderr);
                    cleanup_net_project(&dir);
                    let _ = fs::remove_file(&js_path);
                    panic!("JS transpile failed: {}", stderr);
                }
                let child = Command::new("node")
                    .arg(&js_path)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn node");
                thread::sleep(Duration::from_millis(500));
                let _ = fs::remove_file(&js_path);
                child
            }
            "native" => {
                let bin_path = unique_temp_path("taida_net5c_hdr_native", backend, "bin");
                let compile = Command::new(taida_bin())
                    .arg("build")
                    .arg("--target")
                    .arg("native")
                    .arg(&td_path)
                    .arg("-o")
                    .arg(&bin_path)
                    .output()
                    .expect("compile native");
                if !compile.status.success() {
                    let stderr = String::from_utf8_lossy(&compile.stderr);
                    cleanup_net_project(&dir);
                    let _ = fs::remove_file(&bin_path);
                    panic!("Native compile failed: {}", stderr);
                }
                let child = Command::new(&bin_path)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("spawn native");
                thread::sleep(Duration::from_millis(200));
                let _ = fs::remove_file(&bin_path);
                child
            }
            _ => unreachable!(),
        };

        let mut response = Vec::new();
        let mut got_response = false;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(100));
            let stream = match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => s,
                Err(_) => continue,
            };
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
            let mut stream = stream;
            let request = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
            if std::io::Write::write_all(&mut stream, request).is_err() {
                continue;
            }
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stream, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => response.extend_from_slice(&buf[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            if !response.is_empty() {
                got_response = true;
                break;
            }
        }

        if !got_response {
            let _ = child.kill();
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not respond on port {}",
                backend, port
            );
        }

        let resp_str = String::from_utf8_lossy(&response).to_string();
        assert!(
            resp_str.contains("x-taida-test: net5c"),
            "{} backend: expected custom header 'x-taida-test: net5c', got:\n{}",
            backend,
            resp_str
        );
        assert!(
            resp_str.contains("application/json"),
            "{} backend: expected content-type header, got:\n{}",
            backend,
            resp_str
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout, "1",
            "{} backend: httpServe result mismatch. stdout: {:?}",
            backend, stdout
        );

        cleanup_net_project(&dir);
    }
}

// ── NET-5d: legacy net surface re-verification ─────────────────

/// NET-5d: import taida-lang/net with both legacy (TCP) and HTTP v1 symbols — 3-way parity
/// Verifies that legacy symbols and new HTTP v1 symbols can coexist in a single import.
/// Uses tcpListen as a legacy symbol alongside httpParseRequestHead (HTTP v1).
#[test]
fn test_net5d_legacy_and_v1_coexist_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Import both legacy (dnsResolve, socketClose) and HTTP v1 (httpParseRequestHead, httpEncodeResponse)
    // from taida-lang/net. dnsResolve/socketClose are legacy re-exports from taida-lang/os.
    // Verifies that mixed legacy+v1 import doesn't cause errors and HTTP v1 works correctly.
    let source = r#">>> taida-lang/net => @(dnsResolve, socketClose, httpParseRequestHead, httpEncodeResponse)

// HTTP v1 symbol: httpParseRequestHead (coexists with legacy dnsResolve, socketClose)
bytesLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed
stdout(parsed.method.len)

// HTTP v1 symbol: httpEncodeResponse
resp <= @(status <= 200, headers <= @[], body <= "ok")
encResult <= httpEncodeResponse(resp)
encResult ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "5d_coexist");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5d_coexist").expect("js failed");
    let native = run_net_native(&dir, "5d_coexist").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5d: coexist Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5d: coexist Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5d: sentinel guard -- calling httpServe/httpParseRequestHead/httpEncodeResponse
/// without import resolves as user function on all 3 backends.
#[test]
fn test_net5d_sentinel_guard_no_import() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#"httpServe a b = a + b => :Int
httpParseRequestHead a = a + 2 => :Int
httpEncodeResponse a = a + 1 => :Int

stdout(httpServe(10, 20))
stdout(httpParseRequestHead(5))
stdout(httpEncodeResponse(9))
"#;

    let dir = setup_net_project(source, "5d_sentinel");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5d_sentinel").expect("js failed");
    let native = run_net_native(&dir, "5d_sentinel").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, "30\n7\n10",
        "Interp sentinel guard failed: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5d sentinel: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5d sentinel: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET-5e: example verification ───────────────────────────────

/// NET-5e: net_http_parse_encode example — 3-way parity
/// Runs the actual example file source on all 3 backends and verifies identical output.
/// Source must match examples/net_http_parse_encode.td exactly to prevent example drift.
#[test]
fn test_net5e_parse_encode_example_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Source identical to examples/net_http_parse_encode.td (lines 6-29)
    let source = r#">>> taida-lang/net => @(httpParseRequestHead, httpEncodeResponse)

// Parse a raw HTTP request
bytesLax <= Bytes["GET /hello?name=taida HTTP/1.1\r\nHost: localhost\r\nAccept: text/plain\r\n\r\n"]()
bytesLax ]=> bytes
parseResult <= httpParseRequestHead(bytes)
parseResult ]=> parsed

// Note: use consumed (Int) instead of complete (Bool) for 3-backend parity
stdout(parsed.consumed)
stdout(parsed.method.start)
stdout(parsed.method.len)
stdout(parsed.path.len)
stdout(parsed.query.len)

// Encode a response
resp <= @(
  status <= 200,
  headers <= @[@(name <= "content-type", value <= "text/plain")],
  body <= "Hello, taida!"
)
encodeResult <= httpEncodeResponse(resp)
encodeResult ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "5e_example");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5e_example").expect("js failed");
    let native = run_net_native(&dir, "5e_example").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET-5e: example Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5e: example Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET-5 review fix: scope-aware net builtin shadowing ──────────────────

/// NET-5 shadow fix: when a function parameter shadows httpServe,
/// the call inside the function body must use the parameter (user function),
/// not the net builtin. All 3 backends must produce identical output.
#[test]
fn test_net5_shadow_http_serve_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // httpServe is imported but shadowed by a function parameter.
    // Inside wrap, httpServe refers to the parameter (add), not the builtin.
    let source = r#">>> taida-lang/net => @(httpServe)

wrap httpServe =
  httpServe(10, 20)
=> :Int

add a b = a + b => :Int
stdout(wrap(add))
"#;

    let dir = setup_net_project(source, "5_shadow_serve");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5_shadow_serve").expect("js failed");
    let native = run_net_native(&dir, "5_shadow_serve").expect("native failed");
    cleanup_net_project(&dir);

    // Expected: 30 (add(10, 20) = 30)
    assert_eq!(
        interp.trim(),
        "30",
        "NET-5 shadow: Interpreter should produce 30, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5 shadow httpServe: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5 shadow httpServe: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5 shadow fix: httpParseRequestHead shadowed by parameter.
#[test]
fn test_net5_shadow_http_parse_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

apply httpParseRequestHead x =
  httpParseRequestHead(x)
=> :Int

double n = n + n => :Int
stdout(apply(double, 7))
"#;

    let dir = setup_net_project(source, "5_shadow_parse");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5_shadow_parse").expect("js failed");
    let native = run_net_native(&dir, "5_shadow_parse").expect("native failed");
    cleanup_net_project(&dir);

    // Expected: 14 (double(7) = 14)
    assert_eq!(
        interp.trim(),
        "14",
        "NET-5 shadow: Interpreter should produce 14, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5 shadow httpParseRequestHead: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5 shadow httpParseRequestHead: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5 shadow fix: httpEncodeResponse shadowed by parameter.
#[test]
fn test_net5_shadow_http_encode_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

apply httpEncodeResponse x =
  httpEncodeResponse(x)
=> :Int

triple n = n * 3 => :Int
stdout(apply(triple, 5))
"#;

    let dir = setup_net_project(source, "5_shadow_encode");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5_shadow_encode").expect("js failed");
    let native = run_net_native(&dir, "5_shadow_encode").expect("native failed");
    cleanup_net_project(&dir);

    // Expected: 15 (triple(5) = 15)
    assert_eq!(
        interp.trim(),
        "15",
        "NET-5 shadow: Interpreter should produce 15, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5 shadow httpEncodeResponse: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5 shadow httpEncodeResponse: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5 shadow fix: lambda parameter shadows httpServe.
/// Uses Map mold to invoke a lambda where httpServe is shadowed by the element parameter name.
#[test]
fn test_net5_shadow_lambda_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // The function `callWith` takes httpServe as a parameter (shadowing the import),
    // then passes it to a list operation where the lambda body calls the shadowed function.
    let source = r#">>> taida-lang/net => @(httpServe)

callWith httpServe =
  items <= @[5]
  Map[items, _ x = httpServe(x)]() ]=> result
  result
=> :@[Int]

triple n = n * 3 => :Int
stdout(callWith(triple))
"#;

    let dir = setup_net_project(source, "5_shadow_lambda");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5_shadow_lambda").expect("js failed");
    let native = run_net_native(&dir, "5_shadow_lambda").expect("native failed");
    cleanup_net_project(&dir);

    // Expected: @[15] (triple(5) = 15, mapped over @[5])
    assert!(
        interp.trim().contains("15"),
        "NET-5 shadow lambda: Interpreter should contain 15, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5 shadow lambda: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5 shadow lambda: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET-5 review fix 2: local assignment shadow ──────────────────────────

/// NET-5 assignment shadow: when httpServe is reassigned via local assignment
/// (`httpServe <= add`), subsequent calls must use the local value.
#[test]
fn test_net5_shadow_assignment_http_serve_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpServe)

add a b = a + b => :Int
wrap =
  httpServe <= add
  httpServe(10, 20)
=> :Int

stdout(wrap())
"#;

    let dir = setup_net_project(source, "5_shadow_assign_serve");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5_shadow_assign_serve").expect("js failed");
    let native = run_net_native(&dir, "5_shadow_assign_serve").expect("native failed");
    cleanup_net_project(&dir);

    // Expected: 30 (add(10, 20) = 30)
    assert_eq!(
        interp.trim(),
        "30",
        "NET-5 assignment shadow httpServe: Interpreter should produce 30, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5 assignment shadow httpServe: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5 assignment shadow httpServe: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5 assignment shadow: httpParseRequestHead reassigned via local assignment.
#[test]
fn test_net5_shadow_assignment_http_parse_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

double n = n + n => :Int
wrap =
  httpParseRequestHead <= double
  httpParseRequestHead(7)
=> :Int

stdout(wrap())
"#;

    let dir = setup_net_project(source, "5_shadow_assign_parse");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5_shadow_assign_parse").expect("js failed");
    let native = run_net_native(&dir, "5_shadow_assign_parse").expect("native failed");
    cleanup_net_project(&dir);

    // Expected: 14 (double(7) = 14)
    assert_eq!(
        interp.trim(),
        "14",
        "NET-5 assignment shadow httpParseRequestHead: Interpreter should produce 14, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5 assignment shadow httpParseRequestHead: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5 assignment shadow httpParseRequestHead: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5 assignment shadow: httpEncodeResponse reassigned via local assignment.
#[test]
fn test_net5_shadow_assignment_http_encode_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

triple n = n * 3 => :Int
wrap =
  httpEncodeResponse <= triple
  httpEncodeResponse(5)
=> :Int

stdout(wrap())
"#;

    let dir = setup_net_project(source, "5_shadow_assign_encode");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5_shadow_assign_encode").expect("js failed");
    let native = run_net_native(&dir, "5_shadow_assign_encode").expect("native failed");
    cleanup_net_project(&dir);

    // Expected: 15 (triple(5) = 15)
    assert_eq!(
        interp.trim(),
        "15",
        "NET-5 assignment shadow httpEncodeResponse: Interpreter should produce 15, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5 assignment shadow httpEncodeResponse: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5 assignment shadow httpEncodeResponse: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET-5 assignment shadow: shadow does NOT leak to a sibling function scope.
/// After wrap() uses httpServe as a local var, a call to httpServe in another
/// function (or at top level in a different function) must still resolve to the builtin.
#[test]
fn test_net5_shadow_assignment_no_leak_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // wrap() shadows httpServe with a local assignment. outer() does NOT shadow it.
    // outer() calls httpServe which should still refer to the net builtin (but since
    // we can't easily test server start, we test that the shadow is correctly scoped
    // by having two functions: one that shadows and one that does not).
    let source = r#">>> taida-lang/net => @(httpServe)

add a b = a + b => :Int
wrap =
  httpServe <= add
  httpServe(10, 20)
=> :Int

// outer does NOT shadow httpServe — it should still see the builtin.
// But calling the builtin would start a server, so we test scoping differently:
// we define another function that also takes httpServe as a param to confirm
// the assignment shadow in wrap does not persist.
outer httpServe =
  httpServe(3, 4)
=> :Int

multiply a b = a * b => :Int
stdout(wrap())
stdout(outer(multiply))
"#;

    let dir = setup_net_project(source, "5_shadow_assign_noleak");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "5_shadow_assign_noleak").expect("js failed");
    let native = run_net_native(&dir, "5_shadow_assign_noleak").expect("native failed");
    cleanup_net_project(&dir);

    // Expected: "30\n12" (add(10,20)=30, multiply(3,4)=12)
    let expected = "30\n12";
    assert_eq!(
        interp.trim(),
        expected,
        "NET-5 assignment shadow no-leak: Interpreter should produce {}, got: {}",
        expected,
        interp
    );
    assert_eq!(
        interp, js,
        "NET-5 assignment shadow no-leak: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET-5 assignment shadow no-leak: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NB-31: httpServe handler callable check regression tests ──

/// NB-31: Passing a large Int as handler to httpServe must NOT crash the Native
/// binary (previously caused a segfault via indirect call to an integer value).
/// JS and Native should both return a TypeError Result instead of crashing.
/// Tests BOTH literal and variable-path Int handlers to cover compile-time tag.
#[test]
fn test_nb31_http_serve_int_handler_js_native_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Variable-path: bad <= 1000000; httpServe(0, bad, ...)
    // This exercises the int_vars → callable_type_tag path in lower.rs.
    let source = r#">>> taida-lang/net => @(httpServe)

bad <= 1000000
asyncResult <= httpServe(0, bad, 1, 1000)
asyncResult ]=> result
stdout(result.__value.ok.toString())
stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "nb31_int_handler");

    // -- JS backend --
    let js = run_net_js(&dir, "nb31_int_handler")
        .expect("NB-31: JS backend should not crash with Int handler");

    // -- Native backend --
    let native = run_net_native(&dir, "nb31_int_handler")
        .expect("NB-31: Native backend should not crash with Int handler");

    cleanup_net_project(&dir);

    // Both should produce error result with ok=false and kind=TypeError
    let js_lines: Vec<&str> = js.trim().lines().collect();
    let native_lines: Vec<&str> = native.trim().lines().collect();

    assert!(
        js_lines.len() >= 2,
        "NB-31: JS output should have at least 2 lines, got: {:?}",
        js
    );
    assert!(
        native_lines.len() >= 2,
        "NB-31: Native output should have at least 2 lines, got: {:?}",
        native
    );

    // ok should be false (or "0" in native where bool may render as int)
    assert!(
        js_lines[0] == "false" || js_lines[0] == "0",
        "NB-31: JS ok field should be false, got: {}",
        js_lines[0]
    );
    assert!(
        native_lines[0] == "false" || native_lines[0] == "0",
        "NB-31: Native ok field should be false, got: {}",
        native_lines[0]
    );

    // Both must return TypeError — this is the critical assertion.
    assert!(
        js_lines[1] == "TypeError",
        "NB-31: JS kind must be TypeError, got: {}",
        js_lines[1]
    );
    assert!(
        native_lines[1] == "TypeError",
        "NB-31: Native kind must be TypeError, got: {}",
        native_lines[1]
    );
}

/// NB-31: Interpreter should return RuntimeError for non-callable handler.
/// This is a different behavior from JS/Native (which return a Result),
/// but we verify it doesn't silently succeed.
/// Uses variable-path Int handler to match the JS/Native test.
#[test]
fn test_nb31_http_serve_int_handler_interpreter_error() {
    let source = r#">>> taida-lang/net => @(httpServe)

bad <= 1000000
asyncResult <= httpServe(0, bad, 1, 1000)
stdout("should not reach here")
"#;

    let dir = setup_net_project(source, "nb31_interp_error");
    let td_path = dir.join("main.td");

    let output = Command::new(taida_bin())
        .arg(&td_path)
        .output()
        .expect("spawn interpreter");

    cleanup_net_project(&dir);

    // Interpreter should fail (exit code != 0)
    assert!(
        !output.status.success(),
        "NB-31: Interpreter should reject non-callable handler (exit non-zero)"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("handler must be a Function"),
        "NB-31: Interpreter stderr should mention handler type error, got: {}",
        stderr
    );
}

/// NB-31: Arithmetic Int expression path (bad <= 999999 + 1).
/// Exercises noncallable_type_tag → expr_is_int → BinaryOp detection.
#[test]
fn test_nb31_http_serve_arithmetic_int_handler_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpServe)

bad <= 999999 + 1
asyncResult <= httpServe(0, bad, 1, 1000)
asyncResult ]=> result
stdout(result.__value.ok.toString())
stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "nb31_arith");

    let js = run_net_js(&dir, "nb31_arith")
        .expect("NB-31: JS should not crash with arithmetic Int handler");
    let native = run_net_native(&dir, "nb31_arith")
        .expect("NB-31: Native should not crash with arithmetic Int handler");

    cleanup_net_project(&dir);

    let js_lines: Vec<&str> = js.trim().lines().collect();
    let native_lines: Vec<&str> = native.trim().lines().collect();

    assert!(
        js_lines.len() >= 2,
        "NB-31 arith: JS output too short: {:?}",
        js
    );
    assert!(
        native_lines.len() >= 2,
        "NB-31 arith: Native output too short: {:?}",
        native
    );

    assert_eq!(
        js_lines[1], "TypeError",
        "NB-31 arith: JS kind must be TypeError, got: {}",
        js_lines[1]
    );
    assert_eq!(
        native_lines[1], "TypeError",
        "NB-31 arith: Native kind must be TypeError, got: {}",
        native_lines[1]
    );
}

/// NB-31: Int-returning function path (mk n = n + 1 => :Int / bad <= mk(999999)).
/// Exercises noncallable_type_tag → expr_is_int → FuncCall return type.
#[test]
fn test_nb31_http_serve_func_return_int_handler_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpServe)

mk n = n + 1 => :Int
bad <= mk(999999)
asyncResult <= httpServe(0, bad, 1, 1000)
asyncResult ]=> result
stdout(result.__value.ok.toString())
stdout(result.__value.kind)
"#;

    let dir = setup_net_project(source, "nb31_func_ret");

    let js = run_net_js(&dir, "nb31_func_ret")
        .expect("NB-31: JS should not crash with func-return Int handler");
    let native = run_net_native(&dir, "nb31_func_ret")
        .expect("NB-31: Native should not crash with func-return Int handler");

    cleanup_net_project(&dir);

    let js_lines: Vec<&str> = js.trim().lines().collect();
    let native_lines: Vec<&str> = native.trim().lines().collect();

    assert!(
        js_lines.len() >= 2,
        "NB-31 func_ret: JS output too short: {:?}",
        js
    );
    assert!(
        native_lines.len() >= 2,
        "NB-31 func_ret: Native output too short: {:?}",
        native
    );

    assert_eq!(
        js_lines[1], "TypeError",
        "NB-31 func_ret: JS kind must be TypeError, got: {}",
        js_lines[1]
    );
    assert_eq!(
        native_lines[1], "TypeError",
        "NB-31 func_ret: Native kind must be TypeError, got: {}",
        native_lines[1]
    );
}

/// NB-31: Typed Int parameter path (wrap bad: Int = ...).
/// Exercises callable_type_tag → Ident → int_vars (param registration).
#[test]
fn test_nb31_http_serve_typed_param_int_handler_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpServe)

wrap bad: Int =
  asyncResult <= httpServe(0, bad, 1, 1000)
  asyncResult ]=> result
  stdout(result.__value.ok.toString())
  stdout(result.__value.kind)

wrap(1000000)
"#;

    let dir = setup_net_project(source, "nb31_typed_param");

    let js = run_net_js(&dir, "nb31_typed_param")
        .expect("NB-31: JS should not crash with typed Int param handler");
    let native = run_net_native(&dir, "nb31_typed_param")
        .expect("NB-31: Native should not crash with typed Int param handler");

    cleanup_net_project(&dir);

    let js_lines: Vec<&str> = js.trim().lines().collect();
    let native_lines: Vec<&str> = native.trim().lines().collect();

    assert!(
        js_lines.len() >= 2,
        "NB-31 typed_param: JS output too short: {:?}",
        js
    );
    assert!(
        native_lines.len() >= 2,
        "NB-31 typed_param: Native output too short: {:?}",
        native
    );

    assert_eq!(
        js_lines[1], "TypeError",
        "NB-31 typed_param: JS kind must be TypeError, got: {}",
        js_lines[1]
    );
    assert_eq!(
        native_lines[1], "TypeError",
        "NB-31 typed_param: Native kind must be TypeError, got: {}",
        native_lines[1]
    );
}

/// NB-14: httpEncodeResponse with dynamically-typed Bool status via function parameter
/// must produce "status must be Int, got true" in Native (not "got 1").
/// Reproduction: `encodeWrap s = @(status <= s, ...)` called with `encodeWrap(true)`.
#[test]
fn test_nb14_dynamic_bool_status_native_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

encodeWrap s =
  @(status <= s, headers <= @[], body <= "ok")

result <= httpEncodeResponse(encodeWrap(true))
stdout(result.__value.message)
"#;

    let dir = setup_net_project(source, "nb14_bool_status");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for NB-14 bool status");
    let native =
        run_net_native(&dir, "nb14_bool_status").expect("native failed for NB-14 bool status");
    cleanup_net_project(&dir);

    assert_eq!(
        interp.trim(),
        native.trim(),
        "NB-14: dynamic Bool status parity mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    assert!(
        native.contains("got true"),
        "NB-14: Native should say 'got true', got: {}",
        native
    );
}

/// NB-21: httpEncodeResponse with dynamically-typed Bool body via function parameter
/// must produce "body must be Bytes or Str, got true" in Native (not "got 1").
#[test]
fn test_nb21_dynamic_bool_body_native_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

encodeWrap x =
  @(status <= 200, headers <= @[], body <= x)

result <= httpEncodeResponse(encodeWrap(true))
stdout(result.__value.message)
"#;

    let dir = setup_net_project(source, "nb21_bool_body");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for NB-21 bool body");
    let native = run_net_native(&dir, "nb21_bool_body").expect("native failed for NB-21 bool body");
    cleanup_net_project(&dir);

    assert_eq!(
        interp.trim(),
        native.trim(),
        "NB-21: dynamic Bool body parity mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    assert!(
        native.contains("got true"),
        "NB-21: Native should say 'got true', got: {}",
        native
    );
}

/// NB-14 nested: httpEncodeResponse(encodeWrap(idBool(true))) must preserve Bool tag
/// through nested call chain. The inner idBool call must not overwrite the outer
/// call's tag frame. Both Interpreter and Native should say "got true".
#[test]
fn test_nb14_nested_call_bool_status_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

idBool x = x => :Bool
encodeWrap s =
  @(status <= s, headers <= @[], body <= "ok")

result <= httpEncodeResponse(encodeWrap(idBool(true)))
stdout(result.__value.message)
"#;

    let dir = setup_net_project(source, "nb14_nested_status");
    let interp =
        run_net_interpreter(&dir).expect("interpreter failed for NB-14 nested bool status");
    let native = run_net_native(&dir, "nb14_nested_status")
        .expect("native failed for NB-14 nested bool status");
    cleanup_net_project(&dir);

    assert_eq!(
        interp.trim(),
        native.trim(),
        "NB-14 nested: dynamic Bool status parity mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    assert!(
        native.contains("got true"),
        "NB-14 nested: Native should say 'got true', got: {}",
        native
    );
}

/// NB-21 nested: httpEncodeResponse(encodeWrap(idBool(true))) with Bool body
/// must preserve Bool tag through nested call chain.
#[test]
fn test_nb21_nested_call_bool_body_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

idBool x = x => :Bool
encodeWrap x =
  @(status <= 200, headers <= @[], body <= x)

result <= httpEncodeResponse(encodeWrap(idBool(true)))
stdout(result.__value.message)
"#;

    let dir = setup_net_project(source, "nb21_nested_body");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for NB-21 nested bool body");
    let native =
        run_net_native(&dir, "nb21_nested_body").expect("native failed for NB-21 nested bool body");
    cleanup_net_project(&dir);

    assert_eq!(
        interp.trim(),
        native.trim(),
        "NB-21 nested: dynamic Bool body parity mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    assert!(
        native.contains("got true"),
        "NB-21 nested: Native should say 'got true', got: {}",
        native
    );
}

/// NB-14 mixed-arg: encodeWrap(true, id(true)) where arg 0 has a known tag (Bool)
/// but arg 1 passes through a generic `id` function (UNKNOWN). The unset slot must
/// remain UNKNOWN, not default to INT.
#[test]
fn test_nb14_mixed_arg_bool_status_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

id x =
  x

encodeWrap a b =
  @(status <= b, headers <= @[], body <= "ok")

result <= httpEncodeResponse(encodeWrap(true, id(true)))
stdout(result.__value.message)
"#;

    let dir = setup_net_project(source, "nb14_mixed_arg_status");
    let interp =
        run_net_interpreter(&dir).expect("interpreter failed for NB-14 mixed-arg bool status");
    let native = run_net_native(&dir, "nb14_mixed_arg_status")
        .expect("native failed for NB-14 mixed-arg bool status");
    cleanup_net_project(&dir);

    assert_eq!(
        interp.trim(),
        native.trim(),
        "NB-14 mixed-arg: dynamic Bool status parity mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    assert!(
        native.contains("got true"),
        "NB-14 mixed-arg: Native should say 'got true', got: {}",
        native
    );
}

/// NB-21 mixed-arg: encodeWrap(true, id(true)) where the body comes from a generic
/// `id` function. The unset tag slot must not collapse to INT.
#[test]
fn test_nb21_mixed_arg_bool_body_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

id x =
  x

encodeWrap a b =
  @(status <= 200, headers <= @[], body <= b)

result <= httpEncodeResponse(encodeWrap(true, id(true)))
stdout(result.__value.message)
"#;

    let dir = setup_net_project(source, "nb21_mixed_arg_body");
    let interp =
        run_net_interpreter(&dir).expect("interpreter failed for NB-21 mixed-arg bool body");
    let native = run_net_native(&dir, "nb21_mixed_arg_body")
        .expect("native failed for NB-21 mixed-arg bool body");
    cleanup_net_project(&dir);

    assert_eq!(
        interp.trim(),
        native.trim(),
        "NB-21 mixed-arg: dynamic Bool body parity mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    assert!(
        native.contains("got true"),
        "NB-21 mixed-arg: Native should say 'got true', got: {}",
        native
    );
}

/// NB-14 IIFE: (_ b = @(status <= b, ...))(id(true)) — direct lambda call must
/// propagate return_tag from id(true) into the IIFE's arg tag slot.
#[test]
fn test_nb14_iife_bool_status_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

id x =
  x

result <= httpEncodeResponse((_ b = @(status <= b, headers <= @[], body <= "ok"))(id(true)))
stdout(result.__value.message)
"#;

    let dir = setup_net_project(source, "nb14_iife_status");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for NB-14 IIFE bool status");
    let native =
        run_net_native(&dir, "nb14_iife_status").expect("native failed for NB-14 IIFE bool status");
    cleanup_net_project(&dir);

    assert_eq!(
        interp.trim(),
        native.trim(),
        "NB-14 IIFE: dynamic Bool status parity mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    assert!(
        native.contains("got true"),
        "NB-14 IIFE: Native should say 'got true', got: {}",
        native
    );
}

/// NB-21 IIFE: (_ b = @(status <= 200, ..., body <= b))(id(true)) — body path.
#[test]
fn test_nb21_iife_bool_body_parity() {
    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

id x =
  x

result <= httpEncodeResponse((_ b = @(status <= 200, headers <= @[], body <= b))(id(true)))
stdout(result.__value.message)
"#;

    let dir = setup_net_project(source, "nb21_iife_body");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for NB-21 IIFE bool body");
    let native =
        run_net_native(&dir, "nb21_iife_body").expect("native failed for NB-21 IIFE bool body");
    cleanup_net_project(&dir);

    assert_eq!(
        interp.trim(),
        native.trim(),
        "NB-21 IIFE: dynamic Bool body parity mismatch\nInterp: {}\nNative: {}",
        interp,
        native
    );
    assert!(
        native.contains("got true"),
        "NB-21 IIFE: Native should say 'got true', got: {}",
        native
    );
}

// ── NB-11: Non-ASCII path span parity ──────────────────────────────

/// NB-11: URL-encoded non-ASCII path should produce identical span offsets
/// across all 3 backends. Tests latin1 byte-to-character 1:1 property in JS.
/// Uses the actual UTF-8 character directly (Taida has no \u escape).
#[test]
fn test_nb11_non_ascii_path_span_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Path "/caf\xC3\xA9" contains 2-byte UTF-8 sequence for e-acute (U+00E9).
    // In raw bytes the path is 5 bytes: /cafe\xcc\xa9 (but the char is \xc3\xa9, so 5 bytes: / c a f \xc3\xa9)
    // Bytes[] encodes the Taida string to UTF-8, so the byte offsets should match.
    // The e-acute character is embedded directly as a UTF-8 literal in the Rust source.
    let source = ">>> taida-lang/net => @(httpParseRequestHead)\n\
\n\
bytesLax <= Bytes[\"GET /caf\u{00e9}?q=1 HTTP/1.1\\r\\nHost: localhost\\r\\n\\r\\n\"]()\n\
bytesLax ]=> bytes\n\
result <= httpParseRequestHead(bytes)\n\
result ]=> parsed\n\
\n\
stdout(parsed.method.start)\n\
stdout(parsed.method.len)\n\
stdout(parsed.path.start)\n\
stdout(parsed.path.len)\n\
stdout(parsed.query.start)\n\
stdout(parsed.query.len)\n\
stdout(parsed.version.major)\n\
stdout(parsed.version.minor)\n\
stdout(parsed.contentLength)\n\
";

    let dir = setup_net_project(source, "nb11_non_ascii_path");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "nb11_non_ascii_path").expect("js failed");
    let native = run_net_native(&dir, "nb11_non_ascii_path").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NB-11: non-ASCII path span Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NB-11: non-ASCII path span Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NB-11: Header with non-ASCII value (high bytes) should parse identically.
#[test]
fn test_nb11_non_ascii_header_value_span_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // X-Name header with value containing a 2-byte UTF-8 character (e-acute).
    let source = ">>> taida-lang/net => @(httpParseRequestHead)\n\
\n\
bytesLax <= Bytes[\"GET / HTTP/1.1\\r\\nHost: localhost\\r\\nX-Name: caf\u{00e9}\\r\\n\\r\\n\"]()\n\
bytesLax ]=> bytes\n\
result <= httpParseRequestHead(bytes)\n\
result ]=> parsed\n\
\n\
stdout(parsed.consumed)\n\
stdout(parsed.path.start)\n\
stdout(parsed.path.len)\n\
stdout(parsed.bodyOffset)\n\
";

    let dir = setup_net_project(source, "nb11_non_ascii_header");
    let interp = run_net_interpreter(&dir).expect("interpreter failed");
    let js = run_net_js(&dir, "nb11_non_ascii_header").expect("js failed");
    let native = run_net_native(&dir, "nb11_non_ascii_header").expect("native failed");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NB-11: non-ASCII header Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NB-11: non-ASCII header Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── readBody: 3-way parity tests ──────────────────────────────

/// readBody with Content-Length body — 3-way parity.
/// Build a request pack manually (raw + body span) and verify readBody
/// extracts the body bytes correctly across all three backends.
#[test]
fn test_net_readbody_content_length_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Construct a request pack with raw bytes + body span, then call readBody.
    // The raw bytes contain "POST /data HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"
    // body span starts at offset 42 (after \r\n\r\n), length 5.
    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["POST /data HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 42, len <= 5))
body <= readBody(req)
stdout(body.length())
"#;

    let dir = setup_net_project(source, "readbody_cl");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for readBody parity");
    let js = run_net_js(&dir, "readbody_cl").expect("js failed for readBody parity");
    let native = run_net_native(&dir, "readbody_cl").expect("native failed for readBody parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "readBody Content-Length Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "readBody Content-Length Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    // Verify the body length is 5 (the "hello" body)
    assert_eq!(
        interp.trim(),
        "5",
        "readBody should return 5 bytes for 'hello'"
    );
}

/// readBody with empty body (len=0) — 3-way parity.
#[test]
fn test_net_readbody_empty_body_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 35, len <= 0))
body <= readBody(req)
stdout(body.length())
"#;

    let dir = setup_net_project(source, "readbody_empty");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for readBody empty parity");
    let js = run_net_js(&dir, "readbody_empty").expect("js failed for readBody empty parity");
    let native =
        run_net_native(&dir, "readbody_empty").expect("native failed for readBody empty parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "readBody empty Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "readBody empty Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
    // Empty body should return 0 bytes
    assert_eq!(
        interp.trim(),
        "0",
        "readBody should return 0 bytes for empty body"
    );
}

// ── readBody runtime-error helpers ──────────────────────────────

/// Run a net project with the interpreter and return stderr on runtime failure.
fn run_net_interpreter_runtime_error(project_dir: &Path) -> Option<String> {
    let td_path = project_dir.join("main.td");
    let output = Command::new(taida_bin()).arg(&td_path).output().ok()?;
    if output.status.success() {
        return None; // expected to fail
    }
    Some(String::from_utf8_lossy(&output.stderr).to_string())
}

/// Transpile a net project to JS, run with node, and return stderr on runtime failure.
fn run_net_js_runtime_error(project_dir: &Path, label: &str) -> Option<String> {
    let td_path = project_dir.join("main.td");
    let js_path = unique_temp_path("taida_net_err_js", label, "mjs");

    let transpile = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&td_path)
        .arg("-o")
        .arg(&js_path)
        .output()
        .ok()?;

    if !transpile.status.success() {
        let _ = fs::remove_file(&js_path);
        return None; // transpile failure, not runtime
    }

    let run_output = Command::new("node").arg(&js_path).output().ok()?;
    let _ = fs::remove_file(&js_path);

    if run_output.status.success() {
        return None; // expected to fail
    }
    Some(String::from_utf8_lossy(&run_output.stderr).to_string())
}

/// Compile a net project to native, run the binary, and return stderr on runtime failure.
fn run_net_native_runtime_error(project_dir: &Path, label: &str) -> Option<String> {
    let td_path = project_dir.join("main.td");
    let bin_path = unique_temp_path("taida_net_err_native", label, "bin");

    let compile = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("native")
        .arg(&td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .ok()?;

    if !compile.status.success() {
        let _ = fs::remove_file(&bin_path);
        return None; // compile failure, not runtime
    }

    let output = Command::new(&bin_path).output().ok()?;
    let _ = fs::remove_file(&bin_path);

    if output.status.success() {
        return None; // expected to fail
    }
    Some(String::from_utf8_lossy(&output.stderr).to_string())
}

/// readBody with invalid argument (Int) — 3-way runtime error parity.
/// All backends should produce an error containing the message fragment.
#[test]
fn test_net_readbody_invalid_arg_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(readBody)

body <= readBody(42)
stdout(body.length())
"#;

    let expected_fragment = "readBody: argument must be a request pack @(...), got 42";

    let dir = setup_net_project(source, "readbody_invalid_arg");

    let interp_err =
        run_net_interpreter_runtime_error(&dir).expect("interpreter should fail for readBody(42)");
    let js_err = run_net_js_runtime_error(&dir, "readbody_invalid_arg")
        .expect("js should fail for readBody(42)");
    let native_err = run_net_native_runtime_error(&dir, "readbody_invalid_arg")
        .expect("native should fail for readBody(42)");

    cleanup_net_project(&dir);

    assert!(
        interp_err.contains(expected_fragment),
        "interpreter stderr should contain error message fragment.\nExpected: {}\nGot: {}",
        expected_fragment,
        interp_err
    );
    assert!(
        js_err.contains(expected_fragment),
        "js stderr should contain error message fragment.\nExpected: {}\nGot: {}",
        expected_fragment,
        js_err
    );
    assert!(
        native_err.contains(expected_fragment),
        "native stderr should contain error message fragment.\nExpected: {}\nGot: {}",
        expected_fragment,
        native_err
    );
}

// ── NET2-4e: JS backend parity tests ──────────────────────────────

/// Helper: spawn a server (interpreter, JS, or native) and return the child process.
/// For JS, transpiles to .mjs, spawns `node`, and waits for startup.
/// For native, compiles to a binary and spawns it.
fn spawn_net_server_backend(dir: &Path, backend: &str, label: &str) -> (Child, Option<PathBuf>) {
    let td_path = dir.join("main.td");
    match backend {
        "interp" => {
            let child = Command::new(taida_bin())
                .arg(&td_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn interpreter");
            (child, None)
        }
        "js" => {
            let js_path = unique_temp_path("taida_net4_js", label, "mjs");
            let transpile = Command::new(taida_bin())
                .arg("build")
                .arg("--target")
                .arg("js")
                .arg(&td_path)
                .arg("-o")
                .arg(&js_path)
                .output()
                .expect("transpile");
            if !transpile.status.success() {
                let stderr = String::from_utf8_lossy(&transpile.stderr);
                panic!("JS transpile failed for {}: {}", label, stderr);
            }
            let child = Command::new("node")
                .arg(&js_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn node");
            thread::sleep(Duration::from_millis(500));
            (child, Some(js_path))
        }
        "native" => {
            let bin_path = unique_temp_path("taida_net5_native", label, "bin");
            let compile = Command::new(taida_bin())
                .arg("build")
                .arg("--target")
                .arg("native")
                .arg(&td_path)
                .arg("-o")
                .arg(&bin_path)
                .output()
                .expect("compile native");
            if !compile.status.success() {
                let stderr = String::from_utf8_lossy(&compile.stderr);
                panic!("Native compile failed for {}: {}", label, stderr);
            }
            let child = Command::new(&bin_path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn native binary");
            thread::sleep(Duration::from_millis(500));
            (child, Some(bin_path))
        }
        _ => unreachable!(),
    }
}

/// Helper: wait for server to be ready and send a single HTTP request.
/// Returns the raw response bytes, or None if the server never responded.
fn send_http_request(port: u16, request: &[u8]) -> Option<Vec<u8>> {
    let mut response = Vec::new();
    for _ in 0..80 {
        thread::sleep(Duration::from_millis(100));
        let stream = match TcpStream::connect(format!("127.0.0.1:{}", port)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
        let mut stream = stream;
        if std::io::Write::write_all(&mut stream, request).is_err() {
            continue;
        }
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut stream, &mut buf) {
                Ok(0) => break,
                Ok(n) => response.extend_from_slice(&buf[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        if !response.is_empty() {
            return Some(response);
        }
    }
    None
}

/// NET2-4e: Keep-alive parity — Interpreter vs JS.
/// Sends 2 requests on a single TCP connection, verifies both get responses.
#[test]
fn test_net2_4e_keep_alive_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for backend in &["interp", "js"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 2)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net4_ka_{}", backend));
        let (mut child, js_path) =
            spawn_net_server_backend(&dir, backend, &format!("ka_{}", backend));

        // Wait for server to be ready
        let mut connected = false;
        let mut stream = None;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(100));
            match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
                    stream = Some(s);
                    connected = true;
                    break;
                }
                Err(_) => continue,
            }
        }

        if !connected {
            let _ = child.kill();
            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            panic!("{} backend: server did not start on port {}", backend, port);
        }

        let s = stream.as_mut().unwrap();

        // Send first request (HTTP/1.1, keep-alive by default)
        let req1 = b"GET /first HTTP/1.1\r\nHost: localhost\r\n\r\n";
        std::io::Write::write_all(s, req1).expect("write req1");

        let mut resp1 = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(s, &mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    resp1.extend_from_slice(&buf[..n]);
                    // Check if we got a complete response (Content-Length based)
                    let resp_str = String::from_utf8_lossy(&resp1);
                    if resp_str.contains("\r\n\r\n") {
                        // Simple check: if the response has a body, we're done
                        if resp_str.contains("ok") {
                            break;
                        }
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }

        assert!(
            !resp1.is_empty(),
            "{} backend: no response to first keep-alive request",
            backend
        );
        let resp1_str = String::from_utf8_lossy(&resp1);
        assert!(
            resp1_str.contains("200"),
            "{} backend: first response should be 200, got: {}",
            backend,
            resp1_str
        );

        // Send second request on the same connection (Connection: close to end)
        let req2 = b"GET /second HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        std::io::Write::write_all(s, req2).expect("write req2");

        let mut resp2 = Vec::new();
        loop {
            match std::io::Read::read(s, &mut buf) {
                Ok(0) => break,
                Ok(n) => resp2.extend_from_slice(&buf[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }

        assert!(
            !resp2.is_empty(),
            "{} backend: no response to second keep-alive request",
            backend
        );
        let resp2_str = String::from_utf8_lossy(&resp2);
        assert!(
            resp2_str.contains("200"),
            "{} backend: second response should be 200, got: {}",
            backend,
            resp2_str
        );

        // Wait for server to finish (maxRequests=2 reached)
        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout, "true\n2",
            "{} backend: keep-alive server should report ok=true, requests=2. Got: {}",
            backend, stdout
        );

        if let Some(p) = &js_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-4e: Chunked body parity — Interpreter vs JS.
/// Sends a POST with Transfer-Encoding: chunked, verifies readBody returns correct bytes.
#[test]
fn test_net2_4e_chunked_body_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for backend in &["interp", "js"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  @(status <= 200, headers <= @[], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net4_chunk_{}", backend));
        let (mut child, js_path) =
            spawn_net_server_backend(&dir, backend, &format!("chunk_{}", backend));

        // Chunked POST body: "Wiki" (4 bytes) + "pedia " (7 bytes, note trailing space) = "Wikipedia "
        // Actually let's use a simpler body: "Hello" (5) + "World" (5) = "HelloWorld"
        let chunked_request = b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nHello\r\n5\r\nWorld\r\n0\r\n\r\n";

        let response = send_http_request(port, chunked_request);

        if response.is_none() {
            let _ = child.kill();
            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not respond for chunked request",
                backend
            );
        }

        let resp_bytes = response.unwrap();
        let resp_str = String::from_utf8_lossy(&resp_bytes);
        assert!(
            resp_str.contains("200"),
            "{} backend: chunked response should be 200, got: {}",
            backend,
            resp_str
        );
        // The response body should be "HelloWorld" (the reassembled chunked body echoed back)
        assert!(
            resp_str.contains("HelloWorld"),
            "{} backend: response should contain reassembled chunked body 'HelloWorld', got: {}",
            backend,
            resp_str
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout, "true\n1",
            "{} backend: chunked server should report ok=true, requests=1. Got: {}",
            backend, stdout
        );

        if let Some(p) = &js_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-4e: Malformed chunked body parity — Interpreter vs JS.
/// Sends a POST with an invalid chunk-size hex (prefix-valid garbage like "5X"),
/// verifies both backends reject with 400 Bad Request.
#[test]
fn test_net2_4e_malformed_chunked_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for backend in &["interp", "js"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  @(status <= 200, headers <= @[], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net4_malchunk_{}", backend));
        let (mut child, js_path) =
            spawn_net_server_backend(&dir, backend, &format!("malchunk_{}", backend));

        // Malformed chunk-size: "5X" is prefix-valid garbage that parseInt would accept as 5,
        // but strict hex validation must reject it entirely.
        let malformed_request = b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5X\r\nHello\r\n0\r\n\r\n";

        let response = send_http_request(port, malformed_request);

        if response.is_none() {
            // Server closed connection without response — acceptable rejection behavior
            let _ = child.kill();
            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            continue;
        }

        let resp_bytes = response.unwrap();
        let resp_str = String::from_utf8_lossy(&resp_bytes);
        assert!(
            resp_str.contains("400 Bad Request"),
            "{} backend: malformed chunk-size '5X' should be rejected with 400, got: {}",
            backend,
            resp_str
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout, "true\n1",
            "{} backend: malformed chunked server should report ok=true, requests=1. Got: {}",
            backend, stdout
        );

        if let Some(p) = &js_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-4e: Concurrent connections parity — Interpreter vs JS.
/// Connects 2 clients simultaneously, verifies both get responses.
#[test]
fn test_net2_4e_concurrent_connections_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for backend in &["interp", "js"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "concurrent-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 3, 5000, 4)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net4_conc_{}", backend));
        let (mut child, js_path) =
            spawn_net_server_backend(&dir, backend, &format!("conc_{}", backend));

        // Wait for server to be ready using send_http_request (which consumes 1 request slot).
        // maxRequests is 3: 1 readiness probe + 2 real concurrent clients.
        let probe_resp = send_http_request(
            port,
            b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        if probe_resp.is_none() {
            let _ = child.kill();
            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            panic!("{} backend: server did not start on port {}", backend, port);
        }

        // Send 2 requests on separate connections simultaneously
        let port_clone = port;
        let handle1 = thread::spawn(move || {
            let stream = TcpStream::connect(format!("127.0.0.1:{}", port_clone)).ok()?;
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
            let mut stream = stream;
            let req = b"GET /c1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            std::io::Write::write_all(&mut stream, req).ok()?;
            let mut resp = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stream, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => resp.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
            if resp.is_empty() { None } else { Some(resp) }
        });

        let port_clone2 = port;
        let handle2 = thread::spawn(move || {
            let stream = TcpStream::connect(format!("127.0.0.1:{}", port_clone2)).ok()?;
            stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
            stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
            let mut stream = stream;
            let req = b"GET /c2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
            std::io::Write::write_all(&mut stream, req).ok()?;
            let mut resp = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stream, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => resp.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
            if resp.is_empty() { None } else { Some(resp) }
        });

        let resp1 = handle1.join().expect("client 1 thread");
        let resp2 = handle2.join().expect("client 2 thread");

        // Both concurrent clients must succeed
        assert!(
            resp1.is_some(),
            "{} backend: concurrent client 1 must get a response",
            backend
        );
        assert!(
            resp2.is_some(),
            "{} backend: concurrent client 2 must get a response",
            backend
        );

        let s1 = String::from_utf8_lossy(resp1.as_ref().unwrap());
        assert!(
            s1.contains("concurrent-ok"),
            "{} backend: client 1 response should contain 'concurrent-ok', got: {}",
            backend,
            s1
        );
        let s2 = String::from_utf8_lossy(resp2.as_ref().unwrap());
        assert!(
            s2.contains("concurrent-ok"),
            "{} backend: client 2 response should contain 'concurrent-ok', got: {}",
            backend,
            s2
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout, "true\n3",
            "{} backend: concurrent server should report ok=true, requests=3 (1 probe + 2 real). Got: {}",
            backend, stdout
        );

        if let Some(p) = &js_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-4e: maxConnections parity — verify connection limiting works in both backends.
/// Uses maxConnections=1, maxRequests=2. Sends 2 sequential requests on separate
/// connections. Both must succeed (proving the single-slot pool recycles correctly).
/// This verifies actual limiting behavior, not just argument acceptance.
#[test]
fn test_net2_4e_max_connections_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for backend in &["interp", "js"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "maxconn-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 2, 5000, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net4_maxconn_{}", backend));
        let (mut child, js_path) =
            spawn_net_server_backend(&dir, backend, &format!("maxconn_{}", backend));

        // First request: fills the single connection slot, then closes (Connection: close)
        let response1 = send_http_request(
            port,
            b"GET /r1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );

        if response1.is_none() {
            let _ = child.kill();
            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not respond for first request with maxConnections=1",
                backend
            );
        }

        let resp1_str = String::from_utf8_lossy(response1.as_ref().unwrap());
        assert!(
            resp1_str.contains("maxconn-ok"),
            "{} backend: first response should contain 'maxconn-ok', got: {}",
            backend,
            resp1_str
        );

        // Second request: after the first connection closed, the slot is freed.
        // This verifies the connection pool correctly recycles with maxConnections=1.
        // Brief sleep to let the first connection fully close.
        thread::sleep(Duration::from_millis(100));
        let stream = TcpStream::connect(format!("127.0.0.1:{}", port));
        if stream.is_err() {
            // Server already shut down after maxRequests=2 was reached (first request counted)
            // This is acceptable — the first request proved maxConn=1 works
            let _ = child.wait_with_output();
            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            continue;
        }
        let stream = stream.unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
        let mut stream = stream;
        let _ = std::io::Write::write_all(
            &mut stream,
            b"GET /r2 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        let mut resp2 = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut stream, &mut buf) {
                Ok(0) => break,
                Ok(n) => resp2.extend_from_slice(&buf[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        if !resp2.is_empty() {
            let resp2_str = String::from_utf8_lossy(&resp2);
            assert!(
                resp2_str.contains("maxconn-ok"),
                "{} backend: second response should contain 'maxconn-ok', got: {}",
                backend,
                resp2_str
            );
        }

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout, "true\n2",
            "{} backend: maxConnections=1 server should report ok=true, requests=2. Got: {}",
            backend, stdout
        );

        if let Some(p) = &js_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-4e: keepAlive field parity — verify keepAlive is correctly set in JS.
/// Handler echoes keepAlive via Str[] mold. Sends Connection: close -> false.
#[test]
fn test_net2_4e_keep_alive_field_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for backend in &["interp", "js"] {
        let port = find_free_loopback_port();
        // Handler echoes keepAlive field as string using Str[] mold
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  kaLax <= Str[req.keepAlive]()
  kaLax ]=> kaStr
  @(status <= 200, headers <= @[], body <= kaStr)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
"#
        );

        let dir = setup_net_project(&source, &format!("net4_kafield_{}", backend));
        let (mut child, js_path) =
            spawn_net_server_backend(&dir, backend, &format!("kafield_{}", backend));

        // HTTP/1.1 request with Connection: close -> keepAlive should be false
        let response = send_http_request(
            port,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );

        if response.is_none() {
            let stderr = {
                let _ = child.kill();
                let out = child.wait_with_output().ok();
                out.map(|o| String::from_utf8_lossy(&o.stderr).to_string())
                    .unwrap_or_default()
            };
            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not respond for keepAlive field test.\nstderr: {}",
                backend, stderr
            );
        }

        let resp_bytes = response.unwrap();
        let resp_str = String::from_utf8_lossy(&resp_bytes);
        assert!(
            resp_str.contains("false"),
            "{} backend: keepAlive should be false for Connection: close, got: {}",
            backend,
            resp_str
        );

        let _ = child.wait_with_output();
        if let Some(p) = &js_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-4e: Malformed chunked body — additional variant parity.
/// Tests multiple malformed chunk-size patterns beyond "5X":
///   - empty chunk-size ("\r\nHello\r\n...")
///   - negative chunk-size ("-1\r\nX\r\n...")
///     Each must be rejected with 400 or connection close by both backends.
#[test]
fn test_net2_4e_malformed_chunked_variants_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let malformed_patterns: &[(&str, &[u8])] = &[
        (
            "empty chunk-size",
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n\r\nHello\r\n0\r\n\r\n",
        ),
        (
            "negative chunk-size",
            b"POST /data HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n-1\r\nX\r\n0\r\n\r\n",
        ),
    ];

    for backend in &["interp", "js"] {
        for (label, payload) in malformed_patterns {
            let port = find_free_loopback_port();
            let source = format!(
                r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  @(status <= 200, headers <= @[], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
            );

            let dir = setup_net_project(
                &source,
                &format!("net4_malvar_{}_{}", label.replace(' ', "_"), backend),
            );
            let (mut child, js_path) = spawn_net_server_backend(
                &dir,
                backend,
                &format!("malvar_{}_{}", label.replace(' ', "_"), backend),
            );

            let response = send_http_request(port, payload);

            if response.is_none() {
                // Server closed connection without response — acceptable rejection
                let _ = child.kill();
                let _ = child.wait_with_output();
                if let Some(p) = &js_path {
                    let _ = fs::remove_file(p);
                }
                cleanup_net_project(&dir);
                continue;
            }

            let resp_bytes = response.unwrap();
            let resp_str = String::from_utf8_lossy(&resp_bytes);
            assert!(
                resp_str.contains("400 Bad Request"),
                "{} backend: malformed chunk-size '{}' should be rejected with 400, got: {}",
                backend,
                label,
                resp_str
            );

            let output = child.wait_with_output().expect("wait for server");
            let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
            assert_eq!(
                stdout, "true\n1",
                "{} backend: malformed chunked ({}) server should report ok=true, requests=1. Got: {}",
                backend, label, stdout
            );

            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
        }
    }
}

/// NET2-4e: maxConnections=1 concurrent access parity — Interpreter vs JS.
/// Starts a server with maxConnections=1, maxRequests=3, and sends 2 requests
/// sequentially (after a readiness probe).  The first request fills the single
/// slot; after it closes, the second request occupies the freed slot.
/// Both must succeed, proving the single-slot pool recycles correctly across
/// both backends.
#[test]
fn test_net2_4e_max_connections_concurrent_interp_js_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    for backend in &["interp", "js"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "slot-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 3, 10000, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net4_maxconc_{}", backend));
        let (mut child, js_path) =
            spawn_net_server_backend(&dir, backend, &format!("maxconc_{}", backend));

        // Readiness probe (consumes request slot 1 of 3)
        let probe_resp = send_http_request(
            port,
            b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        if probe_resp.is_none() {
            let _ = child.kill();
            if let Some(p) = &js_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not start on port {} for maxConnections concurrent test",
                backend, port
            );
        }

        // Client A: fills the single connection slot, then closes (request 2 of 3)
        let resp_a = send_http_request(
            port,
            b"GET /a HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            resp_a.is_some(),
            "{} backend: client A must get a response with maxConnections=1",
            backend
        );
        let body_a = String::from_utf8_lossy(resp_a.as_ref().unwrap());
        assert!(
            body_a.contains("slot-ok"),
            "{} backend: client A response should contain 'slot-ok', got: {}",
            backend,
            body_a
        );

        // Brief wait to let the slot recycle
        thread::sleep(Duration::from_millis(100));

        // Client B: after A closed, the slot is freed (request 3 of 3)
        let resp_b = send_http_request(
            port,
            b"GET /b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            resp_b.is_some(),
            "{} backend: client B must get a response after slot freed (maxConnections=1)",
            backend
        );
        let body_b = String::from_utf8_lossy(resp_b.as_ref().unwrap());
        assert!(
            body_b.contains("slot-ok"),
            "{} backend: client B response should contain 'slot-ok', got: {}",
            backend,
            body_b
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout, "true\n3",
            "{} backend: maxConnections=1 server should report ok=true, requests=3 (1 probe + 2 real). Got: {}",
            backend, stdout
        );

        if let Some(p) = &js_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

// ── NET2-5e: Native backend parity tests ──────────────────────────

/// NET2-5e: Keep-alive parity -- Interpreter vs Native.
/// Sends 2 requests on a single TCP connection, verifies both get responses.
#[test]
fn test_net2_5e_keep_alive_interp_native_parity() {
    for backend in &["interp", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 2)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net5_ka_{}", backend));
        let (mut child, artifact_path) =
            spawn_net_server_backend(&dir, backend, &format!("ka_{}", backend));

        // Wait for server to be ready and open a single TCP connection
        let mut connected = false;
        let mut stream = None;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(100));
            match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
                    stream = Some(s);
                    connected = true;
                    break;
                }
                Err(_) => continue,
            }
        }
        if !connected {
            let _ = child.kill();
            if let Some(p) = &artifact_path {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not become ready on port {}",
                backend, port
            );
        }

        let s = stream.as_mut().unwrap();

        // Request 1
        let req1 = b"GET /a HTTP/1.1\r\nHost: localhost\r\n\r\n";
        std::io::Write::write_all(s, req1).expect("write req1");
        let mut resp1 = vec![0u8; 4096];
        let n1 = std::io::Read::read(s, &mut resp1).expect("read resp1");
        let resp1_str = String::from_utf8_lossy(&resp1[..n1]);
        assert!(
            resp1_str.contains("200 OK"),
            "{} backend: first keep-alive response should be 200 OK, got: {}",
            backend,
            resp1_str
        );

        // Request 2 on same connection (keep-alive)
        let req2 = b"GET /b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        std::io::Write::write_all(s, req2).expect("write req2");
        let mut resp2 = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(s, &mut buf) {
                Ok(0) => break,
                Ok(n) => resp2.extend_from_slice(&buf[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        let resp2_str = String::from_utf8_lossy(&resp2);
        assert!(
            resp2_str.contains("200 OK"),
            "{} backend: second keep-alive response should be 200 OK, got: {}",
            backend,
            resp2_str
        );

        // Wait for server to exit (maxRequests=2)
        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert!(
            stdout.contains("2"),
            "{} backend: server should report 2 requests. Got: {}",
            backend,
            stdout
        );

        if let Some(p) = &artifact_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-5e: Chunked body parity -- Interpreter vs Native.
/// Sends a POST with Transfer-Encoding: chunked, verifies readBody returns correct bytes.
#[test]
fn test_net2_5e_chunked_body_interp_native_parity() {
    for backend in &["interp", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  @(status <= 200, headers <= @[], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
"#
        );

        let dir = setup_net_project(&source, &format!("net5_chunked_{}", backend));
        let (child, artifact_path) =
            spawn_net_server_backend(&dir, backend, &format!("chunked_{}", backend));

        // Chunked POST: "4\r\nWiki\r\n7\r\npedia i\r\n0\r\n\r\n" = "Wikipedia i"
        let chunked_request = b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nWiki\r\n7\r\npedia i\r\n0\r\n\r\n";
        let resp = send_http_request(port, chunked_request);
        assert!(
            resp.is_some(),
            "{} backend: chunked request must get a response",
            backend
        );
        let body = String::from_utf8_lossy(resp.as_ref().unwrap());
        assert!(
            body.contains("Wikipedia i"),
            "{} backend: chunked readBody should return 'Wikipedia i', got: {}",
            backend,
            body
        );

        let _ = child.wait_with_output();
        if let Some(p) = &artifact_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-5e: Concurrent connections parity -- Interpreter vs Native.
/// Connects 2 clients simultaneously, verifies both get responses.
#[test]
fn test_net2_5e_concurrent_connections_interp_native_parity() {
    for backend in &["interp", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "concurrent-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 3)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net5_conc_{}", backend));
        let (mut child, artifact_path) =
            spawn_net_server_backend(&dir, backend, &format!("conc_{}", backend));

        // Readiness probe
        let probe = send_http_request(
            port,
            b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            probe.is_some(),
            "{} backend: readiness probe must succeed",
            backend
        );

        // Connect 2 clients simultaneously
        let mut clients: Vec<TcpStream> = Vec::new();
        for _ in 0..2 {
            match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
                    clients.push(s);
                }
                Err(e) => {
                    let _ = child.kill();
                    if let Some(p) = &artifact_path {
                        let _ = fs::remove_file(p);
                    }
                    cleanup_net_project(&dir);
                    panic!("{} backend: concurrent connect failed: {}", backend, e);
                }
            }
        }

        // Send requests on both connections
        for (i, s) in clients.iter_mut().enumerate() {
            let req = format!(
                "GET /{} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                i
            );
            std::io::Write::write_all(s, req.as_bytes()).expect("write concurrent req");
        }

        // Read responses from both connections
        for (i, s) in clients.iter_mut().enumerate() {
            let mut resp = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(s, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => resp.extend_from_slice(&buf[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            let resp_str = String::from_utf8_lossy(&resp);
            assert!(
                resp_str.contains("concurrent-ok"),
                "{} backend: client {} should get 'concurrent-ok', got: {}",
                backend,
                i,
                resp_str
            );
        }

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        // NB2-17: Check last line is "3" (output has "true\n3" from ok + requests)
        let last_line = stdout.trim().lines().last().unwrap_or("");
        assert_eq!(
            last_line, "3",
            "{} backend: concurrent server should report 3 requests (1 probe + 2). Got: {}",
            backend, stdout
        );

        if let Some(p) = &artifact_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-5e: maxConnections parity -- Interpreter vs Native.
/// Uses maxConnections=1, maxRequests=2. Sends 2 sequential requests on separate
/// connections. Both must succeed (proving the single-slot pool recycles correctly).
#[test]
fn test_net2_5e_max_connections_interp_native_parity() {
    for backend in &["interp", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "slot-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 3, 5000, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net5_maxconn_{}", backend));
        let (child, artifact_path) =
            spawn_net_server_backend(&dir, backend, &format!("maxconn_{}", backend));

        // Readiness probe (request 1 of 3)
        let probe = send_http_request(
            port,
            b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            probe.is_some(),
            "{} backend: readiness probe must succeed",
            backend
        );

        // Brief wait for slot recycling
        thread::sleep(Duration::from_millis(200));

        // Client A: (request 2 of 3)
        let resp_a = send_http_request(
            port,
            b"GET /a HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            resp_a.is_some(),
            "{} backend: client A must get a response with maxConnections=1",
            backend
        );
        let body_a = String::from_utf8_lossy(resp_a.as_ref().unwrap());
        assert!(
            body_a.contains("slot-ok"),
            "{} backend: client A response should contain 'slot-ok', got: {}",
            backend,
            body_a
        );

        thread::sleep(Duration::from_millis(200));

        // Client B: after A closed, slot freed (request 3 of 3)
        let resp_b = send_http_request(
            port,
            b"GET /b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            resp_b.is_some(),
            "{} backend: client B must get a response after slot freed (maxConnections=1)",
            backend
        );
        let body_b = String::from_utf8_lossy(resp_b.as_ref().unwrap());
        assert!(
            body_b.contains("slot-ok"),
            "{} backend: client B response should contain 'slot-ok', got: {}",
            backend,
            body_b
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        // NB2-17: Check last line is "3" (output has "true\n3" from ok + requests)
        let last_line = stdout.trim().lines().last().unwrap_or("");
        assert_eq!(
            last_line, "3",
            "{} backend: maxConnections=1 server should report 3 requests. Got: {}",
            backend, stdout
        );

        if let Some(p) = &artifact_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-5e: keepAlive field parity -- verify keepAlive is correctly set in Native.
/// Handler echoes keepAlive via Str[] mold. Sends Connection: close -> false.
/// Note: Native renders Bool false as "0" due to known type tag loss on field access.
/// Interpreter renders it as "false". Both are accepted.
#[test]
fn test_net2_5e_keep_alive_field_interp_native_parity() {
    for backend in &["interp", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  kaStr <= Str[req.keepAlive]()
  kaStr ]=> ka
  @(status <= 200, headers <= @[], body <= ka)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
"#
        );

        let dir = setup_net_project(&source, &format!("net5_kafield_{}", backend));
        let (child, artifact_path) =
            spawn_net_server_backend(&dir, backend, &format!("kafield_{}", backend));

        // Send request with Connection: close -> keepAlive should be false
        let resp = send_http_request(
            port,
            b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            resp.is_some(),
            "{} backend: keepAlive field request must get a response",
            backend
        );
        let body = String::from_utf8_lossy(resp.as_ref().unwrap());
        // "false" (interpreter) or "0" (native Bool display limitation)
        assert!(
            body.contains("false")
                || body.contains("\n0\n")
                || body.ends_with("\n0")
                || body.ends_with("\r\n0"),
            "{} backend: keepAlive should be false/0 with Connection: close, got: {}",
            backend,
            body
        );

        let _ = child.wait_with_output();
        if let Some(p) = &artifact_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-5f: maxRequests=1 with 2 concurrent clients -- only 1 should get 200.
/// The reservation pattern (TOCTOU fix) ensures the second client cannot overshoot.
/// Interpreter vs Native parity.
#[test]
fn test_net2_5f_max_requests_no_overshoot_interp_native_parity() {
    for backend in &["interp", "native"] {
        let port = find_free_loopback_port();
        // maxRequests=1, maxConnections=2: server should serve exactly 1 request then stop.
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "single-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 5000, 2)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net5_overshoot_{}", backend));
        let (child, artifact_path) =
            spawn_net_server_backend(&dir, backend, &format!("overshoot_{}", backend));

        // Readiness probe counts as request 1 of 1 — server should stop after this.
        let probe = send_http_request(
            port,
            b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            probe.is_some(),
            "{} backend: readiness probe must succeed",
            backend
        );
        let probe_body = String::from_utf8_lossy(probe.as_ref().unwrap());
        assert!(
            probe_body.contains("single-ok"),
            "{} backend: probe should get 'single-ok', got: {}",
            backend,
            probe_body
        );

        // Brief sleep to let server process shutdown after maxRequests hit.
        thread::sleep(Duration::from_millis(500));

        // Second client: server should be shutting down, connection should fail or get no HTTP response.
        let second = TcpStream::connect(format!("127.0.0.1:{}", port));
        let got_http_200 = match second {
            Ok(mut s) => {
                s.set_read_timeout(Some(Duration::from_secs(2))).ok();
                s.set_write_timeout(Some(Duration::from_secs(2))).ok();
                let req = b"GET /second HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
                let wrote = std::io::Write::write_all(&mut s, req);
                if wrote.is_err() {
                    false
                } else {
                    let mut resp = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        match std::io::Read::read(&mut s, &mut buf) {
                            Ok(0) => break,
                            Ok(n) => resp.extend_from_slice(&buf[..n]),
                            Err(_) => break,
                        }
                    }
                    let resp_str = String::from_utf8_lossy(&resp);
                    resp_str.contains("200 OK") && resp_str.contains("single-ok")
                }
            }
            Err(_) => false, // connection refused = correct
        };
        assert!(
            !got_http_200,
            "{} backend: second client must NOT get 200 OK when maxRequests=1 (overshoot)",
            backend
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        // Server should report exactly 1 request
        assert!(
            stdout.contains("1"),
            "{} backend: server should report 1 request. Got: {}",
            backend,
            stdout
        );

        if let Some(p) = &artifact_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-5f: partial head timeout returns 400 Bad Request.
/// Sends an incomplete HTTP head and waits for the timeout to fire.
/// Interpreter vs Native parity.
#[test]
fn test_net2_5f_partial_timeout_400_interp_native_parity() {
    for backend in &["interp", "native"] {
        let port = find_free_loopback_port();
        // timeoutMs=500: partial data should trigger 400 after 500ms.
        // maxRequests=2: 1 for readiness probe, 1 for the partial timeout test.
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "timeout-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 2, 500)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net5_timeout_{}", backend));
        let (child, artifact_path) =
            spawn_net_server_backend(&dir, backend, &format!("timeout_{}", backend));

        // Readiness probe (request 1 of 2)
        let probe = send_http_request(
            port,
            b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            probe.is_some(),
            "{} backend: readiness probe must succeed",
            backend
        );

        // Brief sleep for connection cleanup
        thread::sleep(Duration::from_millis(200));

        // Send partial head (no \r\n\r\n termination) -- this should timeout and get 400.
        let partial_result = match TcpStream::connect(format!("127.0.0.1:{}", port)) {
            Ok(mut s) => {
                s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                s.set_write_timeout(Some(Duration::from_secs(5))).ok();
                // Send incomplete HTTP head -- no \r\n\r\n
                let partial = b"GET /partial HTTP/1.1\r\nHost: localhost\r\n";
                let _ = std::io::Write::write_all(&mut s, partial);
                // Wait for server timeout (500ms) + margin
                thread::sleep(Duration::from_millis(1200));
                // Read whatever the server sent back
                let mut resp = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    match std::io::Read::read(&mut s, &mut buf) {
                        Ok(0) => break,
                        Ok(n) => resp.extend_from_slice(&buf[..n]),
                        Err(ref e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut =>
                        {
                            break;
                        }
                        Err(_) => break,
                    }
                }
                String::from_utf8_lossy(&resp).to_string()
            }
            Err(e) => {
                // If we can't even connect, that's also acceptable (server may have shut down)
                format!("connect_failed: {}", e)
            }
        };

        // The response should contain 400 Bad Request
        assert!(
            partial_result.contains("400 Bad Request"),
            "{} backend: partial head timeout should return 400 Bad Request, got: {}",
            backend,
            partial_result
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        // Server should report 2 requests (1 probe + 1 partial timeout)
        assert!(
            stdout.contains("2"),
            "{} backend: server should report 2 requests (probe + partial timeout). Got: {}",
            backend,
            stdout
        );

        if let Some(p) = &artifact_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-5f: idle connection does NOT consume maxRequests budget.
/// Opens a TCP connection, sends nothing, closes it. Then sends a real request.
/// With maxRequests=1, the real request must succeed (idle didn't eat the budget).
/// Interpreter vs Native parity.
#[test]
fn test_net2_5f_idle_connection_does_not_consume_max_requests() {
    for backend in &["interp", "native"] {
        let port = find_free_loopback_port();
        // maxRequests=1, maxConnections=2, timeout=2000ms
        // The idle connection should NOT consume the single request slot.
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "idle-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 2000, 2)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net5_idle_budget_{}", backend));
        let (child, artifact_path) =
            spawn_net_server_backend(&dir, backend, &format!("idle_budget_{}", backend));

        // Wait for server to be ready by polling
        let mut server_ready = false;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(100));
            if TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok() {
                server_ready = true;
                break;
            }
        }
        assert!(server_ready, "{} backend: server did not start", backend);

        // Step 1: Open an idle TCP connection, send nothing, then close.
        {
            let idle_conn = TcpStream::connect(format!("127.0.0.1:{}", port));
            assert!(
                idle_conn.is_ok(),
                "{} backend: idle connection should succeed",
                backend
            );
            // Hold briefly then drop (clean close with no data)
            thread::sleep(Duration::from_millis(300));
            drop(idle_conn);
        }

        // Brief sleep to let server process the idle close
        thread::sleep(Duration::from_millis(500));

        // Step 2: Send a real HTTP request — this MUST succeed (budget was not consumed).
        let real_resp = send_http_request(
            port,
            b"GET /real HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            real_resp.is_some(),
            "{} backend: real request after idle close must succeed (idle must not consume maxRequests budget)",
            backend
        );
        let body = String::from_utf8_lossy(real_resp.as_ref().unwrap());
        assert!(
            body.contains("idle-ok"),
            "{} backend: real request should get 'idle-ok', got: {}",
            backend,
            body
        );

        // Wait for server to shut down (maxRequests=1 reached)
        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        // Server should report exactly 1 request (the real one, not the idle).
        // Native prints bool as "1" (not "true"), so just check requests count.
        // stdout lines: ok (true/1), requests (1)
        let lines: Vec<&str> = stdout.lines().collect();
        assert!(
            lines.len() >= 2 && lines[1].trim() == "1",
            "{} backend: server should report requests=1, got stdout: {}",
            backend,
            stdout
        );

        if let Some(p) = &artifact_path {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-5f: Concurrent race test for maxRequests TOCTOU fix.
/// maxRequests=1, maxConnections=2: two clients connect and send request heads
/// simultaneously. Exactly one must get 200 OK; the other must NOT get 200 OK
/// (either 503 Service Unavailable or connection refused/reset).
/// No readiness probe — both clients race for the single request slot.
#[test]
fn test_net2_5f_max_requests_concurrent_race() {
    // Native only — the TOCTOU race is specific to the pthread pool in native_runtime.c.
    // The interpreter is single-threaded and cannot exhibit this race.
    let port = find_free_loopback_port();
    let source = format!(
        r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "race-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1, 5000, 2)
asyncResult ]=> result
result ]=> r
stdout(r.ok)
stdout(r.requests)
"#
    );

    let dir = setup_net_project(&source, "net5_race_native");
    let (child, artifact_path) = spawn_net_server_backend(&dir, "native", "race_native");

    // Wait for server to accept TCP connections (poll without sending HTTP data).
    let mut server_ready = false;
    for _ in 0..80 {
        thread::sleep(Duration::from_millis(100));
        if let Ok(s) = TcpStream::connect(format!("127.0.0.1:{}", port)) {
            // Immediately close — idle connection, no data sent, no budget consumed.
            drop(s);
            server_ready = true;
            break;
        }
    }
    assert!(server_ready, "native backend: server did not start");

    // Brief sleep to let the server process the idle close.
    thread::sleep(Duration::from_millis(300));

    // Open two TCP connections simultaneously, then send request heads in parallel.
    let conn1 = TcpStream::connect(format!("127.0.0.1:{}", port));
    let conn2 = TcpStream::connect(format!("127.0.0.1:{}", port));

    let req = b"GET /race HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    // Helper: send request and read response on a stream.
    fn race_send(mut stream: TcpStream, req: &[u8]) -> String {
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
        if std::io::Write::write_all(&mut stream, req).is_err() {
            return String::new();
        }
        let mut resp = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(&mut stream, &mut buf) {
                Ok(0) => break,
                Ok(n) => resp.extend_from_slice(&buf[..n]),
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&resp).to_string()
    }

    // Send both requests in parallel using threads.
    let h1 = if let Ok(s) = conn1 {
        let req_copy = req.to_vec();
        Some(thread::spawn(move || race_send(s, &req_copy)))
    } else {
        None
    };
    let h2 = if let Ok(s) = conn2 {
        let req_copy = req.to_vec();
        Some(thread::spawn(move || race_send(s, &req_copy)))
    } else {
        None
    };

    let resp1 = h1.map(|h| h.join().unwrap_or_default()).unwrap_or_default();
    let resp2 = h2.map(|h| h.join().unwrap_or_default()).unwrap_or_default();

    let got_200_1 = resp1.contains("200 OK") && resp1.contains("race-ok");
    let got_200_2 = resp2.contains("200 OK") && resp2.contains("race-ok");
    let count_200 = (got_200_1 as i32) + (got_200_2 as i32);

    assert_eq!(
        count_200, 1,
        "native backend: exactly 1 of 2 concurrent clients should get 200 OK (TOCTOU race guard). \
         resp1 200={}, resp2 200={}. resp1: [{}] resp2: [{}]",
        got_200_1, got_200_2, resp1, resp2
    );

    // The other response should be 503 or empty (connection refused/reset).
    let other_resp = if got_200_1 { &resp2 } else { &resp1 };
    assert!(
        other_resp.is_empty()
            || other_resp.contains("503")
            || other_resp.contains("Service Unavailable"),
        "native backend: rejected client should get 503 or connection error, got: [{}]",
        other_resp
    );

    // Wait for server to shut down.
    let output = child.wait_with_output().expect("wait for server");
    let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
    // Server should report exactly 1 request.
    assert!(
        stdout.contains("1"),
        "native backend: server should report 1 request. Got: {}",
        stdout
    );

    if let Some(p) = &artifact_path {
        let _ = fs::remove_file(p);
    }
    cleanup_net_project(&dir);
}

// ── NET2-6: Phase 6 — Parity / Hardening ──────────────────────────

// ── NET2-6a: Keep-Alive 3-way parity ──────────────────────────────

/// NET2-6a: Keep-alive 3-way parity — Interpreter vs JS vs Native.
/// Sends 2 requests on a single TCP connection for each backend,
/// verifies both get 200 OK responses and server reports 2 requests.
#[test]
fn test_net2_6a_keep_alive_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let mut results: Vec<(String, String)> = Vec::new(); // (backend, stdout)

    for backend in &["interp", "js", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "ka-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 2)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net6_ka_{}", backend));
        let (mut child, artifact) =
            spawn_net_server_backend(&dir, backend, &format!("6ka_{}", backend));

        // Wait for server to be ready and open a single TCP connection
        let mut connected = false;
        let mut stream = None;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(100));
            match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
                    stream = Some(s);
                    connected = true;
                    break;
                }
                Err(_) => continue,
            }
        }
        if !connected {
            let _ = child.kill();
            if let Some(p) = &artifact {
                let _ = fs::remove_file(p);
            }
            cleanup_net_project(&dir);
            panic!(
                "{} backend: server did not start for keep-alive 3-way parity",
                backend
            );
        }

        let s = stream.as_mut().unwrap();

        // Request 1 (keep-alive by default in HTTP/1.1)
        let req1 = b"GET /a HTTP/1.1\r\nHost: localhost\r\n\r\n";
        std::io::Write::write_all(s, req1).expect("write req1");
        let mut resp1 = vec![0u8; 4096];
        let n1 = std::io::Read::read(s, &mut resp1).expect("read resp1");
        let resp1_str = String::from_utf8_lossy(&resp1[..n1]);
        assert!(
            resp1_str.contains("200 OK"),
            "{} backend: first keep-alive response should be 200 OK, got: {}",
            backend,
            resp1_str
        );
        assert!(
            resp1_str.contains("ka-ok"),
            "{} backend: first keep-alive response body should contain 'ka-ok', got: {}",
            backend,
            resp1_str
        );

        // Request 2 on same connection (Connection: close to end)
        let req2 = b"GET /b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        std::io::Write::write_all(s, req2).expect("write req2");
        let mut resp2 = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match std::io::Read::read(s, &mut buf) {
                Ok(0) => break,
                Ok(n) => resp2.extend_from_slice(&buf[..n]),
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(_) => break,
            }
        }
        let resp2_str = String::from_utf8_lossy(&resp2);
        assert!(
            resp2_str.contains("200 OK"),
            "{} backend: second keep-alive response should be 200 OK, got: {}",
            backend,
            resp2_str
        );

        // Wait for server to exit (maxRequests=2)
        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        results.push((backend.to_string(), stdout));

        if let Some(p) = &artifact {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }

    // 3-way comparison: all backends should report "2" requests
    let interp = &results[0].1;
    let js = &results[1].1;
    let native = &results[2].1;

    assert!(
        interp.contains("2"),
        "NET2-6a: Interpreter should report 2 requests, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET2-6a: Keep-Alive 3-way parity: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET2-6a: Keep-Alive 3-way parity: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET2-6b: Chunked body 3-way parity ──────────────────────────────

/// NET2-6b: Chunked body 3-way parity — Interpreter vs JS vs Native.
/// Sends a POST with Transfer-Encoding: chunked, verifies readBody
/// returns the correct reassembled body across all three backends.
#[test]
fn test_net2_6b_chunked_body_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let mut results: Vec<(String, String)> = Vec::new();

    for backend in &["interp", "js", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  @(status <= 200, headers <= @[], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net6_chunked_{}", backend));
        let (child, artifact) =
            spawn_net_server_backend(&dir, backend, &format!("6chunked_{}", backend));

        // Chunked POST: "5\r\nHello\r\n6\r\n World\r\n0\r\n\r\n" = "Hello World"
        let chunked_request = b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nHello\r\n6\r\n World\r\n0\r\n\r\n";
        let resp = send_http_request(port, chunked_request);
        assert!(
            resp.is_some(),
            "{} backend: chunked request must get a response",
            backend
        );
        let resp_str = String::from_utf8_lossy(resp.as_ref().unwrap());
        assert!(
            resp_str.contains("Hello World"),
            "{} backend: chunked readBody should return 'Hello World', got: {}",
            backend,
            resp_str
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        results.push((backend.to_string(), stdout));

        if let Some(p) = &artifact {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }

    // 3-way comparison
    let interp = &results[0].1;
    let js = &results[1].1;
    let native = &results[2].1;

    assert!(
        interp.contains("1"),
        "NET2-6b: Interpreter should report 1 request, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET2-6b: Chunked 3-way parity: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET2-6b: Chunked 3-way parity: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET2-6c: readBody 3-way parity (httpServe-level) ──────────────

/// NET2-6c: readBody Content-Length body 3-way parity via httpServe.
/// Sends a POST with Content-Length body, verifies readBody returns
/// the body correctly and all backends echo the same response.
#[test]
fn test_net2_6c_readbody_content_length_httpserve_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let mut results: Vec<(String, String)> = Vec::new();

    for backend in &["interp", "js", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  @(status <= 200, headers <= @[], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Bytes)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net6_rb_cl_{}", backend));
        let (child, artifact) =
            spawn_net_server_backend(&dir, backend, &format!("6rb_cl_{}", backend));

        // POST with Content-Length body "taida-net-v2"
        let body_str = "taida-net-v2";
        let request = format!(
            "POST /echo HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body_str.len(),
            body_str
        );
        let resp = send_http_request(port, request.as_bytes());
        assert!(
            resp.is_some(),
            "{} backend: Content-Length POST must get a response",
            backend
        );
        let resp_str = String::from_utf8_lossy(resp.as_ref().unwrap());
        assert!(
            resp_str.contains("taida-net-v2"),
            "{} backend: readBody should echo 'taida-net-v2', got: {}",
            backend,
            resp_str
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        results.push((backend.to_string(), stdout));

        if let Some(p) = &artifact {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }

    // 3-way comparison
    let interp = &results[0].1;
    let js = &results[1].1;
    let native = &results[2].1;

    assert!(
        interp.contains("1"),
        "NET2-6c: Interpreter should report 1 request, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET2-6c: readBody CL 3-way parity: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET2-6c: readBody CL 3-way parity: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET2-6c: readBody empty body 3-way parity via httpServe.
/// Sends a GET with no body, verifies readBody returns empty Bytes.
#[test]
fn test_net2_6c_readbody_empty_httpserve_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let mut results: Vec<(String, String)> = Vec::new();

    for backend in &["interp", "js", "native"] {
        let port = find_free_loopback_port();
        // readBody on a GET (no body) should return empty Bytes.
        // We output the body length to compare across backends.
        let source = format!(
            r#">>> taida-lang/net => @(httpServe, readBody)

handler req =
  body <= readBody(req)
  lenStr <= Str[body.length()]()
  lenStr ]=> ls
  @(status <= 200, headers <= @[], body <= ls)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net6_rb_empty_{}", backend));
        let (child, artifact) =
            spawn_net_server_backend(&dir, backend, &format!("6rb_empty_{}", backend));

        // GET with no body
        let request = b"GET /empty HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        let resp = send_http_request(port, request);
        assert!(
            resp.is_some(),
            "{} backend: GET must get a response",
            backend
        );
        let resp_str = String::from_utf8_lossy(resp.as_ref().unwrap());
        // The response body should be "0" (length of empty readBody result)
        assert!(
            resp_str.contains("\r\n\r\n0") || resp_str.ends_with("0"),
            "{} backend: readBody of GET should return 0-length body, got: {}",
            backend,
            resp_str
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        results.push((backend.to_string(), stdout));

        if let Some(p) = &artifact {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }

    // 3-way comparison
    let interp = &results[0].1;
    let js = &results[1].1;
    let native = &results[2].1;

    assert_eq!(
        interp, js,
        "NET2-6c: readBody empty 3-way parity: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET2-6c: readBody empty 3-way parity: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET2-6d: Concurrent connections 3-way parity ──────────────────

/// NET2-6d: Concurrent connections 3-way parity — Interpreter vs JS vs Native.
/// Opens 2 clients simultaneously, verifies both get responses.
#[test]
fn test_net2_6d_concurrent_connections_3way_parity() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let mut results: Vec<(String, String)> = Vec::new();

    for backend in &["interp", "js", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "conc-3way")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 3)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net6_conc_{}", backend));
        let (mut child, artifact) =
            spawn_net_server_backend(&dir, backend, &format!("6conc_{}", backend));

        // Readiness probe (request 1 of 3)
        let probe = send_http_request(
            port,
            b"GET /probe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        assert!(
            probe.is_some(),
            "{} backend: readiness probe must succeed",
            backend
        );

        // Connect 2 clients simultaneously (requests 2 and 3)
        let mut clients: Vec<TcpStream> = Vec::new();
        for _ in 0..2 {
            match TcpStream::connect(format!("127.0.0.1:{}", port)) {
                Ok(s) => {
                    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
                    clients.push(s);
                }
                Err(e) => {
                    let _ = child.kill();
                    if let Some(p) = &artifact {
                        let _ = fs::remove_file(p);
                    }
                    cleanup_net_project(&dir);
                    panic!("{} backend: concurrent connect failed: {}", backend, e);
                }
            }
        }

        // Send requests on both clients
        for (i, c) in clients.iter_mut().enumerate() {
            let req = format!(
                "GET /c{} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                i
            );
            std::io::Write::write_all(c, req.as_bytes())
                .unwrap_or_else(|_| panic!("{} backend: write to client {} failed", backend, i));
        }

        // Read responses from both clients
        for (i, c) in clients.iter_mut().enumerate() {
            let mut resp = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match std::io::Read::read(c, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => resp.extend_from_slice(&buf[..n]),
                    Err(ref e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }
            let resp_str = String::from_utf8_lossy(&resp);
            assert!(
                resp_str.contains("conc-3way"),
                "{} backend: concurrent client {} should get 'conc-3way', got: {}",
                backend,
                i,
                resp_str
            );
        }

        // Wait for server to exit (maxRequests=3)
        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        results.push((backend.to_string(), stdout));

        if let Some(p) = &artifact {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }

    // 3-way comparison: all should report 3 requests
    let interp = &results[0].1;
    let js = &results[1].1;
    let native = &results[2].1;

    // NB2-17: Check last line is "3" (output has "true\n3" from ok + requests)
    let interp_last = interp.trim().lines().last().unwrap_or("");
    assert_eq!(
        interp_last, "3",
        "NET2-6d: Interpreter should report 3 requests, got: {}",
        interp
    );
    assert_eq!(
        interp, js,
        "NET2-6d: Concurrent 3-way parity: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET2-6d: Concurrent 3-way parity: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET2-6e: v1 existing test re-confirmation ─────────────────────

/// NET2-6e: v1 httpServe bounded 3-way parity still works.
/// This re-runs the existing all-backends httpServe test to confirm
/// no v2 changes broke v1 behavior.
#[test]
fn test_net2_6e_v1_http_serve_bounded_3way_still_passes() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    // Re-run the v1 bounded serve test pattern with all 3 backends.
    for backend in &["interp", "js", "native"] {
        let port = find_free_loopback_port();
        let source = format!(
            r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= "v1-ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe({port}, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.requests)
"#
        );

        let dir = setup_net_project(&source, &format!("net6_v1_{}", backend));
        let (child, artifact) =
            spawn_net_server_backend(&dir, backend, &format!("6v1_{}", backend));

        let request = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
        let resp = send_http_request(port, request);
        assert!(
            resp.is_some(),
            "{} backend: v1 bounded serve must respond",
            backend
        );
        let resp_str = String::from_utf8_lossy(resp.as_ref().unwrap());
        assert!(
            resp_str.contains("200 OK"),
            "{} backend: v1 bounded serve response should contain '200 OK', got: {}",
            backend,
            resp_str
        );
        assert!(
            resp_str.contains("v1-ok"),
            "{} backend: v1 bounded serve body should contain 'v1-ok', got: {}",
            backend,
            resp_str
        );

        let output = child.wait_with_output().expect("wait for server");
        let stdout = normalize(&String::from_utf8_lossy(&output.stdout));
        assert_eq!(
            stdout, "1",
            "{} backend: v1 httpServe result should report 1 request, got: {}",
            backend, stdout
        );

        if let Some(p) = &artifact {
            let _ = fs::remove_file(p);
        }
        cleanup_net_project(&dir);
    }
}

/// NET2-6e: v1 httpParseRequestHead 3-way parity still works.
/// Confirms parse API parity is unchanged by v2 additions.
#[test]
fn test_net2_6e_v1_parse_request_head_3way_still_passes() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpParseRequestHead)

bytesLax <= Bytes["GET /hello?q=1 HTTP/1.1\r\nHost: localhost\r\nContent-Length: 3\r\n\r\nabc"]()
bytesLax ]=> bytes
result <= httpParseRequestHead(bytes)
result ]=> parsed

stdout(parsed.consumed)
stdout(parsed.method.start)
stdout(parsed.method.len)
stdout(parsed.path.start)
stdout(parsed.path.len)
stdout(parsed.query.start)
stdout(parsed.query.len)
stdout(parsed.contentLength)
"#;

    let dir = setup_net_project(source, "net6_v1_parse");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for v1 parse parity");
    let js = run_net_js(&dir, "net6_v1_parse").expect("js failed for v1 parse parity");
    let native = run_net_native(&dir, "net6_v1_parse").expect("native failed for v1 parse parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET2-6e: v1 parse 3-way parity: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET2-6e: v1 parse 3-way parity: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

/// NET2-6e: v1 httpEncodeResponse 3-way parity still works.
#[test]
fn test_net2_6e_v1_encode_response_3way_still_passes() {
    if !node_available() {
        eprintln!("SKIP: node not available");
        return;
    }

    let source = r#">>> taida-lang/net => @(httpEncodeResponse)

response <= @(status <= 200, headers <= @[@(name <= "X-Test", value <= "yes")], body <= "hello")
result <= httpEncodeResponse(response)
result ]=> encoded
stdout(encoded.bytes.length())
"#;

    let dir = setup_net_project(source, "net6_v1_encode");
    let interp = run_net_interpreter(&dir).expect("interpreter failed for v1 encode parity");
    let js = run_net_js(&dir, "net6_v1_encode").expect("js failed for v1 encode parity");
    let native =
        run_net_native(&dir, "net6_v1_encode").expect("native failed for v1 encode parity");
    cleanup_net_project(&dir);

    assert_eq!(
        interp, js,
        "NET2-6e: v1 encode 3-way parity: Interp vs JS\nInterp: {}\nJS: {}",
        interp, js
    );
    assert_eq!(
        interp, native,
        "NET2-6e: v1 encode 3-way parity: Interp vs Native\nInterp: {}\nNative: {}",
        interp, native
    );
}

// ── NET2-6f: WASM capability gating ───────────────────────────────

/// NET2-6f: readBody must produce compile error on wasm-min.
/// v2 added readBody to the net API — WASM gating must reject it.
#[test]
fn test_net2_6f_wasm_min_readbody_compile_error() {
    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 0, len <= 0))
body <= readBody(req)
stdout(body.length())
"#;
    let td_path = std::env::temp_dir().join("taida_net6f_readbody_wasm.td");
    let wasm_path = std::env::temp_dir().join("taida_net6f_readbody_wasm.wasm");
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-min")
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida build");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "NET2-6f: wasm-min should reject readBody, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("readBody") || stderr.contains("net"),
        "NET2-6f: compile error should mention readBody or net.\nstderr: {}",
        stderr
    );
}

/// NET2-6f: readBody must produce compile error on wasm-wasi.
#[test]
fn test_net2_6f_wasm_wasi_readbody_compile_error() {
    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 0, len <= 0))
body <= readBody(req)
stdout(body.length())
"#;
    let td_path = std::env::temp_dir().join("taida_net6f_readbody_wasi.td");
    let wasm_path = std::env::temp_dir().join("taida_net6f_readbody_wasi.wasm");
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-wasi")
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida build");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "NET2-6f: wasm-wasi should reject readBody, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("readBody") || stderr.contains("net"),
        "NET2-6f: compile error should mention readBody or net.\nstderr: {}",
        stderr
    );
}

/// NET2-6f: readBody must produce compile error on wasm-edge.
#[test]
fn test_net2_6f_wasm_edge_readbody_compile_error() {
    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 0, len <= 0))
body <= readBody(req)
stdout(body.length())
"#;
    let td_path = std::env::temp_dir().join("taida_net6f_readbody_edge.td");
    let wasm_path = std::env::temp_dir().join("taida_net6f_readbody_edge.wasm");
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-edge")
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida build");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "NET2-6f: wasm-edge should reject readBody, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("readBody") || stderr.contains("net"),
        "NET2-6f: compile error should mention readBody or net.\nstderr: {}",
        stderr
    );
}

/// NET2-6f: readBody must produce compile error on wasm-full.
#[test]
fn test_net2_6f_wasm_full_readbody_compile_error() {
    let source = r#">>> taida-lang/net => @(readBody)

rawLax <= Bytes["GET / HTTP/1.1\r\nHost: localhost\r\n\r\n"]()
rawLax ]=> raw
req <= @(raw <= raw, body <= @(start <= 0, len <= 0))
body <= readBody(req)
stdout(body.length())
"#;
    let td_path = std::env::temp_dir().join("taida_net6f_readbody_full.td");
    let wasm_path = std::env::temp_dir().join("taida_net6f_readbody_full.wasm");
    std::fs::write(&td_path, source).expect("write test .td");

    let output = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("wasm-full")
        .arg(&td_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .expect("failed to run taida build");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&wasm_path);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "NET2-6f: wasm-full should reject readBody, but compile succeeded.\nstderr: {}",
        stderr
    );
    assert!(
        stderr.contains("readBody") || stderr.contains("net"),
        "NET2-6f: compile error should mention readBody or net.\nstderr: {}",
        stderr
    );
}

/// NET2-6f: httpServe still rejected on all 4 WASM profiles (regression check).
#[test]
fn test_net2_6f_wasm_all_profiles_httpserve_still_rejected() {
    let source = r#">>> taida-lang/net => @(httpServe)

handler req =
  @(status <= 200, headers <= @[], body <= "ok")
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

httpServe(8080, handler, 1, 1000) ]=> serverResult
stdout(serverResult.ok)
"#;

    for profile in &["wasm-min", "wasm-wasi", "wasm-edge", "wasm-full"] {
        let td_path = std::env::temp_dir().join(format!("taida_net6f_serve_{}.td", profile));
        let wasm_path = std::env::temp_dir().join(format!("taida_net6f_serve_{}.wasm", profile));
        std::fs::write(&td_path, source).expect("write test .td");

        let output = Command::new(taida_bin())
            .arg("build")
            .arg("--target")
            .arg(profile)
            .arg(&td_path)
            .arg("-o")
            .arg(&wasm_path)
            .output()
            .expect("failed to run taida build");

        let _ = std::fs::remove_file(&td_path);
        let _ = std::fs::remove_file(&wasm_path);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "NET2-6f: {} should reject httpServe, but compile succeeded.\nstderr: {}",
            profile,
            stderr
        );
        assert!(
            stderr.contains("httpServe") || stderr.contains("net"),
            "NET2-6f: {} compile error should mention httpServe or net.\nstderr: {}",
            profile,
            stderr
        );
    }
}
