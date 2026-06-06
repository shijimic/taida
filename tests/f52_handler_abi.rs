#![cfg(feature = "native")]

mod common;

use base64::Engine;
use common::{run_interpreter, taida_bin};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn compile_handler(target: &str, td_path: &Path, wasm_path: &Path) -> Option<String> {
    let output = Command::new(taida_bin())
        .args(["build", target, "--no-cache", "--handler", "handle"])
        .arg(td_path)
        .arg("-o")
        .arg(wasm_path)
        .output()
        .ok()?;

    if output.status.success() {
        None
    } else {
        Some(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[test]
fn f52_interpreter_response_helpers_construct_meaningful_values() {
    let source = r#">>> taida-lang/abi => @(text, json, bytes, status, header)
>>> taida-lang/crypto => @(sha256)

textResponse <= header("x-mode", "interp", status(202, text("hello")))
textMode <= textResponse.headers.get(1).getOrDefault(@(name <= "", value <= "")).value
stdout(textResponse.status.toString() + ":" + textMode + ":" + sha256(textResponse.body))

jsonResponse <= json(@(ok <= "yes"))
stdout(jsonResponse.headers.get(0).getOrDefault(@(name <= "", value <= "")).value)

Bytes["Hi"]() >=> raw
bytesResponse <= bytes(raw)
stdout(bytesResponse.headers.get(0).getOrDefault(@(name <= "", value <= "")).value + ":" + sha256(bytesResponse.body))

dupResponse <= header("x-mode", "second", header("x-mode", "first", status(99, text("dup"))))
dupFirst <= dupResponse.headers.get(1).getOrDefault(@(name <= "", value <= "")).value
dupSecond <= dupResponse.headers.get(2).getOrDefault(@(name <= "", value <= "")).value
stdout(dupResponse.status.toString() + ":" + dupFirst + ":" + dupSecond)

badHeader <= header("bad\r\nname", "value", text("bad"))
stdout(badHeader.status.toString() + ":" + badHeader.headers.get(0).getOrDefault(@(name <= "", value <= "")).value)
"#;

    let stem = format!("taida_f52_interp_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let _ = std::fs::remove_file(&td_path);
    std::fs::write(&td_path, source).expect("write interpreter F52 helper fixture");

    let out = run_interpreter(&td_path).expect("interpreter helper fixture should run");
    assert!(
        out.contains("202:interp:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"),
        "text/status/header helper output mismatch: {}",
        out
    );
    assert!(
        out.contains("application/json"),
        "json helper content-type missing: {}",
        out
    );
    assert!(
        out.contains(
            "application/octet-stream:3639efcd08abb273b1619e82e78c29a7df02c1051b1820e99fc395dcaa3326b8"
        ),
        "bytes helper output mismatch: {}",
        out
    );
    assert!(
        out.contains("100:first:second"),
        "status clamp or duplicate header append mismatch: {}",
        out
    );
    assert!(
        out.contains("500:abi"),
        "invalid header should materialize an ABI error response: {}",
        out
    );

    let _ = std::fs::remove_file(&td_path);
}

fn run_handler_with_node_response(
    wasm_path: &Path,
    label: &str,
    path: &str,
    body_base64: &str,
) -> Option<serde_json::Value> {
    let js_path = std::env::temp_dir().join(format!(
        "taida_f52_handler_{}_{}.js",
        label,
        std::process::id()
    ));
    let wasm_for_js = wasm_path.to_string_lossy();
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const noOpWasi = new Proxy({{}}, {{
    get(_target, prop) {{
      if (prop === "fd_write") {{
        return (_fd, _iovsPtr, _iovsLen, nwrittenPtr) => {{
          new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
          return 0;
        }};
      }}
      return () => 0;
    }},
  }});
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: noOpWasi,
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const wasm = fs.readFileSync("{wasm_for_js}");
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  if (typeof instance.exports.taida_abi_web_start !== "function") throw new Error("missing start export");
  if (typeof instance.exports.taida_abi_web_poll !== "function") throw new Error("missing poll export");
  if (typeof instance.exports.taida_abi_web_resume !== "function") throw new Error("missing resume export");
  const payload = encoder.encode(JSON.stringify({{
    method: "POST",
    path: "{path}",
    rawQuery: "q=search",
    query: [{{ name: "q", value: "search" }}],
    headers: [{{ name: "x-mode", value: "edge" }}],
    bodyBase64: "{body_base64}",
  }}));
  const inPtr = instance.exports.taida_abi_web_alloc(payload.length);
  if (!inPtr) throw new Error("alloc failed");
  new Uint8Array(memory.buffer, inPtr, payload.length).set(payload);
  const handle = instance.exports.taida_abi_web_start(inPtr, payload.length);
  const poll = instance.exports.taida_abi_web_poll(handle);
  if (poll !== 0) throw new Error("unexpected poll state " + poll);
  const outPtr = instance.exports.taida_abi_web_out_ptr(handle);
  const outLen = instance.exports.taida_abi_web_out_len(handle);
  const raw = decoder.decode(new Uint8Array(memory.buffer, outPtr, outLen));
  instance.exports.taida_abi_web_free(handle);
  const reusePtr = instance.exports.taida_abi_web_alloc(payload.length);
  const response = JSON.parse(raw);
  const inRangeUnissuedPtr = instance.exports.taida_abi_web_out_ptr(2n);
  const inRangeUnissuedLen = instance.exports.taida_abi_web_out_len(2n);
  const activeNeighborHandle = (1n << 16n) | 2n;
  const activeNeighborPtr = instance.exports.taida_abi_web_out_ptr(activeNeighborHandle);
  const activeNeighborLen = instance.exports.taida_abi_web_out_len(activeNeighborHandle);
  const forgedPtr = instance.exports.taida_abi_web_out_ptr(12345n);
  const forgedLen = instance.exports.taida_abi_web_out_len(12345n);
  const hugeAlloc = instance.exports.taida_abi_web_alloc(16777217);
  const floodHandles = [];
  for (let i = 0; i < 64; i++) {{
    const floodPtr = instance.exports.taida_abi_web_alloc(payload.length);
    if (!floodPtr) throw new Error("flood alloc failed");
    new Uint8Array(memory.buffer, floodPtr, payload.length).set(payload);
    floodHandles.push(instance.exports.taida_abi_web_handle(floodPtr, payload.length));
  }}
  const staleOverflowHandle = floodHandles[63];
  const overflowPtr = instance.exports.taida_abi_web_alloc(payload.length);
  if (!overflowPtr) throw new Error("overflow alloc failed");
  new Uint8Array(memory.buffer, overflowPtr, payload.length).set(payload);
  const overflowHandle = instance.exports.taida_abi_web_handle(overflowPtr, payload.length);
  const staleOverflowPtr = instance.exports.taida_abi_web_out_ptr(staleOverflowHandle);
  const staleOverflowLen = instance.exports.taida_abi_web_out_len(staleOverflowHandle);
  const overflowOutPtr = instance.exports.taida_abi_web_out_ptr(overflowHandle);
  const overflowOutLen = instance.exports.taida_abi_web_out_len(overflowHandle);
  instance.exports.taida_abi_web_free(overflowHandle);
  for (let i = 0; i < floodHandles.length; i++) {{
    instance.exports.taida_abi_web_free(floodHandles[i]);
  }}
  console.log(JSON.stringify({{
    response,
    inputPtr: inPtr,
    reusePtr,
    staleOverflowPtr,
    staleOverflowLen,
    overflowOutPtr,
    overflowOutLen,
    inRangeUnissuedPtr,
    inRangeUnissuedLen,
    activeNeighborPtr,
    activeNeighborLen,
    forgedPtr,
    forgedLen,
    hugeAlloc,
  }}));
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#,
        path = path,
        body_base64 = body_base64
    );
    std::fs::write(&js_path, script).ok()?;
    let output = Command::new("node").arg(&js_path).output().ok()?;
    let _ = std::fs::remove_file(&js_path);
    if !output.status.success() {
        eprintln!(
            "node handler host failed for {}: {}",
            label,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn run_handler_with_node_raw_request(
    wasm_path: &Path,
    label: &str,
    request_json: &str,
) -> Option<serde_json::Value> {
    let js_path = std::env::temp_dir().join(format!(
        "taida_f52_handler_raw_{}_{}.js",
        label,
        std::process::id()
    ));
    let wasm_for_js = wasm_path.to_string_lossy();
    let request_literal = serde_json::to_string(request_json).ok()?;
    let script = format!(
        r#"
const fs = require("fs");

(async () => {{
  let memory = new WebAssembly.Memory({{ initial: 2 }});
  const noOpWasi = new Proxy({{}}, {{
    get(_target, prop) {{
      if (prop === "fd_write") {{
        return (_fd, _iovsPtr, _iovsLen, nwrittenPtr) => {{
          new DataView(memory.buffer).setUint32(nwrittenPtr, 0, true);
          return 0;
        }};
      }}
      return () => 0;
    }},
  }});
  const imports = {{
    env: {{ memory }},
    wasi_snapshot_preview1: noOpWasi,
    taida_host: {{
      env_get() {{ return 0; }},
      env_get_all() {{ return 0; }},
    }},
  }};
  const wasm = fs.readFileSync("{wasm_for_js}");
  const {{ instance }} = await WebAssembly.instantiate(wasm, imports);
  if (instance.exports.memory) {{
    memory = instance.exports.memory;
  }}
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  if (typeof instance.exports.taida_abi_web_start !== "function") throw new Error("missing start export");
  if (typeof instance.exports.taida_abi_web_poll !== "function") throw new Error("missing poll export");
  if (typeof instance.exports.taida_abi_web_resume !== "function") throw new Error("missing resume export");
  const payload = encoder.encode({request_literal});
  const inPtr = instance.exports.taida_abi_web_alloc(payload.length);
  if (!inPtr) throw new Error("alloc failed");
  new Uint8Array(memory.buffer, inPtr, payload.length).set(payload);
  const handle = instance.exports.taida_abi_web_start(inPtr, payload.length);
  const poll = instance.exports.taida_abi_web_poll(handle);
  if (poll !== 0) throw new Error("unexpected poll state " + poll);
  const outPtr = instance.exports.taida_abi_web_out_ptr(handle);
  const outLen = instance.exports.taida_abi_web_out_len(handle);
  const raw = decoder.decode(new Uint8Array(memory.buffer, outPtr, outLen));
  instance.exports.taida_abi_web_free(handle);
  console.log(raw);
}})().catch((err) => {{
  console.error(err && err.stack ? err.stack : err);
  process.exit(1);
}});
"#
    );
    std::fs::write(&js_path, script).ok()?;
    let output = Command::new("node").arg(&js_path).output().ok()?;
    let _ = std::fs::remove_file(&js_path);
    if !output.status.success() {
        eprintln!(
            "node raw handler host failed for {}: {}",
            label,
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn run_handler_with_node(wasm_path: &Path, label: &str) -> Option<String> {
    let value = run_handler_with_node_response(wasm_path, label, "/f52", "cGF5bG9hZA==")?;
    assert_eq!(
        value["forgedPtr"].as_i64(),
        Some(0),
        "forged output handle must not expose memory"
    );
    assert_eq!(
        value["inRangeUnissuedPtr"].as_i64(),
        Some(0),
        "in-range unissued output handle must not expose memory"
    );
    assert_eq!(
        value["activeNeighborPtr"].as_i64(),
        Some(0),
        "active generation with unissued slot must not expose memory"
    );
    assert_eq!(
        value["forgedLen"].as_i64(),
        Some(0),
        "forged output handle length must be rejected"
    );
    assert_eq!(
        value["inRangeUnissuedLen"].as_i64(),
        Some(0),
        "in-range unissued output handle length must be rejected"
    );
    assert_eq!(
        value["activeNeighborLen"].as_i64(),
        Some(0),
        "active generation with unissued slot length must be rejected"
    );
    assert_eq!(
        value["hugeAlloc"].as_i64(),
        Some(0),
        "oversized wasm request allocation must fail"
    );
    assert_eq!(
        value["reusePtr"].as_i64(),
        value["inputPtr"].as_i64(),
        "free(handle) must rewind the request arena for persistent wasm instances"
    );
    assert_eq!(
        value["staleOverflowPtr"].as_i64(),
        Some(0),
        "overflow eviction must bump generation so stale handles cannot read the new response"
    );
    assert_eq!(
        value["staleOverflowLen"].as_i64(),
        Some(0),
        "overflow eviction must reject stale handle length reads"
    );
    assert!(
        value["overflowOutPtr"].as_i64().unwrap_or_default() > 0,
        "overflow fallback must still return a readable current handle"
    );
    assert!(
        value["overflowOutLen"].as_i64().unwrap_or_default() > 0,
        "overflow fallback must still return a current response length"
    );
    let body = value["response"]["bodyBase64"].as_str()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    String::from_utf8(bytes).ok()
}

fn run_handler_native(bin_path: &Path, payload: &[u8]) -> Option<String> {
    let mut child = Command::new(bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    {
        let stdin = child.stdin.as_mut()?;
        stdin.write_all(payload).ok()?;
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        eprintln!(
            "native handler host failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_handler_native_with_large_payload(bin_path: &Path, payload: &[u8]) -> Option<String> {
    let mut child = Command::new(bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    {
        let stdin = child.stdin.as_mut()?;
        match stdin.write_all(payload) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(_) => return None,
        }
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        eprintln!(
            "native handler host failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn response_json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw)
        .unwrap_or_else(|err| panic!("response must be valid JSON: {err}; raw={raw}"))
}

fn response_body_text(value: &serde_json::Value) -> String {
    let body = value["bodyBase64"].as_str().unwrap_or_default();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .expect("response body must decode");
    String::from_utf8(bytes).expect("response body must be utf-8")
}

fn response_header_values(value: &serde_json::Value, name: &str) -> Vec<String> {
    value["headers"]
        .as_array()
        .unwrap_or_else(|| panic!("response headers must be an array: {value}"))
        .iter()
        .filter_map(|header| {
            let header_name = header["name"].as_str()?;
            let header_value = header["value"].as_str()?;
            if header_name.eq_ignore_ascii_case(name) {
                Some(header_value.to_string())
            } else {
                None
            }
        })
        .collect()
}

fn response_header_value(value: &serde_json::Value, name: &str) -> Option<String> {
    response_header_values(value, name).into_iter().next()
}

#[test]
fn f52_wasm_profiles_share_handler_json_fixture() {
    if !node_available() {
        eprintln!("node not found, skipping F52 handler ABI smoke test");
        return;
    }

    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, bytes, header)

handle req: WebRequest =
  | req.path == "/bytes" |> bytes(req.body)
  | req.path == "/body-info" |> text(req.body.length().toString() + ":" + req.body.get(1).getOrDefault(0).toString())
  | req.path == "/pairs" |> text(req.rawQuery + ":" + req.query.get(0).getOrDefault(@(name <= "", value <= "")).value + "," + req.query.get(1).getOrDefault(@(name <= "", value <= "")).value + "," + req.query.get(2).getOrDefault(@(name <= "", value <= "")).value + ":" + req.headers.get(0).getOrDefault(@(name <= "", value <= "")).value + "," + req.headers.get(1).getOrDefault(@(name <= "", value <= "")).value)
  | req.path == "/cookies" |> header("set-cookie", "b=2", header("set-cookie", "a=1", text("cookies")))
  | req.path == "/bare-throw" |> throw("secret-token-xyz")
  | _ |> text(req.method + ":" + req.path + ":" + req.query.get(0).getOrDefault(@(name <= "", value <= "")).value + ":" + req.headers.get(0).getOrDefault(@(name <= "", value <= "")).value)
=> :WebResponse
"#;

    for target in ["wasm-min", "wasm-wasi", "wasm-full", "wasm-edge"] {
        let stem = format!(
            "taida_f52_{}_{}",
            target.replace('-', "_"),
            std::process::id()
        );
        let td_path = std::env::temp_dir().join(format!("{}.td", stem));
        let wasm_path: PathBuf = std::env::temp_dir().join(format!("{}.wasm", stem));
        let glue_path = std::env::temp_dir().join(format!("{}.edge.js", stem));

        let _ = std::fs::remove_file(&td_path);
        let _ = std::fs::remove_file(&wasm_path);
        let _ = std::fs::remove_file(&glue_path);
        std::fs::write(&td_path, source).expect("write F52 handler fixture");

        let err = compile_handler(target, &td_path, &wasm_path);
        assert!(
            err.is_none(),
            "{} handler build should compile: {:?}",
            target,
            err
        );
        let body = run_handler_with_node(&wasm_path, target)
            .unwrap_or_else(|| panic!("{} handler host should run", target));
        assert_eq!(body, "POST:/f52:search:edge", "target={}", target);
        let raw_body = [0x41u8, 0x00, 0x42, 0x00, 0x43];
        let body_b64 = base64::engine::general_purpose::STANDARD.encode(raw_body);
        let value = run_handler_with_node_response(&wasm_path, target, "/bytes", &body_b64)
            .unwrap_or_else(|| panic!("{} handler bytes host should run", target));
        assert_eq!(
            value["response"]["bodyBase64"].as_str(),
            Some(body_b64.as_str()),
            "target={} must preserve embedded NUL bytes",
            target
        );
        let value = run_handler_with_node_response(&wasm_path, target, "/body-info", "QQBD")
            .unwrap_or_else(|| panic!("{} handler body-info host should run", target));
        assert_eq!(
            response_body_text(&value["response"]),
            "3:0",
            "target={} must expose req.body length/get",
            target
        );
        let value = run_handler_with_node_raw_request(
            &wasm_path,
            target,
            r#"{"method":"POST","path":"/caf\u00e9","rawQuery":"q=%E6%9D%B1","query":[{"name":"q","value":"\u6771"}],"headers":[{"name":"x-mode","value":"\u30a8\u30c3\u30b8"}],"bodyBase64":""}"#,
        )
        .unwrap_or_else(|| panic!("{} handler unicode host should run", target));
        assert_eq!(
            response_body_text(&value),
            "POST:/café:東:エッジ",
            "target={} must decode JSON unicode escapes as UTF-8",
            target
        );
        let value = run_handler_with_node_raw_request(
            &wasm_path,
            target,
            r#"{"method":"GET","path":"/pairs","rawQuery":"tag=a&tag=b&tag=c","query":[{"name":"tag","value":"a"},{"name":"tag","value":"b"},{"name":"tag","value":"c"}],"headers":[{"name":"x-repeat","value":"a"},{"name":"x-repeat","value":"b"}],"bodyBase64":""}"#,
        )
        .unwrap_or_else(|| panic!("{} handler duplicate pair host should run", target));
        assert_eq!(
            response_body_text(&value),
            "tag=a&tag=b&tag=c:a,b,c:a,b",
            "target={} must preserve duplicate query/header pair order",
            target
        );
        let value = run_handler_with_node_response(&wasm_path, target, "/cookies", "")
            .unwrap_or_else(|| panic!("{} handler duplicate response headers should run", target));
        assert_eq!(
            response_header_values(&value["response"], "set-cookie"),
            vec!["a=1".to_string(), "b=2".to_string()],
            "target={} must preserve duplicate response header fields",
            target
        );
        let value = run_handler_with_node_response(&wasm_path, target, "/bare-throw", "")
            .unwrap_or_else(|| panic!("{} handler bare-throw host should run", target));
        assert_eq!(value["response"]["status"].as_i64(), Some(500));
        assert_eq!(
            response_body_text(&value["response"]),
            "handler throw",
            "target={} must convert bare throw to a fixed handler error response",
            target
        );

        let _ = std::fs::remove_file(&td_path);
        let _ = std::fs::remove_file(&wasm_path);
        let _ = std::fs::remove_file(&glue_path);
    }
}

#[test]
fn f52_native_handler_decodes_request_body_bytes() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, header)
>>> taida-lang/crypto => @(sha256)

handle req: WebRequest =
  | req.path == "/body-info" |> text(req.body.length().toString() + ":" + req.body.get(1).getOrDefault(0).toString())
  | req.path == "/pairs" |> text(req.rawQuery + ":" + req.query.get(0).getOrDefault(@(name <= "", value <= "")).value + "," + req.query.get(1).getOrDefault(@(name <= "", value <= "")).value + "," + req.query.get(2).getOrDefault(@(name <= "", value <= "")).value + ":" + req.headers.get(0).getOrDefault(@(name <= "", value <= "")).value + "," + req.headers.get(1).getOrDefault(@(name <= "", value <= "")).value)
  | req.path == "/cookies" |> header("set-cookie", "b=2", header("set-cookie", "a=1", text("cookies")))
  | _ |> text(req.method + ":" + req.path + ":" + req.query.get(0).getOrDefault(@(name <= "", value <= "")).value + ":" + req.headers.get(0).getOrDefault(@(name <= "", value <= "")).value + ":" + sha256(req.body))
=> :WebResponse
"#;

    let stem = format!("taida_f52_native_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let bin_path: PathBuf = std::env::temp_dir().join(stem);

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
    std::fs::write(&td_path, source).expect("write native F52 handler fixture");

    let err = compile_handler("native", &td_path, &bin_path);
    assert!(
        err.is_none(),
        "native handler build should compile: {:?}",
        err
    );
    let raw = run_handler_native(
        &bin_path,
        br#"{"method":"POST","path":"/f52","rawQuery":"q=search","query":[{"name":"q","value":"search"}],"headers":[{"name":"x-mode","value":"native"}],"bodyBase64":"cGF5bG9hZA=="}"#,
    )
    .expect("native handler should run");
    let json = response_json(&raw);
    assert_eq!(json["status"].as_i64(), Some(200));
    assert_eq!(
        json["bodyBase64"].as_str(),
        Some(
            "UE9TVDovZjUyOnNlYXJjaDpuYXRpdmU6MjM5ZjU5ZWQ1NWU3MzdjNzcxNDdjZjU1YWQwYzFiMDMwYjZkN2VlNzQ4YTc0MjY5NTJmOWI4NTJkNWE5MzVlNQ=="
        )
    );
    let raw = run_handler_native(
        &bin_path,
        br#"{"method":"POST","path":"/body-info","rawQuery":"","query":[],"headers":[],"bodyBase64":"QQBD"}"#,
    )
    .expect("native body-info handler should run");
    let json = response_json(&raw);
    assert_eq!(response_body_text(&json), "3:0");

    let raw = run_handler_native(
        &bin_path,
        br#"{"method":"GET","path":"/pairs","rawQuery":"tag=a&tag=b&tag=c","query":[{"name":"tag","value":"a"},{"name":"tag","value":"b"},{"name":"tag","value":"c"}],"headers":[{"name":"x-repeat","value":"a"},{"name":"x-repeat","value":"b"}],"bodyBase64":""}"#,
    )
    .expect("native pair-list handler should run");
    let json = response_json(&raw);
    assert_eq!(
        response_body_text(&json),
        "tag=a&tag=b&tag=c:a,b,c:a,b",
        "native bridge must preserve duplicate query/header pair order"
    );

    let raw = run_handler_native(
        &bin_path,
        br#"{"method":"GET","path":"/cookies","rawQuery":"","query":[],"headers":[],"bodyBase64":""}"#,
    )
    .expect("native duplicate response header handler should run");
    let json = response_json(&raw);
    assert_eq!(
        response_header_values(&json, "set-cookie"),
        vec!["a=1".to_string(), "b=2".to_string()],
        "native bridge must preserve duplicate response header fields"
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
}

#[test]
fn f52_native_handler_decodes_unicode_escapes() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: WebRequest = text(req.path + ":" + req.query.get(0).getOrDefault(@(name <= "", value <= "")).value + ":" + req.headers.get(0).getOrDefault(@(name <= "", value <= "")).value) => :WebResponse
"#;

    let stem = format!("taida_f52_native_unicode_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let bin_path: PathBuf = std::env::temp_dir().join(stem);

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
    std::fs::write(&td_path, source).expect("write native unicode F52 handler fixture");

    let err = compile_handler("native", &td_path, &bin_path);
    assert!(
        err.is_none(),
        "native handler build should compile: {:?}",
        err
    );
    let raw = run_handler_native(
        &bin_path,
        br#"{"method":"POST","path":"/caf\u00e9","rawQuery":"q=%E6%9D%B1","query":[{"name":"q","value":"\u6771"}],"headers":[{"name":"x-mode","value":"\u30a8\u30c3\u30b8"}],"bodyBase64":""}"#,
    )
    .expect("native unicode handler should run");
    let json = response_json(&raw);
    assert_eq!(response_body_text(&json), "/café:東:エッジ");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
}

#[test]
fn f52_native_handler_throw_stdout_and_header_edges_return_json() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, status, header)

debugOk =
  wrote <= stdout("debug line")
  text("ok")
=> :WebResponse

handle req: WebRequest =
  | req.path == "/throw" |> Error(type <= "Error", message <= "secret-token-xyz").throw()
  | req.path == "/bare-throw" |> throw("secret-token-xyz")
  | req.path == "/status" |> status(999, text("status"))
  | req.path == "/header" |> header("bad\r\nname", "value", text("bad"))
  | _ |> debugOk()
=> :WebResponse
"#;

    let stem = format!("taida_f52_native_edges_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let bin_path: PathBuf = std::env::temp_dir().join(stem);

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
    std::fs::write(&td_path, source).expect("write native edge F52 handler fixture");

    let err = compile_handler("native", &td_path, &bin_path);
    assert!(
        err.is_none(),
        "native handler build should compile: {:?}",
        err
    );

    let stdout_raw = run_handler_native(
        &bin_path,
        br#"{"method":"GET","path":"/stdout","rawQuery":"","query":[],"headers":[],"bodyBase64":""}"#,
    )
    .expect("native stdout handler should run");
    let stdout_json = response_json(&stdout_raw);
    assert_eq!(stdout_json["status"].as_i64(), Some(200));
    assert_eq!(stdout_json["bodyBase64"].as_str(), Some("b2s="));

    let throw_raw = run_handler_native(
        &bin_path,
        br#"{"method":"GET","path":"/throw","rawQuery":"","query":[],"headers":[],"bodyBase64":""}"#,
    )
    .expect("native throw handler should run");
    let throw_json = response_json(&throw_raw);
    assert_eq!(throw_json["status"].as_i64(), Some(500));
    assert_eq!(
        response_header_value(&throw_json, "x-taida-error").as_deref(),
        Some("handler-throw")
    );
    let throw_body = base64::engine::general_purpose::STANDARD
        .decode(throw_json["bodyBase64"].as_str().unwrap_or_default())
        .expect("throw response body must decode");
    let throw_body = String::from_utf8(throw_body).expect("throw response body must be utf-8");
    assert_eq!(throw_body, "handler throw");
    assert!(
        !throw_body.contains("secret-token-xyz"),
        "native handler throw response must not leak handler-supplied error message"
    );

    let bare_throw_raw = run_handler_native(
        &bin_path,
        br#"{"method":"GET","path":"/bare-throw","rawQuery":"","query":[],"headers":[],"bodyBase64":""}"#,
    )
    .expect("native bare throw handler should run");
    let bare_throw_json = response_json(&bare_throw_raw);
    assert_eq!(bare_throw_json["status"].as_i64(), Some(500));
    assert_eq!(
        response_header_value(&bare_throw_json, "x-taida-error").as_deref(),
        Some("handler-throw")
    );
    assert_eq!(response_body_text(&bare_throw_json), "handler throw");

    let status_raw = run_handler_native(
        &bin_path,
        br#"{"method":"GET","path":"/status","rawQuery":"","query":[],"headers":[],"bodyBase64":""}"#,
    )
    .expect("native status handler should run");
    assert_eq!(response_json(&status_raw)["status"].as_i64(), Some(599));

    let header_raw = run_handler_native(
        &bin_path,
        br#"{"method":"GET","path":"/header","rawQuery":"","query":[],"headers":[],"bodyBase64":""}"#,
    )
    .expect("native invalid-header handler should run");
    let header_json = response_json(&header_raw);
    assert_eq!(header_json["status"].as_i64(), Some(500));
    assert_eq!(
        response_header_value(&header_json, "x-taida-error").as_deref(),
        Some("abi")
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
}

#[test]
fn f52_native_handler_malformed_request_uses_default_shape() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)
>>> taida-lang/crypto => @(sha256)

handle req: WebRequest = text(req.method + ":" + req.path + ":" + sha256(req.body)) => :WebResponse
"#;

    let stem = format!("taida_f52_native_malformed_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let bin_path: PathBuf = std::env::temp_dir().join(stem);

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
    std::fs::write(&td_path, source).expect("write native malformed F52 handler fixture");

    let err = compile_handler("native", &td_path, &bin_path);
    assert!(
        err.is_none(),
        "native handler build should compile: {:?}",
        err
    );
    let raw = run_handler_native(&bin_path, b"not json").expect("native handler should run");
    assert!(
        raw.contains(
            "\"bodyBase64\":\"R0VUOi86ZTNiMGM0NDI5OGZjMWMxNDlhZmJmNGM4OTk2ZmI5MjQyN2FlNDFlNDY0OWI5MzRjYTQ5NTk5MWI3ODUyYjg1NQ==\""
        ),
        "malformed native request should use default method/path/body: {}",
        raw
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
}

#[test]
fn f52_native_handler_oversized_request_returns_413_json() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse, text)

handle req: WebRequest = text(req.method + ":" + req.path) => :WebResponse
"#;

    let stem = format!("taida_f52_native_oversized_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let bin_path: PathBuf = std::env::temp_dir().join(stem);

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
    std::fs::write(&td_path, source).expect("write native oversized F52 handler fixture");

    let err = compile_handler("native", &td_path, &bin_path);
    assert!(
        err.is_none(),
        "native handler build should compile: {:?}",
        err
    );

    let payload = vec![b'{'; 16 * 1024 * 1024 + 1];
    let raw = run_handler_native_with_large_payload(&bin_path, &payload)
        .expect("oversized native request should return structured JSON");
    let json = response_json(&raw);
    assert_eq!(json["status"].as_i64(), Some(413));
    assert_eq!(
        response_header_value(&json, "x-taida-error").as_deref(),
        Some("abi")
    );

    let body = base64::engine::general_purpose::STANDARD
        .decode(json["bodyBase64"].as_str().unwrap_or_default())
        .expect("oversized response body must decode");
    let body = String::from_utf8(body).expect("oversized response body must be utf-8");
    assert_eq!(body, "request too large");

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
}

#[test]
fn f52_handler_mode_still_checks_surface_under_no_check() {
    let source = r#">>> taida-lang/abi => @(WebRequest, WebResponse)

handle req: WebRequest = @() => :WebResponse
"#;

    let stem = format!("taida_f52_no_check_handler_{}", std::process::id());
    let td_path = std::env::temp_dir().join(format!("{}.td", stem));
    let bin_path: PathBuf = std::env::temp_dir().join(stem);

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
    std::fs::write(&td_path, source).expect("write no-check F52 handler fixture");

    let output = Command::new(taida_bin())
        .args(["--no-check", "build", "native", "--handler", "handle"])
        .arg(&td_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .expect("run no-check native handler build");
    assert!(
        !output.status.success(),
        "--no-check handler build must still reject non-WebResponse body"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[E1601]") && stderr.contains("WebResponse"),
        "handler type surface should remain checked under --no-check: {}",
        stderr
    );

    let _ = std::fs::remove_file(&td_path);
    let _ = std::fs::remove_file(&bin_path);
}
