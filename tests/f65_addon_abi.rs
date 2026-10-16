//! Regressions for the addon ABI: values crossing the native dispatcher in
//! both directions, descriptor and status validation, the scaffold that
//! `taida init --target rust-addon` generates, and the WASM host-call
//! value encoding.

mod common;

use common::{taida_bin, unique_temp_dir, wasmtime_bin};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn taida_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(taida_bin())
        .args(args)
        .current_dir(dir)
        .env("HOME", dir.join(".home"))
        .output()
        .expect("run taida")
}

/// Run `main.td` in `project` on the interpreter and, when the native
/// backend is built in, as a native binary. Returns both outputs.
fn run_both(project: &Path) -> (Output, Option<Output>) {
    let interp = taida_in(project, &["main.td"]);
    if !cfg!(feature = "native") {
        return (interp, None);
    }
    let bin = project.join("main.bin");
    let build = taida_in(project, &["build", "native", "main.td", "-o", "main.bin"]);
    if !build.status.success() {
        return (interp, Some(build));
    }
    let native = Command::new(&bin)
        .current_dir(project)
        .output()
        .expect("run native binary");
    (interp, Some(native))
}

// ── A test addon written in C against the frozen ABI v1 layout ──────

/// ABI v1 declarations (byte-compatible with `crates/addon-rs/src/abi.rs`)
/// shared by the C test addons.
const ABI_V1_HEADER: &str = r#"
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
typedef struct V { uint32_t tag; uint32_t reserved; void *payload; } V;
typedef struct E { uint32_t code; uint32_t reserved; const char *message; } E;
typedef struct { int64_t value; } IntP;
typedef struct { double value; } FloatP;
typedef struct { uint8_t value; } BoolP;
typedef struct { const uint8_t *ptr; size_t len; } BytesP;
typedef struct { V **items; size_t len; } ListP;
typedef struct { const char *name; V *value; } Entry;
typedef struct { Entry *entries; size_t len; } PackP;
typedef struct Host Host;
struct Host {
    uint32_t abi_version, reserved;
    V *(*unit)(const Host *);
    V *(*int_)(const Host *, int64_t);
    V *(*float_)(const Host *, double);
    V *(*bool_)(const Host *, uint8_t);
    V *(*str)(const Host *, const uint8_t *, size_t);
    V *(*bytes)(const Host *, const uint8_t *, size_t);
    V *(*list)(const Host *, V *const *, size_t);
    V *(*pack)(const Host *, const char *const *, V *const *, size_t);
    void (*release)(const Host *, V *);
    E *(*error_new)(const Host *, uint32_t, const uint8_t *, size_t);
    void (*error_release)(const Host *, E *);
};
typedef uint32_t (*Call)(const V *, uint32_t, V **, E **);
typedef struct { const char *name; uint32_t arity; uint32_t pad; Call call; } Fn;
typedef struct {
    uint32_t abi_version, reserved;
    const char *addon_name;
    uint32_t function_count, reserved2;
    const Fn *functions;
    uint32_t (*init)(const Host *);
} Desc;
"#;

/// `describe(x)` renders the ABI kinds the addon received (`i` Int, `f`
/// Float, `b` Bool, `s` Str, `y<len>` Bytes, `[..]` List, `{name:..}`
/// Pack); `nested()` returns a pack holding every compound kind; `half(x)`
/// requires a Float; `bad_status()` returns a status outside the enum.
const PROBE_ADDON: &str = r#"
static const Host *H;
static uint32_t probe_init(const Host *h) { H = h; return 0; }
static void put(char *buf, size_t cap, const char *s) {
    size_t n = strlen(buf), k = strlen(s);
    if (n + k < cap) memcpy(buf + n, s, k + 1);
}
static void render(const V *v, char *buf, size_t cap) {
    char tmp[32];
    switch (v->tag) {
        case 0: put(buf, cap, "u"); break;
        case 1: put(buf, cap, "i"); break;
        case 2: put(buf, cap, "f"); break;
        case 3: put(buf, cap, "b"); break;
        case 4: put(buf, cap, "s"); break;
        case 5:
            snprintf(tmp, sizeof tmp, "y%zu", ((const BytesP *)v->payload)->len);
            put(buf, cap, tmp);
            break;
        case 6: {
            const ListP *l = (const ListP *)v->payload;
            put(buf, cap, "[");
            for (size_t i = 0; i < l->len; i++) {
                if (i) put(buf, cap, ",");
                render(l->items[i], buf, cap);
            }
            put(buf, cap, "]");
            break;
        }
        case 7: {
            const PackP *p = (const PackP *)v->payload;
            put(buf, cap, "{");
            for (size_t i = 0; i < p->len; i++) {
                if (i) put(buf, cap, ",");
                put(buf, cap, p->entries[i].name);
                put(buf, cap, ":");
                render(p->entries[i].value, buf, cap);
            }
            put(buf, cap, "}");
            break;
        }
        default: put(buf, cap, "?"); break;
    }
}
static uint32_t describe(const V *args, uint32_t n, V **out, E **err) {
    (void)err;
    if (n != 1) return 6;
    char buf[512] = "";
    render(&args[0], buf, sizeof buf);
    *out = H->str(H, (const uint8_t *)buf, strlen(buf));
    return 0;
}
static uint32_t nested(const V *args, uint32_t n, V **out, E **err) {
    (void)args; (void)n; (void)err;
    V *items[2] = { H->int_(H, 7), H->int_(H, 8) };
    const char *zn[1] = { "z" };
    V *zv[1] = { H->str(H, (const uint8_t *)"deep", 4) };
    const uint8_t raw[3] = { 1, 2, 3 };
    const char *names[5] = { "xs", "p", "b", "f", "t" };
    V *vals[5] = {
        H->list(H, items, 2), H->pack(H, zn, zv, 1), H->bytes(H, raw, 3),
        H->float_(H, 2.5), H->bool_(H, 1),
    };
    *out = H->pack(H, names, vals, 5);
    return 0;
}
static uint32_t half(const V *args, uint32_t n, V **out, E **err) {
    (void)err;
    if (n != 1 || args[0].tag != 2) return 4;
    *out = H->float_(H, ((const FloatP *)args[0].payload)->value / 2.0);
    return 0;
}
static uint32_t bad_status(const V *args, uint32_t n, V **out, E **err) {
    (void)args; (void)n; (void)out; (void)err;
    return 99;
}
static const Fn fns[] = {
    { "describe", 1, 0, describe },
    { "nested", 0, 0, nested },
    { "half", 1, 0, half },
    { "bad_status", 0, 0, bad_status },
};
static const Desc desc = { 1, 0, "me/probe", 4, 0, fns, probe_init };
const Desc *taida_addon_get_v1(void) { return &desc; }
"#;

const PROBE_MANIFEST: &str = "abi = 1\nentry = \"taida_addon_get_v1\"\npackage = \"me/probe\"\nlibrary = \"probe\"\n\n[functions]\ndescribe = 1\nnested = 0\nhalf = 1\nbad_status = 0\n";

/// Compile `body` (after the ABI header) into `dir/libprobe.so`. `None`
/// when no C compiler is available or the host is not Linux.
fn build_c_addon(dir: &Path, body: &str) -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let src = dir.join("probe.c");
    fs::write(&src, format!("{ABI_V1_HEADER}{body}")).unwrap();
    let lib = dir.join("libprobe.so");
    let out = Command::new("cc")
        .args(["-shared", "-fPIC", "-O1", "-o"])
        .arg(&lib)
        .arg(&src)
        .output()
        .ok()?;
    assert!(out.status.success(), "cc: {}", stderr_of(&out));
    Some(lib)
}

/// A project importing the C addon as `me/probe`, with `main.td` = `main`.
fn probe_project(prefix: &str, body: &str, manifest: &str, main: &str) -> Option<PathBuf> {
    let dir = unique_temp_dir(prefix);
    let lib = build_c_addon(&dir, body)?;
    let project = dir.join("app");
    let native = project.join(".taida/deps/me/probe/native");
    fs::create_dir_all(&native).unwrap();
    fs::copy(&lib, native.join("libprobe.so")).unwrap();
    fs::write(native.join("addon.toml"), manifest).unwrap();
    fs::write(project.join("packages.tdm"), "").unwrap();
    fs::write(project.join("main.td"), main).unwrap();
    Some(project)
}

fn assert_both_print(project: &Path, expected: &str) {
    let (interp, native) = run_both(project);
    assert!(interp.status.success(), "interp: {}", stderr_of(&interp));
    assert_eq!(stdout_of(&interp).trim_end(), expected, "interp");
    if let Some(native) = native {
        assert!(native.status.success(), "native: {}", stderr_of(&native));
        assert_eq!(stdout_of(&native).trim_end(), expected, "native");
    }
}

#[test]
fn addon_values_cross_the_boundary_with_their_kinds() {
    let Some(project) = probe_project(
        "f65_abi_kinds",
        PROBE_ADDON,
        PROBE_MANIFEST,
        r#">>> me/probe => @(describe, nested, half)

d1: Str <= describe(@[1, 2])
stdout(d1)
d2: Str <= describe(@(a <= 1.5, b <= "s"))
stdout(d2)
d3: Str <= describe(Utf8Encode["hi"]().unmold())
stdout(d3)
d4: Str <= describe(@[@[true], @[false]])
stdout(d4)
d5: Str <= describe(3)
stdout(d5)
d6: Str <= describe("x")
stdout(d6)
n <= nested()
stdout(n.xs)
stdout(n.p.z)
stdout(n.b)
x: Float <= half(5.0)
stdout(x)
stdout(x + 1.0)
"#,
    ) else {
        return;
    };
    assert_both_print(
        &project,
        "[i,i]\n{a:f,b:s}\ny2\n[[b],[b]]\ni\ns\n@[7, 8]\ndeep\nBytes[@[1, 2, 3]]\n2.5\n3.5",
    );
}

#[test]
fn an_untyped_addon_value_is_not_displayed_by_native_code() {
    // Native code cannot tell an Int from a Float or a Bool without a type,
    // so it rejects displaying, computing with or storing an addon return
    // value nothing types; the interpreter shows the value.
    for (tag, use_site) in [
        ("show", "x <= half(7.0)\nstdout(x)\n"),
        ("arith", "x <= half(7.0)\ny <= x\nstdout(y + 1.0)\n"),
        ("template", "x <= half(7.0)\nstdout(`v=${x}`)\n"),
        ("list", "xs <= @[half(7.0)]\nstdout(xs.length())\n"),
        ("direct", "stdout(half(7.0))\n"),
    ] {
        let Some(project) = probe_project(
            &format!("f65_abi_untyped_{tag}"),
            PROBE_ADDON,
            PROBE_MANIFEST,
            &format!(">>> me/probe => @(half)\n{use_site}"),
        ) else {
            return;
        };
        let (interp, native) = run_both(&project);
        assert!(interp.status.success(), "{tag}: {}", stderr_of(&interp));
        if let Some(native) = native {
            assert!(!native.status.success(), "{tag}: native build must fail");
            assert!(
                stderr_of(&native).contains("[E1414]"),
                "{tag}: {}",
                stderr_of(&native)
            );
        }
    }
}

#[test]
fn facade_values_reading_other_facade_values_are_made_after_them() {
    // `Op` reads `OpKind` and sorts before it: native code makes the
    // facade's values in the order they read each other, as the facade
    // itself runs them.
    let Some(project) = probe_project(
        "f65_abi_facade_value_order",
        PROBE_ADDON,
        PROBE_MANIFEST,
        ">>> me/probe => @(Op)\nstdout(Op.kind)\n",
    ) else {
        return;
    };
    let facade = project.join(".taida/deps/me/probe/taida");
    fs::create_dir_all(&facade).unwrap();
    fs::write(
        facade.join("kinds.td"),
        "OpKind <= @(\n  move <= 0\n  write <= 1\n)\nOp <= @(\n  kind <= OpKind.write\n)\n<<< @(OpKind, Op)\n",
    )
    .unwrap();
    fs::write(
        facade.join("probe.td"),
        ">>> ./kinds.td => @(OpKind, Op)\ndescribe <= RustAddon[\"describe\"](arity <= 1)\n<<< @(Op, OpKind, describe)\n",
    )
    .unwrap();
    assert_both_print(&project, "1");
}

#[test]
fn a_function_argument_is_rejected_not_passed_as_an_address() {
    let Some(project) = probe_project(
        "f65_abi_unsupported",
        PROBE_ADDON,
        PROBE_MANIFEST,
        r#">>> me/probe => @(describe)

f v: Int = v + 1 => :Int
|== e: Error =
  stdout(e.type)
  0
=> :Int
r: Str <= describe(f)
stdout(r)
"#,
    ) else {
        return;
    };
    assert_both_print(&project, "AddonUnsupportedInput");
}

#[test]
fn a_status_outside_the_enum_is_a_catchable_error() {
    let Some(project) = probe_project(
        "f65_abi_status",
        PROBE_ADDON,
        PROBE_MANIFEST,
        r#">>> me/probe => @(bad_status)

|== e: Error =
  stdout("caught")
  0
=> :Int
r: Int <= bad_status()
stdout(r)
"#,
    ) else {
        return;
    };
    assert_both_print(&project, "caught");
}

#[test]
fn a_facade_binds_addon_functions_explicitly_on_every_backend() {
    // A bare reference to a manifest function is `[E1413]` everywhere.
    let Some(project) = probe_project(
        "f65_abi_facade_bare",
        PROBE_ADDON,
        PROBE_MANIFEST,
        ">>> me/probe => @(Describe)\nd: Str <= Describe(3)\nstdout(d)\n",
    ) else {
        return;
    };
    let facade = project.join(".taida/deps/me/probe/taida");
    fs::create_dir_all(&facade).unwrap();
    fs::write(
        facade.join("probe.td"),
        "Describe <= describe\n<<< @(Describe)\n",
    )
    .unwrap();
    let (interp, native) = run_both(&project);
    assert!(!interp.status.success());
    assert!(
        stderr_of(&interp).contains("[E1413]"),
        "{}",
        stderr_of(&interp)
    );
    if let Some(native) = native {
        assert!(!native.status.success(), "native build must fail");
        assert!(
            stderr_of(&native).contains("[E1413]"),
            "{}",
            stderr_of(&native)
        );
    }

    // A name bound with `RustAddon[...]` in a sibling file and imported
    // may be bound again under another name.
    fs::write(
        facade.join("bind.td"),
        "describe <= RustAddon[\"describe\"](arity <= 1)\n<<< @(describe)\n",
    )
    .unwrap();
    fs::write(
        facade.join("probe.td"),
        ">>> ./bind.td => @(describe)\nDescribe <= describe\n<<< @(Describe)\n",
    )
    .unwrap();
    assert_both_print(&project, "i");
}

#[test]
fn a_package_id_mismatch_is_rejected_by_every_backend() {
    let manifest = PROBE_MANIFEST.replace("package = \"me/probe\"", "package = \"me/other\"");
    let Some(project) = probe_project(
        "f65_abi_pkg_id",
        PROBE_ADDON,
        &manifest,
        ">>> me/probe => @(describe)\nstdout(describe(1))\n",
    ) else {
        return;
    };
    let (interp, native) = run_both(&project);
    assert!(!interp.status.success());
    assert!(
        stderr_of(&interp).contains("package id mismatch"),
        "{}",
        stderr_of(&interp)
    );
    if let Some(native) = native {
        assert!(!native.status.success(), "native build must fail");
        assert!(
            stderr_of(&native).contains("package id mismatch"),
            "{}",
            stderr_of(&native)
        );
    }
}

#[test]
fn a_structurally_invalid_descriptor_fails_cleanly() {
    let body = r#"
static uint32_t one(const V *a, uint32_t n, V **o, E **e) { (void)a; (void)n; (void)o; (void)e; return 0; }
static const Desc desc = { 1, 0, "me/probe", 1, 0, NULL, NULL };
const Desc *taida_addon_get_v1(void) { (void)one; return &desc; }
"#;
    let manifest = "abi = 1\nentry = \"taida_addon_get_v1\"\npackage = \"me/probe\"\nlibrary = \"probe\"\n\n[functions]\none = 0\n";
    let Some(project) = probe_project(
        "f65_abi_descriptor",
        body,
        manifest,
        ">>> me/probe => @(one)\nr: Int <= one()\nstdout(r)\n",
    ) else {
        return;
    };
    let (interp, native) = run_both(&project);
    assert_eq!(interp.status.code(), Some(1), "{}", stderr_of(&interp));
    assert!(
        stderr_of(&interp).contains("invalid descriptor"),
        "{}",
        stderr_of(&interp)
    );
    if let Some(native) = native {
        assert_eq!(
            native.status.code(),
            Some(1),
            "no signal: {}",
            stderr_of(&native)
        );
        assert!(
            stderr_of(&native).contains("invalid descriptor"),
            "{}",
            stderr_of(&native)
        );
    }
}

// ── The `taida init --target rust-addon` scaffold works as generated ─

fn cargo_available() -> bool {
    Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn cargo(dir: &Path, target_dir: &Path, args: &[&str]) -> Output {
    Command::new("cargo")
        .args(args)
        .current_dir(dir)
        .env("CARGO_TARGET_DIR", target_dir)
        .env("CARGO_NET_OFFLINE", "true")
        .output()
        .expect("run cargo")
}

#[test]
fn rust_addon_scaffold_builds_lints_and_round_trips_values() {
    if !cargo_available() || !cfg!(target_os = "linux") {
        return;
    }
    let dir = unique_temp_dir("f65_scaffold_e2e");
    let init = taida_in(&dir, &["init", "--target", "rust-addon", "my-addon"]);
    assert!(init.status.success(), "{}", stderr_of(&init));
    let crate_dir = dir.join("my-addon");

    // Point the SDK dependency at this checkout and keep the crate out of
    // any workspace above the temp dir; nothing else is edited.
    let sdk = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crates/addon-rs");
    let cargo_toml = fs::read_to_string(crate_dir.join("Cargo.toml")).unwrap();
    let cargo_toml = cargo_toml.replace(
        "path = \"../taida/crates/addon-rs\"",
        &format!("path = \"{}\"", sdk.display()),
    );
    fs::write(
        crate_dir.join("Cargo.toml"),
        format!("{cargo_toml}\n[workspace]\n"),
    )
    .unwrap();

    let target_dir = dir.join("cargo-target");
    let build = cargo(
        &crate_dir,
        &target_dir,
        &["build", "--release", "--offline", "--quiet"],
    );
    assert!(build.status.success(), "cargo build: {}", stderr_of(&build));
    // The release workflow's `gate` job runs these on the generated crate.
    let fmt = cargo(&crate_dir, &target_dir, &["fmt", "--check"]);
    if fmt.status.code() != Some(101) || !stderr_of(&fmt).contains("no such command") {
        assert!(
            fmt.status.success(),
            "cargo fmt --check: {}{}",
            stdout_of(&fmt),
            stderr_of(&fmt)
        );
    }
    let clippy = cargo(
        &crate_dir,
        &target_dir,
        &[
            "clippy",
            "--release",
            "--offline",
            "--quiet",
            "--",
            "-D",
            "warnings",
        ],
    );
    if !stderr_of(&clippy).contains("no such command") {
        assert!(
            clippy.status.success(),
            "cargo clippy: {}",
            stderr_of(&clippy)
        );
    }

    // Install the package as generated (with the built library) and call
    // the facade name the scaffold exports.
    let manifest =
        taida::addon::manifest::parse_addon_manifest(&crate_dir.join("native/addon.toml")).unwrap();
    let app = dir.join("app");
    let pkg = app.join(".taida/deps").join(&manifest.package);
    fs::create_dir_all(pkg.join("native")).unwrap();
    fs::create_dir_all(pkg.join("taida")).unwrap();
    for rel in ["packages.tdm", "native/addon.toml", "taida/my-addon.td"] {
        fs::copy(crate_dir.join(rel), pkg.join(rel)).unwrap();
    }
    fs::copy(
        target_dir.join("release/libmy_addon.so"),
        pkg.join("native/libmy_addon.so"),
    )
    .unwrap();
    fs::write(app.join("packages.tdm"), "").unwrap();
    fs::write(
        app.join("main.td"),
        format!(
            ">>> {} => @(Echo)\ns: Str <= Echo(\"hello\")\nstdout(s)\nn: Int <= Echo(42)\nstdout(n)\nl: @[Int] <= Echo(@[1, 2, 3])\nstdout(l)\np: @(a: Int, b: Str) <= Echo(@(a <= 1, b <= \"x\"))\nstdout(p)\n",
            manifest.package
        ),
    )
    .unwrap();
    assert_both_print(&app, "hello\n42\n@[1, 2, 3]\n@(a <= 1, b <= \"x\")");
    let _ = fs::remove_dir_all(&dir);
}

// ── WASM: JSON string escapes and host-call error messages ──────────

fn run_wasm(profile: &str, dir: &Path, source: &str) -> Option<String> {
    let wasmtime = wasmtime_bin()?;
    let td = dir.join(format!("{profile}.td"));
    fs::write(&td, source).unwrap();
    let wasm = dir.join(format!("{profile}.wasm"));
    let build = Command::new(taida_bin())
        .args(["build", profile])
        .arg(&td)
        .arg("-o")
        .arg(&wasm)
        .output()
        .unwrap();
    assert!(build.status.success(), "{profile}: {}", stderr_of(&build));
    let run = Command::new(wasmtime).arg(&wasm).output().unwrap();
    assert!(run.status.success(), "{profile}: {}", stderr_of(&run));
    Some(stdout_of(&run).trim_end().to_string())
}

#[test]
fn wasm_json_strings_escape_control_characters_like_the_interpreter() {
    let dir = unique_temp_dir("f65_wasm_json_escape");
    let source = "stdout(jsonEncode(@(a <= \"a\\u{1}b\\u{8}c\\u{c}d\\u{1f}e\\u{7f}f\")))\n";
    let td = dir.join("main.td");
    fs::write(&td, source).unwrap();
    let interp = common::run_interpreter(&td).expect("interpreter run");
    assert_eq!(interp, "{\"a\":\"a\\u0001b\\bc\\fd\\u001fe\u{7f}f\"}");
    for profile in ["wasm-min", "wasm-wasi", "wasm-full"] {
        let Some(out) = run_wasm(profile, &dir, source) else {
            return;
        };
        assert_eq!(out, interp, "{profile}");
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn wasm_host_capability_error_message_is_text() {
    let dir = unique_temp_dir("f65_wasm_host_error");
    let source = r#">>> taida-lang/abi => @(HostCapability, HostStep, HostCall)

KV <= "cloudflare/kv"
c <= HostCapability["CACHE", KV]()
|== e: Error =
  stdout(e.message)
  0
=> :Int
r <=< Cage[c, HostCall[@[HostStep["get", @["k"]]()], Str]()]()
stdout(r)
"#;
    for profile in ["wasm-min", "wasm-wasi", "wasm-full"] {
        let Some(out) = run_wasm(profile, &dir, source) else {
            return;
        };
        assert_eq!(
            out, "host capabilities are not available outside a handler session",
            "{profile}"
        );
    }
    let _ = fs::remove_dir_all(&dir);
}
