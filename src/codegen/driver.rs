/// コンパイルドライバ — .td → パース → IR → CLIF → .o → バイナリ
///
/// cranelift-object で .o を出力し、システムリンカでバイナリを生成。
/// wasm-min ターゲット時は wasm-ld で .wasm を生成する。
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};

use super::emit::Emitter;
use super::emit_wasm_c;
use super::lower::Lowering;
use super::rc_opt;
use crate::module_graph;
use crate::parser::parse;

type ModuleImports = Vec<(String, Vec<String>, Option<String>)>;

/// L-1: Shared clang flags used for both cache key computation and compilation.
/// Keeping them in one place prevents cache_key / wasm_clang_base_args drift.
const WASM_CLANG_FLAGS: &[&str] = &["--target=wasm32-unknown-wasi", "-nostdlib", "-O2", "-c"];

/// Process-wide counter to produce unique .o file names.
static OBJ_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Return a unique .o path derived from `input_path` by appending PID and a
/// monotonic counter.  This prevents races when two threads (or two processes)
/// compile the same .td file concurrently.
///
/// N-41: `unwrap_or` usage in driver.rs
/// Throughout this module, `.parent().unwrap_or(Path::new("."))` and similar
/// fallbacks are used intentionally.  `Path::parent()` returns `None` only for
/// root-less single-component paths (e.g. `Path::new("file.td")`), so falling
/// back to `"."` (the current working directory) is the correct behaviour —
/// it matches the user's expectation that output artifacts land next to the
/// source file.  The same reasoning applies to `.file_stem()` / `.to_str()`
/// fallbacks elsewhere in this file.
fn unique_obj_path(input_path: &Path) -> PathBuf {
    let pid = std::process::id();
    let seq = OBJ_COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("taida_obj");
    let dir = input_path.parent().unwrap_or(Path::new(".")); // see doc above
    dir.join(format!("{}.{}.{}.o", stem, pid, seq))
}

#[derive(Debug)]
pub struct CompileError {
    pub message: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Output of wasm-edge compilation: .wasm binary + JS glue for Workers deployment.
#[derive(Debug)]
pub struct WasmEdgeOutput {
    pub wasm_path: PathBuf,
    pub glue_path: PathBuf,
}

// ---------------------------------------------------------------------------
// RC-8a/8d: WASM runtime .o cache
// ---------------------------------------------------------------------------

/// Filesystem-based cache for pre-compiled WASM runtime .o files.
///
/// The WASM compilation pipeline compiles C runtime sources to .o files
/// via clang on every invocation. Since the runtime sources are embedded
/// via `include_str!` and never change between invocations, caching the
/// .o files eliminates the most expensive step of WASM compilation.
///
/// Cache key: FNV-1a of (runtime_source + clang_version_string + clang_flags).
/// Cache location:
///   - Tests: `target/wasm-rt-cache/`
///   - Production: `.taida/cache/wasm-rt/`
///   - Override: `TAIDA_WASM_RT_CACHE` environment variable
pub struct WasmRuntimeCache {
    cache_dir: PathBuf,
    clang: String,
    clang_version: String,
    include_dir: PathBuf,
}

impl WasmRuntimeCache {
    /// Create a new runtime cache. `cache_dir` will be created if it does not exist.
    ///
    /// S-3: If clang version cannot be determined ("unknown"), the cache is
    /// effectively disabled — every invocation produces a different key because
    /// "unknown" is a degenerate version string.  A warning is emitted to stderr.
    pub fn new(cache_dir: PathBuf) -> Result<Self, CompileError> {
        fs::create_dir_all(&cache_dir).map_err(|e| CompileError {
            message: format!(
                "failed to create wasm runtime cache dir '{}': {}",
                cache_dir.display(),
                e
            ),
        })?;

        let clang = find_clang_for_wasm()?;
        let clang_version = get_clang_version(&clang);

        // S-3: Warn when clang version is unknown — cache keys will be
        // unreliable across invocations.
        if clang_version == "unknown" {
            eprintln!(
                "warning: could not determine clang version; WASM runtime cache may not work correctly"
            );
        }

        // Create a persistent include directory inside the cache dir
        let include_dir = cache_dir.join("include");
        write_wasm_stdint_header(&include_dir)?;

        Ok(Self {
            cache_dir,
            clang,
            clang_version,
            include_dir,
        })
    }

    /// Compute a cache key for the given runtime source + toolchain.
    ///
    /// N-1: Uses FNV-1a (matching the project convention) instead of
    /// `std::hash::DefaultHasher` whose algorithm is not guaranteed
    /// stable across Rust versions.
    fn cache_key(&self, source: &str) -> String {
        let mut state: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
        for byte in source.bytes() {
            state ^= byte as u64;
            state = state.wrapping_mul(0x100000001b3); // FNV prime
        }
        // Mix in clang version
        for byte in self.clang_version.bytes() {
            state ^= byte as u64;
            state = state.wrapping_mul(0x100000001b3);
        }
        // Mix in clang flags that affect output (L-1: shared via WASM_CLANG_FLAGS)
        for flag in WASM_CLANG_FLAGS {
            for byte in flag.bytes() {
                state ^= byte as u64;
                state = state.wrapping_mul(0x100000001b3);
            }
        }
        format!("{:016x}", state)
    }

    /// Get a cached .o file or compile the runtime source and cache the result.
    ///
    /// N-3: When a new cache entry is created, stale .o files for the same
    /// runtime `name` but with a different key are automatically removed.
    fn get_or_compile(&self, name: &str, source: &str) -> Result<PathBuf, CompileError> {
        let key = self.cache_key(source);
        let cached_obj = self.cache_dir.join(format!("{}.{}.o", name, key));

        if cached_obj.exists() {
            return Ok(cached_obj);
        }

        // Compile to a temporary file, then rename atomically.
        // Use PID + counter to avoid collisions between parallel processes/threads.
        let pid = std::process::id();
        let seq = OBJ_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_c = self
            .cache_dir
            .join(format!("{}.{}.{}.{}.tmp.c", name, key, pid, seq));
        let tmp_o = self
            .cache_dir
            .join(format!("{}.{}.{}.{}.tmp.o", name, key, pid, seq));

        fs::write(&tmp_c, source).map_err(|e| CompileError {
            message: format!("failed to write runtime source to cache: {}", e),
        })?;

        let clang_args = wasm_clang_base_args(&self.include_dir);
        let status = run_wasm_clang_object(
            &self.clang,
            &clang_args,
            &tmp_c,
            &tmp_o,
            &self.include_dir,
            &[],
            false,
            None,
        )?;

        let _ = fs::remove_file(&tmp_c);

        if !status.success() {
            let _ = fs::remove_file(&tmp_o);
            return Err(CompileError {
                message: format!(
                    "clang wasm32 compilation of {} runtime failed (cache build).",
                    name
                ),
            });
        }

        // N-4: Atomic rename to final cached path.
        // On POSIX systems, `rename(2)` is atomic when source and destination
        // are on the same filesystem (guaranteed here since both are inside
        // `cache_dir`).  This ensures that concurrent processes never observe
        // a partially-written .o file.  On Windows, `std::fs::rename` uses
        // `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` which is not strictly
        // atomic, but is safe enough for our use case (worst case: a redundant
        // recompile on the next invocation).
        fs::rename(&tmp_o, &cached_obj).map_err(|e| CompileError {
            message: format!("failed to rename cached runtime object: {}", e),
        })?;

        // N-3: Clean up stale cache entries for the same runtime name.
        // Pattern: `{name}.{old_key}.o` where old_key != current key.
        let stale_prefix = format!("{}.", name);
        if let Ok(entries) = fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name();
                let fname_str = fname.to_string_lossy();
                if fname_str.starts_with(&stale_prefix)
                    && fname_str.ends_with(".o")
                    && !fname_str.contains(".tmp.")
                    && entry.path() != cached_obj
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }

        Ok(cached_obj)
    }

    /// Get or compile the core runtime .o
    pub fn rt_core(&self) -> Result<PathBuf, CompileError> {
        let source = include_str!("runtime_core_wasm.c");
        self.get_or_compile("rt_core", source)
    }

    /// Get or compile the WASI I/O runtime .o
    pub fn rt_wasi(&self) -> Result<PathBuf, CompileError> {
        let source = include_str!("runtime_wasi_io.c");
        self.get_or_compile("rt_wasi", source)
    }

    /// Get or compile the edge host runtime .o
    pub fn rt_edge(&self) -> Result<PathBuf, CompileError> {
        let source = include_str!("runtime_edge_host.c");
        self.get_or_compile("rt_edge", source)
    }

    /// Get or compile the full runtime .o
    pub fn rt_full(&self) -> Result<PathBuf, CompileError> {
        let source = include_str!("runtime_full_wasm.c");
        self.get_or_compile("rt_full", source)
    }

    /// Return the clang path discovered during cache init.
    pub fn clang(&self) -> &str {
        &self.clang
    }

    /// Return the include directory (contains stdint.h shim).
    pub fn include_dir(&self) -> &Path {
        &self.include_dir
    }

    /// S-1: Shared helper — compile generated C and link with cached runtime .o files.
    ///
    /// This eliminates ~240 lines of near-identical code across the four cached
    /// compilation branches (wasm-min, wasm-wasi, wasm-edge, wasm-full).
    ///
    /// `rt_objs`: pre-compiled runtime .o files from the cache (e.g. `[rt_core.o]`
    ///            or `[rt_core.o, rt_wasi.o, rt_full.o]`).
    /// `generated_c`: the C source emitted from the IR.
    /// `wasm_path`: final output .wasm path.
    /// `tmp_suffix`: suffix for temp files to avoid collisions between profiles
    ///               (e.g. `"_wasm_tmp"`, `"_wasm_wasi_tmp"`).
    /// `extra_ld_args`: additional wasm-ld flags for profile-specific linking
    ///                  (e.g. `--export=memory` for edge profile).
    fn link_wasm_cached(
        &self,
        rt_objs: &[PathBuf],
        generated_c: &str,
        wasm_path: &Path,
        tmp_suffix: &str,
        extra_ld_args: &[&str],
    ) -> Result<(), CompileError> {
        let tmp_base = wasm_path.with_extension(tmp_suffix);
        let gen_c_path = tmp_base.with_extension("gen.c");
        let gen_obj_path = tmp_base.with_extension("gen.o");

        fs::write(&gen_c_path, generated_c).map_err(|e| CompileError {
            message: format!("failed to write generated C: {}", e),
        })?;

        let clang_args = wasm_clang_base_args(self.include_dir());
        let gen_status = run_wasm_clang_object(
            self.clang(),
            &clang_args,
            &gen_c_path,
            &gen_obj_path,
            self.include_dir(),
            &[],
            false,
            Some(&gen_c_path),
        )?;

        if !gen_status.success() {
            return Err(CompileError {
                message: format!(
                    "clang wasm32 compilation of generated code failed (source preserved at: {}; shim preserved at: {})",
                    gen_c_path.display(),
                    self.include_dir().display()
                ),
            });
        }

        let _ = fs::remove_file(&gen_c_path);

        let wasm_ld = find_wasm_ld()?;
        let mut cmd = Command::new(&wasm_ld);
        cmd.args([
            "--no-entry",
            "--export=_start",
            "--strip-all",
            "--gc-sections",
        ]);
        cmd.args(extra_ld_args);
        for rt_obj in rt_objs {
            cmd.arg(rt_obj);
        }
        cmd.arg(&gen_obj_path).arg("-o").arg(wasm_path);

        let ld_status = cmd.status().map_err(|e| CompileError {
            message: format!("wasm-ld invocation failed: {}", e),
        })?;

        let _ = fs::remove_file(&gen_obj_path);

        if !ld_status.success() {
            return Err(CompileError {
                message: format!("wasm-ld failed with exit code: {:?}", ld_status.code()),
            });
        }

        Ok(())
    }
}

/// Get the clang version string for cache key computation.
///
/// S-3: If the version cannot be determined, returns "unknown" which
/// causes cache invalidation (see `WasmRuntimeCache::new`).
fn get_clang_version(clang: &str) -> String {
    Command::new(clang)
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Resolve the default cache directory for WASM runtime objects.
///
/// Priority:
/// 1. `TAIDA_WASM_RT_CACHE` environment variable
/// 2. `.taida/cache/wasm-rt/` relative to the project root (if `.taida/` exists)
/// 3. `target/wasm-rt-cache/` as fallback
pub fn default_wasm_cache_dir(project_dir: Option<&Path>) -> PathBuf {
    if let Ok(env_dir) = std::env::var("TAIDA_WASM_RT_CACHE") {
        return PathBuf::from(env_dir);
    }

    if let Some(dir) = project_dir {
        // RCB-56: Walk up parent directories to find the Taida project root.
        // A project root is identified by `.taida/` + `packages.tdm` co-existing.
        // `.taida/` alone is not sufficient — it could be user config (~/.taida/)
        // or an unrelated ancestor directory (/tmp/.taida/).
        let mut current = dir.to_path_buf();
        loop {
            if current.join(".taida").exists() && current.join("packages.tdm").exists() {
                return current.join(".taida").join("cache").join("wasm-rt");
            }
            if !current.pop() {
                break;
            }
        }
    }

    // Fallback: target/wasm-rt-cache/ (works for both tests and standalone)
    PathBuf::from("target/wasm-rt-cache")
}

/// 単一 .td ファイルを Native .o にコンパイル（リンクなし）
fn compile_to_object(input_path: &Path) -> Result<(PathBuf, ModuleImports), CompileError> {
    let source = fs::read_to_string(input_path).map_err(|e| CompileError {
        message: format!("failed to read '{}': {}", input_path.display(), e),
    })?;

    let (program, parse_errors) = parse(&source);
    if !parse_errors.is_empty() {
        let msgs: Vec<String> = parse_errors.iter().map(|e| format!("{}", e)).collect();
        return Err(CompileError {
            message: format!("parse errors:\n{}", msgs.join("\n")),
        });
    }

    let mut lowering = Lowering::new();
    // QF-16/17: ソースディレクトリを設定（モジュールインポート解決用）
    if let Some(parent) = input_path.parent() {
        lowering.set_source_dir(parent.to_path_buf());
    }
    lowering.set_module_key(Lowering::module_key_for_path(input_path));
    let mut ir_module = lowering.lower_program(&program).map_err(|e| CompileError {
        message: format!("{}", e),
    })?;

    // RC 最適化パス: 不要な Retain/Release を除去
    rc_opt::optimize(&mut ir_module);

    let imports = ir_module.imports.clone();

    let mut emitter = Emitter::new().map_err(|e| CompileError {
        message: format!("{}", e),
    })?;
    emitter.emit_module(&ir_module).map_err(|e| CompileError {
        message: format!("{}", e),
    })?;

    let product = emitter.module.finish();
    let obj_bytes = product.emit().map_err(|e| CompileError {
        message: format!("object emission failed: {}", e),
    })?;

    let obj_path = unique_obj_path(input_path);
    fs::write(&obj_path, &obj_bytes).map_err(|e| CompileError {
        message: format!("failed to write '{}': {}", obj_path.display(), e),
    })?;

    Ok((obj_path, imports))
}

/// .td ファイルをコンパイルしてネイティブバイナリを生成
pub fn compile_file(
    input_path: &Path,
    output_path: Option<&Path>,
) -> Result<PathBuf, CompileError> {
    module_graph::detect_local_import_cycle(input_path).map_err(|e| CompileError {
        message: e.to_string(),
    })?;

    let base_dir = input_path.parent().unwrap_or(Path::new("."));

    // メインファイルをコンパイル
    let (main_obj, imports) = compile_to_object(input_path)?;

    // 依存モジュールを再帰的にコンパイル
    let mut all_objs = vec![main_obj.clone()];
    let mut compiled = std::collections::HashSet::new();
    compiled.insert(
        input_path
            .canonicalize()
            .unwrap_or(input_path.to_path_buf()),
    );

    // Each pending import carries (module_path, symbols, importing_file_dir, version).
    // Relative paths are resolved from the importing file's directory, not the main file's.
    // RCB-213: version is now carried through for versioned package resolution.
    let mut pending_imports: Vec<(String, Vec<String>, PathBuf, Option<String>)> = imports
        .into_iter()
        .map(|(p, s, v)| (p, s, base_dir.to_path_buf(), v))
        .collect();

    let result = (|| -> Result<PathBuf, CompileError> {
        while let Some((module_path, _symbols, importer_dir, version)) = pending_imports.pop() {
            let dep_path = resolve_module_path(&importer_dir, &module_path, version.as_deref());
            let canonical = dep_path.canonicalize().unwrap_or(dep_path.clone());
            if compiled.contains(&canonical) {
                continue;
            }
            compiled.insert(canonical);

            let dep_dir = dep_path.parent().unwrap_or(Path::new(".")).to_path_buf();
            let (obj_path, sub_imports) = compile_to_object(&dep_path)?;
            all_objs.push(obj_path);
            pending_imports.extend(
                sub_imports
                    .into_iter()
                    .map(|(p, s, v)| (p, s, dep_dir.clone(), v)),
            );
        }

        // リンカ呼び出し
        let bin_path = match output_path {
            Some(p) => p.to_path_buf(),
            None => {
                let stem = input_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output");
                base_dir.join(stem)
            }
        };

        link_objects(&all_objs, &bin_path)?;
        Ok(bin_path)
    })();

    // .o ファイルを削除（エラー時も確実にクリーンアップ）
    for obj in &all_objs {
        let _ = fs::remove_file(obj);
    }

    result
}

/// モジュールパスの解決: "./math.td" → 絶対パス
/// RCB-103: Support ~/ (project root relative) and package imports.
/// RCB-213: Support versioned package imports via resolve_package_import_versioned.
fn resolve_module_path(base_dir: &Path, module_path: &str, version: Option<&str>) -> PathBuf {
    let path = if module_path.starts_with("./") || module_path.starts_with("../") {
        base_dir.join(module_path)
    } else if Path::new(module_path).is_absolute() {
        PathBuf::from(module_path)
    } else if let Some(stripped) = module_path.strip_prefix("~/") {
        let root = find_project_root(base_dir);
        root.join(stripped)
    } else {
        // Package import
        let root = find_project_root(base_dir);

        // RCB-213: Versioned resolution with longest-prefix matching.
        // Supports submodule imports (e.g., alice/pkg/submod@b.12 resolves to
        // .taida/deps/alice/pkg@b.12/submod.td).
        if let Some(ver) = version {
            if let Some(resolution) =
                crate::pkg::resolver::resolve_package_module_versioned(&root, module_path, ver)
            {
                match resolution.submodule {
                    Some(submodule_path) => {
                        resolution.pkg_dir.join(format!("{}.td", submodule_path))
                    }
                    None => {
                        let entry =
                            match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir) {
                                Ok(Some(manifest)) => manifest.entry,
                                _ => "main.td".to_string(),
                            };
                        if entry.starts_with("./") || entry.starts_with("../") {
                            resolution.pkg_dir.join(entry[2..].trim_start_matches('/'))
                        } else {
                            resolution.pkg_dir.join(&entry)
                        }
                    }
                }
            } else {
                // RCB-213: Versioned package not found — do not fall back silently.
                PathBuf::from(format!("<unresolved package: {}@{}>", module_path, ver))
            }
        } else if let Some(resolution) =
            crate::pkg::resolver::resolve_package_module(&root, module_path)
        {
            match resolution.submodule {
                Some(submodule_path) => resolution.pkg_dir.join(format!("{}.td", submodule_path)),
                None => {
                    let entry = match crate::pkg::manifest::Manifest::from_dir(&resolution.pkg_dir)
                    {
                        Ok(Some(manifest)) => manifest.entry,
                        _ => "main.td".to_string(),
                    };
                    if entry.starts_with("./") || entry.starts_with("../") {
                        resolution.pkg_dir.join(entry[2..].trim_start_matches('/'))
                    } else {
                        resolution.pkg_dir.join(&entry)
                    }
                }
            }
        } else {
            // RCB-103 fix: package resolution failed — use a clearly
            // non-existent path so the caller gets a meaningful "not found"
            // error instead of silently misresolving to a local file.
            PathBuf::from(format!("<unresolved package: {}>", module_path))
        }
    };
    // RCB-303: Reject relative imports that escape the project root (path traversal).
    if module_path.starts_with("./") || module_path.starts_with("../") {
        let project_root = find_project_root(base_dir);
        let reject = if let Ok(resolved) = path.canonicalize() {
            if let Ok(root_canonical) = project_root.canonicalize() {
                !resolved.starts_with(&root_canonical)
            } else {
                // Cannot canonicalize project root — reject if path contains ".."
                module_path.contains("..")
            }
        } else {
            // Cannot canonicalize target — reject if path contains ".."
            module_path.contains("..")
        };
        if reject {
            return PathBuf::from(format!("<path traversal rejected: {}>", module_path));
        }
    }

    path
}

/// RCB-103: Find project root by walking up from the given directory.
fn find_project_root(start_dir: &Path) -> PathBuf {
    let mut dir = start_dir.to_path_buf();
    loop {
        if dir.join("packages.tdm").exists()
            || dir.join("taida.toml").exists()
            || dir.join(".taida").exists()
            || dir.join(".git").exists()
        {
            return dir;
        }
        if !dir.pop() {
            break;
        }
    }
    start_dir.to_path_buf()
}

/// 複数 .o ファイルをリンクしてバイナリを生成
fn link_objects(obj_paths: &[PathBuf], bin_path: &Path) -> Result<(), CompileError> {
    // メイン .o のパスからエントリポイント C ファイルの場所を決定
    let obj_path = &obj_paths[0];
    link_objects_inner(obj_paths, obj_path, bin_path)
}

fn link_objects_inner(
    obj_paths: &[PathBuf],
    obj_path: &Path,
    bin_path: &Path,
) -> Result<(), CompileError> {
    // cc を使ってリンク（ランタイム関数はプロセス内に存在しないので、
    // スタブ .o も生成する必要がある）
    //
    // 戦略: main() エントリポイントを含む C ラッパーを生成し、
    // _taida_main を呼ぶ。ランタイム関数は Rust ライブラリとして
    // リンクする。
    //
    // Phase N1 では簡易アプローチ: C エントリポイント + ランタイムを
    // 別途コンパイルしてリンク。

    // C エントリポイントを生成
    let c_wrapper = include_str!("native_runtime.c");

    let c_path = obj_path.with_extension("_entry.c");
    fs::write(&c_path, c_wrapper).map_err(|e| CompileError {
        message: format!("failed to write C wrapper: {}", e),
    })?;

    // コンパイル + リンク（プラットフォーム別）
    #[cfg(windows)]
    let mut cmd = {
        // Windows: clang または cl.exe を使用
        let compiler = find_native_compiler_windows()?;
        let mut c = Command::new(&compiler);
        c.arg(&c_path);
        for obj in obj_paths {
            c.arg(obj);
        }
        // clang の場合は Unix 風オプション、cl.exe の場合は MSVC オプション
        if compiler.contains("clang") {
            // -lm is not needed on Windows (included in MSVC CRT).
            // -lpthread is required: native_runtime.c uses pthread for Async support.
            c.arg("-o").arg(bin_path).arg("-lpthread");
        } else {
            // cl.exe: pthread is not natively available; native_runtime.c's pthread
            // usage will need a pthreads-win32 library or Windows threads adaptation.
            c.arg(&format!("/Fe:{}", bin_path.display()));
        }
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        // Unix: cc でコンパイル + リンク（-no-pie で PIE 警告を回避）
        let mut c = Command::new("cc");
        c.arg("-no-pie").arg(&c_path);
        for obj in obj_paths {
            c.arg(obj);
        }
        c.arg("-o").arg(bin_path).arg("-lm").arg("-lpthread");
        c
    };

    let status = cmd.status().map_err(|e| CompileError {
        message: format!("linker invocation failed: {}", e),
    })?;

    // 一時ファイルを削除
    let _ = fs::remove_file(&c_path);

    if !status.success() {
        return Err(CompileError {
            message: format!("linker failed with exit code: {:?}", status.code()),
        });
    }

    Ok(())
}

/// Windows で native コンパイラ（clang または cl.exe）を検出する
#[cfg(windows)]
fn find_native_compiler_windows() -> Result<String, CompileError> {
    if let Some(path) = which_command("clang") {
        return Ok(path);
    }
    if let Some(path) = which_command("cl.exe") {
        return Ok(path);
    }
    Err(CompileError {
        message: "C compiler not found. Install clang or Visual Studio Build Tools (cl.exe)."
            .to_string(),
    })
}

/// `which` (Unix) / `where.exe` (Windows) でコマンドの絶対パスを検索する
fn which_command(name: &str) -> Option<String> {
    let which_cmd = if cfg!(windows) { "where.exe" } else { "which" };
    if let Ok(output) = Command::new(which_cmd).arg(name).output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// W-1: wasm-min コンパイルパス
// ---------------------------------------------------------------------------

/// wasm-ld の実行パスを検出する
fn find_wasm_ld() -> Result<PathBuf, CompileError> {
    // 1. PATH 上の wasm-ld
    if let Some(path) = which_command("wasm-ld") {
        return Ok(PathBuf::from(path));
    }

    // 2. 既知の LLVM インストール先を探索（プラットフォーム別）
    #[cfg(not(windows))]
    let candidates: &[&str] = &[
        "/opt/rocm-6.4.2/lib/llvm/bin/wasm-ld",
        "/usr/lib/llvm-17/bin/wasm-ld",
        "/usr/lib/llvm-18/bin/wasm-ld",
        "/usr/lib/llvm-19/bin/wasm-ld",
        "/usr/lib/llvm-20/bin/wasm-ld",
        "/usr/local/bin/wasm-ld",
    ];
    #[cfg(windows)]
    let candidates: &[&str] = &[
        "C:\\Program Files\\LLVM\\bin\\wasm-ld.exe",
        "C:\\Program Files (x86)\\LLVM\\bin\\wasm-ld.exe",
    ];

    for candidate in candidates {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Ok(p);
        }
    }

    let install_hint = if cfg!(windows) {
        "wasm-ld not found. Install LLVM (https://releases.llvm.org/) and ensure wasm-ld.exe is on PATH."
    } else {
        "wasm-ld not found. Install LLVM/LLD (e.g. `apt install lld-17`)."
    };
    Err(CompileError {
        message: install_hint.to_string(),
    })
}

/// clang の実行パスを検出する（wasm32 クロスコンパイル用）
fn find_clang_for_wasm() -> Result<String, CompileError> {
    // バージョン付きの clang を優先的に検索（Unix のみ、Windows は clang.exe 一択）
    #[cfg(not(windows))]
    {
        for ver in &["17", "18", "19", "20"] {
            let name = format!("clang-{}", ver);
            if let Some(path) = which_command(&name) {
                return Ok(path);
            }
        }
    }
    // フォールバック: PATH 上の clang
    if let Some(path) = which_command("clang") {
        return Ok(path);
    }

    let install_hint = if cfg!(windows) {
        "clang not found. Install LLVM (https://releases.llvm.org/) and ensure clang.exe is on PATH."
    } else {
        "clang not found. Install clang (e.g. `apt install clang-17`)."
    };
    Err(CompileError {
        message: install_hint.to_string(),
    })
}

const WASM_STDINT_HEADER: &str = r#"#ifndef TAIDA_WASM_STDINT_H
#define TAIDA_WASM_STDINT_H

typedef signed char int8_t;
typedef unsigned char uint8_t;
typedef short int16_t;
typedef unsigned short uint16_t;
typedef int int32_t;
typedef unsigned int uint32_t;
typedef long long int64_t;
typedef unsigned long long uint64_t;
typedef int intptr_t;
typedef unsigned int uintptr_t;

#endif
"#;

fn write_wasm_stdint_header(include_dir: &Path) -> Result<(), CompileError> {
    fs::create_dir_all(include_dir).map_err(|e| CompileError {
        message: format!(
            "failed to create wasm include dir '{}': {}",
            include_dir.display(),
            e
        ),
    })?;

    if let Err(e) = fs::write(include_dir.join("stdint.h"), WASM_STDINT_HEADER) {
        let _ = fs::remove_dir_all(include_dir);
        return Err(CompileError {
            message: format!(
                "failed to write wasm stdint shim into '{}': {}",
                include_dir.display(),
                e
            ),
        });
    }

    Ok(())
}

fn wasm_clang_base_args(include_dir: &Path) -> Vec<String> {
    let mut args: Vec<String> = WASM_CLANG_FLAGS.iter().map(|s| s.to_string()).collect();
    args.push("-I".to_string());
    args.push(include_dir.display().to_string());
    args
}

#[allow(clippy::too_many_arguments)]
fn run_wasm_clang_object(
    clang: &str,
    clang_args: &[String],
    input: &Path,
    output: &Path,
    include_dir: &Path,
    cleanup_paths: &[&Path],
    cleanup_include_dir_on_error: bool,
    preserved_source: Option<&Path>,
) -> Result<ExitStatus, CompileError> {
    Command::new(clang)
        .args(clang_args)
        .arg(input)
        .arg("-o")
        .arg(output)
        .status()
        .map_err(|e| {
            for path in cleanup_paths {
                let _ = fs::remove_file(path);
            }
            if cleanup_include_dir_on_error {
                let _ = fs::remove_dir_all(include_dir);
            }
            CompileError {
                message: match preserved_source {
                    Some(source) => format!(
                        "clang invocation failed: {} (source preserved at: {}; shim preserved at: {})",
                        e,
                        source.display(),
                        include_dir.display()
                    ),
                    None => format!("clang invocation failed: {}", e),
                },
            }
        })
}

/// wasm-min モジュールインライン展開: 依存モジュールの IR 関数をメインモジュールに融合する。
///
/// 各 import されたモジュールを再帰的に parse → lower し、得られた IR 関数を
/// メインモジュールの `functions` に追加する。融合後、`imports` を空にすることで
/// C emitter は単一モジュールとして処理できる。
///
/// RC-1k: エクスポートフィルタリング — 依存モジュールの全関数を融合するのではなく、
/// import で要求されたシンボル + その推移的依存のみを融合する。
/// これにより非公開関数がリンク時に名前空間に漏洩することを防ぐ。
///
/// RCB-43: ダイヤモンド依存時の IR キャッシュ — 同一モジュールが複数パスから
/// 異なるシンボルセットで import された場合、2回目以降は初回の parse+lower 結果を
/// キャッシュから再利用し、ファイル再読込・再パース・再 lower を回避する。
fn inline_wasm_module_imports(
    main_module: &mut super::ir::IrModule,
    base_dir: &Path,
    main_path: &Path,
) -> Result<(), CompileError> {
    use std::collections::{HashMap, HashSet};

    // compiled: モジュールパス → 既にメインモジュールに融合済みのシンボル集合
    // 同一モジュールが複数箇所から異なるシンボルセットで import された場合、
    // 2回目以降は差分シンボルの推移的依存のみを追加融合する。
    let mut compiled: HashMap<PathBuf, HashSet<String>> = HashMap::new();
    compiled.insert(
        main_path.canonicalize().unwrap_or(main_path.to_path_buf()),
        HashSet::new(),
    );

    // RCB-43: IR キャッシュ — parse+lower 結果をモジュールパスごとにキャッシュする。
    // ダイヤモンド依存（A→B, A→C, B→D, C→D）で D が複数回参照される場合、
    // 2回目以降はキャッシュから IR を取得し、再パースを回避する。
    let mut ir_cache: HashMap<PathBuf, super::ir::IrModule> = HashMap::new();

    // pending: (module_path, importer_dir, requested_syms, version)
    // requested_syms: importer が要求するリンクシンボル名のリスト
    // RCB-213: version is carried through for versioned package resolution.
    let mut pending: Vec<(String, PathBuf, Vec<String>, Option<String>)> = main_module
        .imports
        .iter()
        .map(|(path, syms, ver)| {
            (
                path.clone(),
                base_dir.to_path_buf(),
                syms.clone(),
                ver.clone(),
            )
        })
        .collect();

    while let Some((module_path, importer_dir, requested_syms, version)) = pending.pop() {
        let dep_path = resolve_module_path(&importer_dir, &module_path, version.as_deref());
        let canonical = dep_path.canonicalize().unwrap_or(dep_path.clone());

        // 既にコンパイル済みのモジュールか確認
        if let Some(already_merged) = compiled.get(&canonical) {
            // 全ての要求シンボルが既に融合済みならスキップ
            let new_syms: Vec<String> = requested_syms
                .iter()
                .filter(|s| !already_merged.contains(s.as_str()))
                .cloned()
                .collect();
            if new_syms.is_empty() {
                continue;
            }
            // 差分シンボルがある場合: キャッシュ済み IR から差分の推移的依存のみを追加融合
            // RCB-43: ファイル再読込・再パース・再 lower を回避
            let cached_ir = ir_cache.get(&canonical).unwrap_or_else(|| {
                panic!(
                    "BUG: IR cache missing for '{}' despite being in compiled map",
                    canonical.display()
                )
            });

            // 差分シンボルの推移的依存のみを計算
            let needed = compute_needed_functions(cached_ir, &new_syms);
            // 既に融合済みの関数は除外して追加
            for func in &cached_ir.functions {
                if !needed.contains(&func.name) {
                    continue;
                }
                if !main_module.functions.iter().any(|f| f.name == func.name) {
                    main_module.functions.push(func.clone());
                }
            }
            // 融合済みシンボルを更新
            compiled.get_mut(&canonical).unwrap().extend(needed);
            continue;
        }

        let dep_source = fs::read_to_string(&dep_path).map_err(|e| CompileError {
            message: format!("failed to read module '{}': {}", dep_path.display(), e),
        })?;

        let (dep_program, dep_errors) = parse(&dep_source);
        if !dep_errors.is_empty() {
            let msgs: Vec<String> = dep_errors.iter().map(|e| format!("{}", e)).collect();
            return Err(CompileError {
                message: format!(
                    "parse errors in module '{}':\n{}",
                    dep_path.display(),
                    msgs.join("\n")
                ),
            });
        }

        let mut dep_lowering = Lowering::new();
        if let Some(parent) = dep_path.parent() {
            dep_lowering.set_source_dir(parent.to_path_buf());
        }
        dep_lowering.set_module_key(Lowering::module_key_for_path(&dep_path));
        let dep_ir = dep_lowering
            .lower_program(&dep_program)
            .map_err(|e| CompileError {
                message: format!("lowering error in module '{}': {}", dep_path.display(), e),
            })?;

        // RC-1k: エクスポートフィルタリング
        // 要求されたシンボル + init関数 + 推移的依存のみを融合する
        let needed = compute_needed_functions(&dep_ir, &requested_syms);

        for func in &dep_ir.functions {
            if !needed.contains(&func.name) {
                continue; // 非公開・不要な関数はスキップ
            }
            if !main_module.functions.iter().any(|f| f.name == func.name) {
                main_module.functions.push(func.clone());
            }
        }

        // 依存モジュールがさらに import していれば、それも再帰的に処理
        let dep_dir = dep_path.parent().unwrap_or(Path::new(".")).to_path_buf();
        for (sub_path, sub_syms, sub_ver) in &dep_ir.imports {
            pending.push((
                sub_path.clone(),
                dep_dir.clone(),
                sub_syms.clone(),
                sub_ver.clone(),
            ));
        }

        // RCB-43: IR をキャッシュに保存（diff-symbol パスで再利用するため）
        ir_cache.insert(canonical.clone(), dep_ir);

        // 融合済みシンボルを記録
        compiled.insert(canonical, needed);
    }

    // インライン展開完了: imports を空にする
    main_module.imports.clear();

    Ok(())
}

/// RC-1k: 依存モジュールの IR から、要求されたシンボルとその推移的依存を計算する。
///
/// 融合が必要な関数:
/// 1. `requested_syms` に含まれる関数（importer が import で要求したもの）
/// 2. `_taida_init_*` 関数（モジュール初期化、グローバル変数の設定に必要）
/// 3. 上記1,2が CallUser / MakeClosure / FuncAddr で参照する関数（推移的依存）
fn compute_needed_functions(
    dep_ir: &super::ir::IrModule,
    requested_syms: &[String],
) -> std::collections::HashSet<String> {
    use std::collections::{HashMap, HashSet, VecDeque};

    // 関数名 → 関数本体のマップを構築
    let func_map: HashMap<&str, &super::ir::IrFunction> = dep_ir
        .functions
        .iter()
        .map(|f| (f.name.as_str(), f))
        .collect();

    // シード: 要求されたシンボル + init 関数
    let mut needed: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();

    for sym in requested_syms {
        if !needed.contains(sym) {
            needed.insert(sym.clone());
            queue.push_back(sym.clone());
        }
    }

    // init 関数は常に必要（グローバル変数の初期化を行う）
    for func in &dep_ir.functions {
        if func.name.starts_with("_taida_init_") && !needed.contains(&func.name) {
            needed.insert(func.name.clone());
            queue.push_back(func.name.clone());
        }
    }

    // BFS で推移的依存を収集
    while let Some(func_name) = queue.pop_front() {
        if let Some(func) = func_map.get(func_name.as_str()) {
            let refs = collect_func_refs(&func.body);
            for r in refs {
                if func_map.contains_key(r.as_str()) && !needed.contains(&r) {
                    needed.insert(r.clone());
                    queue.push_back(r);
                }
            }
        }
    }

    needed
}

/// IR 命令列から参照されている関数名を収集する（CallUser, MakeClosure, FuncAddr, Call）。
/// CondBranch のネストも再帰的に走査する。
fn collect_func_refs(insts: &[super::ir::IrInst]) -> Vec<String> {
    use super::ir::IrInst;
    let mut refs = Vec::new();

    for inst in insts {
        match inst {
            IrInst::CallUser(_, name, _) => refs.push(name.clone()),
            IrInst::Call(_, name, _) => {
                // ランタイム関数は除外: _taida_ / __taida_ プレフィックスで始まる
                // ユーザー定義関数のみを追跡する（ブラックリスト方式）。
                // これにより将来のプレフィックス追加（_taida_lambda_ 等）でも漏れない。
                if name.starts_with("_taida_") || name.starts_with("__taida_") {
                    refs.push(name.clone());
                }
            }
            IrInst::MakeClosure(_, name, _) => refs.push(name.clone()),
            IrInst::FuncAddr(_, name) => refs.push(name.clone()),
            IrInst::CondBranch(_, arms) => {
                for arm in arms {
                    refs.extend(collect_func_refs(&arm.body));
                }
            }
            _ => {}
        }
    }

    refs
}

// ---------------------------------------------------------------------------
// RCB-20: Shared WASM compilation helpers
// ---------------------------------------------------------------------------

/// RCB-20: Frontend pipeline shared by all WASM profiles.
///
/// Performs: cycle detection -> parse -> lower -> module inline -> RC optimize -> C emit.
/// Returns (generated_c_source, resolved_wasm_output_path).
fn wasm_frontend(
    input_path: &Path,
    output_path: Option<&Path>,
    profile: emit_wasm_c::WasmProfile,
) -> Result<(String, PathBuf), CompileError> {
    // 循環インポート検出
    module_graph::detect_local_import_cycle(input_path).map_err(|e| CompileError {
        message: e.to_string(),
    })?;

    let source = fs::read_to_string(input_path).map_err(|e| CompileError {
        message: format!("failed to read '{}': {}", input_path.display(), e),
    })?;

    let (program, parse_errors) = parse(&source);
    if !parse_errors.is_empty() {
        let msgs: Vec<String> = parse_errors.iter().map(|e| format!("{}", e)).collect();
        return Err(CompileError {
            message: format!("parse errors:\n{}", msgs.join("\n")),
        });
    }

    let mut lowering = Lowering::new();
    if let Some(parent) = input_path.parent() {
        lowering.set_source_dir(parent.to_path_buf());
    }
    lowering.set_module_key(Lowering::module_key_for_path(input_path));
    let mut ir_module = lowering.lower_program(&program).map_err(|e| CompileError {
        message: format!("{}", e),
    })?;

    // モジュールインライン展開: 依存モジュールの IR 関数をメインモジュールに融合
    if !ir_module.imports.is_empty() {
        let base_dir = input_path.parent().unwrap_or(Path::new("."));
        inline_wasm_module_imports(&mut ir_module, base_dir, input_path)?;
    }

    // RC 最適化パス
    rc_opt::optimize(&mut ir_module);

    let profile_name = match profile {
        emit_wasm_c::WasmProfile::Min => "wasm-min",
        emit_wasm_c::WasmProfile::Wasi => "wasm-wasi",
        emit_wasm_c::WasmProfile::Edge => "wasm-edge",
        emit_wasm_c::WasmProfile::Full => "wasm-full",
    };

    // IR -> C ソースコード
    let generated_c = emit_wasm_c::emit_c(&ir_module, profile).map_err(|e| CompileError {
        message: format!("{} C emission failed: {}", profile_name, e),
    })?;

    let base_dir = input_path.parent().unwrap_or(Path::new("."));

    // 出力パスの決定
    let wasm_path = match output_path {
        Some(p) => p.to_path_buf(),
        None => {
            let stem = input_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("output");
            base_dir.join(format!("{}.wasm", stem))
        }
    };

    Ok((generated_c, wasm_path))
}

/// RCB-20: Uncached WASM backend -- compile runtime C sources and link into .wasm.
///
/// `runtime_sources`: list of (name, C source content) pairs for runtime layers.
///   e.g. `[("rt", runtime_core_wasm.c)]` for wasm-min,
///        `[("rt_core", ...), ("rt_wasi", ...)]` for wasm-wasi.
/// `tmp_suffix`: unique suffix to avoid temp file collisions between profiles.
fn wasm_compile_and_link_uncached(
    generated_c: &str,
    wasm_path: &Path,
    runtime_sources: &[(&str, &str)],
    tmp_suffix: &str,
) -> Result<(), CompileError> {
    let tmp_base = wasm_path.with_extension(tmp_suffix);
    let gen_c_path = tmp_base.with_extension("gen.c");
    let gen_obj_path = tmp_base.with_extension("gen.o");
    let include_dir = tmp_base.with_extension("include");

    // Compute runtime file paths up front
    let rt_files: Vec<(PathBuf, PathBuf)> = runtime_sources
        .iter()
        .map(|(name, _)| {
            (
                tmp_base.with_extension(format!("{}.c", name)),
                tmp_base.with_extension(format!("{}.o", name)),
            )
        })
        .collect();

    // Helper: remove all runtime C source files
    let cleanup_rt_c = |rt_files: &[(PathBuf, PathBuf)]| {
        for (c_path, _) in rt_files {
            let _ = fs::remove_file(c_path);
        }
    };

    // 生成された C ソースを書き出し
    fs::write(&gen_c_path, generated_c).map_err(|e| CompileError {
        message: format!("failed to write generated C: {}", e),
    })?;

    // Runtime C ソースを書き出し
    for (i, (name, source)) in runtime_sources.iter().enumerate() {
        fs::write(&rt_files[i].0, source).map_err(|e| CompileError {
            message: format!("failed to write wasm {} source: {}", name, e),
        })?;
    }

    let clang = find_clang_for_wasm()?;
    if let Err(err) = write_wasm_stdint_header(&include_dir) {
        let _ = fs::remove_file(&gen_c_path);
        cleanup_rt_c(&rt_files);
        return Err(err);
    }

    let clang_args = wasm_clang_base_args(&include_dir);

    // 生成 C をコンパイル
    let gen_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &gen_c_path,
        &gen_obj_path,
        &include_dir,
        &[],
        false,
        Some(&gen_c_path),
    )?;

    if !gen_status.success() {
        cleanup_rt_c(&rt_files);
        return Err(CompileError {
            message: format!(
                "clang wasm32 compilation of generated code failed (source preserved at: {}; shim preserved at: {})",
                gen_c_path.display(),
                include_dir.display()
            ),
        });
    }

    // Runtime layers を順番にコンパイル
    // 各 runtime のコンパイル時に、先行するソース/オブジェクトを cleanup_paths に含める
    for i in 0..runtime_sources.len() {
        let rt_name = runtime_sources[i].0;
        let rt_c = &rt_files[i].0;
        let rt_o = &rt_files[i].1;

        // cleanup_paths: gen + 全 runtime C ソース + 先行 runtime の .o
        let mut cleanup: Vec<&Path> = vec![gen_c_path.as_path(), gen_obj_path.as_path()];
        for (c_path, _) in &rt_files {
            cleanup.push(c_path.as_path());
        }
        for (_, o_path) in rt_files.iter().take(i) {
            cleanup.push(o_path.as_path());
        }

        let status = run_wasm_clang_object(
            &clang,
            &clang_args,
            rt_c,
            rt_o,
            &include_dir,
            &cleanup,
            true,
            None,
        )?;

        if !status.success() {
            // Clean up everything produced so far
            let _ = fs::remove_file(&gen_c_path);
            let _ = fs::remove_file(&gen_obj_path);
            cleanup_rt_c(&rt_files);
            for (_, o_path) in rt_files.iter().take(i) {
                let _ = fs::remove_file(o_path);
            }
            let _ = fs::remove_dir_all(&include_dir);
            return Err(CompileError {
                message: format!("clang wasm32 compilation of {} runtime failed.", rt_name),
            });
        }
    }

    // 一時 C ソースを削除
    let _ = fs::remove_file(&gen_c_path);
    cleanup_rt_c(&rt_files);
    let _ = fs::remove_dir_all(&include_dir);

    // wasm-ld でリンク (runtime .o files + gen.o)
    let wasm_ld = find_wasm_ld()?;
    let mut cmd = Command::new(&wasm_ld);
    cmd.args([
        "--no-entry",
        "--export=_start",
        "--strip-all",
        "--gc-sections",
    ]);
    for (_, o_path) in &rt_files {
        cmd.arg(o_path);
    }
    cmd.arg(&gen_obj_path).arg("-o").arg(wasm_path);

    let ld_status = cmd.status().map_err(|e| CompileError {
        message: format!("wasm-ld invocation failed: {}", e),
    })?;

    // 一時 .o ファイルの削除
    let _ = fs::remove_file(&gen_obj_path);
    for (_, o_path) in &rt_files {
        let _ = fs::remove_file(o_path);
    }

    if !ld_status.success() {
        return Err(CompileError {
            message: format!("wasm-ld failed with exit code: {:?}", ld_status.code()),
        });
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// WASM profile compilation functions
// ---------------------------------------------------------------------------

/// .td ファイルを wasm-min ターゲットでコンパイルし .wasm を生成する
///
/// モジュールインポート対応: 依存モジュールを IR レベルでインライン展開し、
/// 単一の IR モジュールとして C emit に渡す (Option C: AST/IR インライン展開)。
/// パイプライン: .td -> parse -> IR -> (依存 IR 融合) -> C source -> clang(wasm32) -> .o -> wasm-ld -> .wasm
///
/// Cranelift の ISA に wasm32 が存在しないため、IR -> C -> clang ルートを採用する。
pub fn compile_file_wasm(
    input_path: &Path,
    output_path: Option<&Path>,
) -> Result<PathBuf, CompileError> {
    compile_file_wasm_cached(input_path, output_path, None)
}

/// wasm-min コンパイル with optional runtime cache (RC-8a/8d).
pub fn compile_file_wasm_cached(
    input_path: &Path,
    output_path: Option<&Path>,
    cache: Option<&WasmRuntimeCache>,
) -> Result<PathBuf, CompileError> {
    let (generated_c, wasm_path) =
        wasm_frontend(input_path, output_path, emit_wasm_c::WasmProfile::Min)?;

    if let Some(rt_cache) = cache {
        let rt_obj = rt_cache.rt_core()?;
        rt_cache.link_wasm_cached(&[rt_obj], &generated_c, &wasm_path, "_wasm_tmp", &[])?;
        return Ok(wasm_path);
    }

    wasm_compile_and_link_uncached(
        &generated_c,
        &wasm_path,
        &[("rt", include_str!("runtime_core_wasm.c"))],
        "_wasm_tmp",
    )?;

    Ok(wasm_path)
}

// ---------------------------------------------------------------------------
// WW-2: wasm-wasi コンパイルパス
// ---------------------------------------------------------------------------

/// .td ファイルを wasm-wasi ターゲットでコンパイルし .wasm を生成する
///
/// wasm-wasi は wasm-min の上位互換で、WASI I/O (env, file read/write) を追加する。
/// パイプライン: .td -> parse -> IR -> C source -> clang(wasm32) -> .o -> wasm-ld -> .wasm
///
/// リンク構成: gen.o + rt_core.o + rt_wasi.o -> wasm-ld -> output.wasm
pub fn compile_file_wasm_wasi(
    input_path: &Path,
    output_path: Option<&Path>,
) -> Result<PathBuf, CompileError> {
    compile_file_wasm_wasi_cached(input_path, output_path, None)
}

/// wasm-wasi コンパイル with optional runtime cache (RC-8a/8d).
pub fn compile_file_wasm_wasi_cached(
    input_path: &Path,
    output_path: Option<&Path>,
    cache: Option<&WasmRuntimeCache>,
) -> Result<PathBuf, CompileError> {
    let (generated_c, wasm_path) =
        wasm_frontend(input_path, output_path, emit_wasm_c::WasmProfile::Wasi)?;

    if let Some(rt_cache) = cache {
        let rt_core = rt_cache.rt_core()?;
        let rt_wasi = rt_cache.rt_wasi()?;
        rt_cache.link_wasm_cached(
            &[rt_core, rt_wasi],
            &generated_c,
            &wasm_path,
            "_wasm_wasi_tmp",
            &[],
        )?;
        return Ok(wasm_path);
    }

    wasm_compile_and_link_uncached(
        &generated_c,
        &wasm_path,
        &[
            ("rt_core", include_str!("runtime_core_wasm.c")),
            ("rt_wasi", include_str!("runtime_wasi_io.c")),
        ],
        "_wasm_wasi_tmp",
    )?;

    Ok(wasm_path)
}

/// wasm-edge: Cloudflare Workers 向け edge profile
///
/// wasm-min の上に host import (taida_host) ベースの env API を追加する。
/// WASI import はそのまま残し、JS glue が wasi_snapshot_preview1.fd_write を提供する。
///
/// パイプライン: .td -> parse -> IR -> C source -> clang(wasm32) -> .o -> wasm-ld -> .wasm
///
/// リンク構成: gen.o + rt_core.o + rt_edge.o -> wasm-ld -> output.wasm
pub fn compile_file_wasm_edge(
    input_path: &Path,
    output_path: Option<&Path>,
) -> Result<WasmEdgeOutput, CompileError> {
    compile_file_wasm_edge_cached(input_path, output_path, None)
}

/// wasm-edge コンパイル with optional runtime cache (RC-8a/8d).
pub fn compile_file_wasm_edge_cached(
    input_path: &Path,
    output_path: Option<&Path>,
    cache: Option<&WasmRuntimeCache>,
) -> Result<WasmEdgeOutput, CompileError> {
    let (generated_c, wasm_path) =
        wasm_frontend(input_path, output_path, emit_wasm_c::WasmProfile::Edge)?;

    if let Some(rt_cache) = cache {
        let rt_core = rt_cache.rt_core()?;
        let rt_edge = rt_cache.rt_edge()?;
        rt_cache.link_wasm_cached(
            &[rt_core, rt_edge],
            &generated_c,
            &wasm_path,
            "_wasm_edge_tmp",
            &[],
        )?;
        let glue_path = generate_edge_js_glue(&wasm_path)?;
        return Ok(WasmEdgeOutput {
            wasm_path,
            glue_path,
        });
    }

    wasm_compile_and_link_uncached(
        &generated_c,
        &wasm_path,
        &[
            ("rt_core", include_str!("runtime_core_wasm.c")),
            ("rt_edge", include_str!("runtime_edge_host.c")),
        ],
        "_wasm_edge_tmp",
    )?;

    // WE-2d: Generate JS glue alongside the .wasm
    let glue_path = generate_edge_js_glue(&wasm_path)?;

    Ok(WasmEdgeOutput {
        wasm_path,
        glue_path,
    })
}

// ---------------------------------------------------------------------------
// WF-2: wasm-full -- extended runtime profile (superset of wasm-wasi)
// ---------------------------------------------------------------------------

/// wasm-full: 拡張 runtime profile
///
/// wasm-wasi の上位互換として、文字列/数値 molds, 拡張 list/hashmap/set,
/// JSON, bytes, bitwise 等の重い runtime を追加する。
///
/// パイプライン: .td -> parse -> IR -> C source -> clang(wasm32) -> .o -> wasm-ld -> .wasm
///
/// リンク構成: gen.o + rt_core.o + rt_wasi.o + rt_full.o -> wasm-ld -> output.wasm
pub fn compile_file_wasm_full(
    input_path: &Path,
    output_path: Option<&Path>,
) -> Result<PathBuf, CompileError> {
    compile_file_wasm_full_cached(input_path, output_path, None)
}

/// wasm-full コンパイル with optional runtime cache (RC-8a/8d).
pub fn compile_file_wasm_full_cached(
    input_path: &Path,
    output_path: Option<&Path>,
    cache: Option<&WasmRuntimeCache>,
) -> Result<PathBuf, CompileError> {
    let (generated_c, wasm_path) =
        wasm_frontend(input_path, output_path, emit_wasm_c::WasmProfile::Full)?;

    if let Some(rt_cache) = cache {
        let rt_core = rt_cache.rt_core()?;
        let rt_wasi = rt_cache.rt_wasi()?;
        let rt_full = rt_cache.rt_full()?;
        rt_cache.link_wasm_cached(
            &[rt_core, rt_wasi, rt_full],
            &generated_c,
            &wasm_path,
            "_wasm_full_tmp",
            &[],
        )?;
        return Ok(wasm_path);
    }

    wasm_compile_and_link_uncached(
        &generated_c,
        &wasm_path,
        &[
            ("rt_core", include_str!("runtime_core_wasm.c")),
            ("rt_wasi", include_str!("runtime_wasi_io.c")),
            ("rt_full", include_str!("runtime_full_wasm.c")),
        ],
        "_wasm_full_tmp",
    )?;

    Ok(wasm_path)
}

// ---------------------------------------------------------------------------
// WE-2d: JS glue generation for Cloudflare Workers
// ---------------------------------------------------------------------------

/// Generate a JS glue file for Cloudflare Workers deployment.
///
/// The glue provides:
/// - `wasi_snapshot_preview1.fd_write` bridged to Response body capture
/// - `taida_host.env_get` / `taida_host.env_get_all` bridged to Workers `env`
/// - Workers `export default { fetch }` entrypoint
///
/// Output: `{stem}.edge.js` next to the `.wasm` file.
fn generate_edge_js_glue(wasm_path: &Path) -> Result<PathBuf, CompileError> {
    let stem = wasm_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let wasm_filename = wasm_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("output.wasm");

    let glue_path = wasm_path.with_extension("edge.js");

    let glue = generate_edge_js_source(stem, wasm_filename);

    fs::write(&glue_path, glue).map_err(|e| CompileError {
        message: format!("failed to write JS glue '{}': {}", glue_path.display(), e),
    })?;

    Ok(glue_path)
}

/// Generate the JS glue source code for Cloudflare Workers.
///
/// This is a pure function (no I/O) to facilitate testing.
pub fn generate_edge_js_source(stem: &str, wasm_filename: &str) -> String {
    format!(
        r#"// {stem}.edge.js -- Cloudflare Workers glue for Taida wasm-edge
// Generated by `taida build --target wasm-edge`
//
// Deploy: wrangler deploy --name {stem}
// wrangler.toml should set:
//   main = "{stem}.edge.js"
//   [wasm_modules]
//   WASM = "{wasm_filename}"

import WASM from "./{wasm_filename}";

export default {{
  async fetch(request, env, ctx) {{
    const stdoutChunks = [];
    const stderrChunks = [];

    let memory = new WebAssembly.Memory({{ initial: 2 }});
    const encoder = new TextEncoder();
    const decoder = new TextDecoder();

    // -- helpers --

    function readStr(ptr, len) {{
      return decoder.decode(new Uint8Array(memory.buffer, ptr, len));
    }}

    function writeStr(ptr, str) {{
      const bytes = encoder.encode(str);
      new Uint8Array(memory.buffer, ptr, bytes.length).set(bytes);
      return bytes.length;
    }}

    // -- wasi_snapshot_preview1 --

    const wasi = {{
      fd_write(fd, iovs_ptr, iovs_len, nwritten_ptr) {{
        const view = new DataView(memory.buffer);
        let total = 0;
        for (let i = 0; i < iovs_len; i++) {{
          const base = iovs_ptr + i * 8;
          const ptr = view.getUint32(base, true);
          const len = view.getUint32(base + 4, true);
          const chunk = new Uint8Array(memory.buffer, ptr, len);
          if (fd === 1) {{
            stdoutChunks.push(new Uint8Array(chunk));
          }} else if (fd === 2) {{
            stderrChunks.push(new Uint8Array(chunk));
          }}
          total += len;
        }}
        view.setUint32(nwritten_ptr, total, true);
        return 0;
      }},
    }};

    // -- taida_host --

    const taida_host = {{
      env_get(key_ptr, key_len, buf_ptr, buf_cap) {{
        const key = readStr(key_ptr, key_len);
        const val = env[key];
        if (val === undefined || val === null || typeof val !== "string") {{
          return 0;
        }}
        const bytes = encoder.encode(val);
        if (bytes.length > buf_cap) {{
          return bytes.length;
        }}
        new Uint8Array(memory.buffer, buf_ptr, bytes.length).set(bytes);
        return bytes.length;
      }},

      env_get_all(buf_ptr, buf_cap) {{
        const entries = [];
        for (const [k, v] of Object.entries(env)) {{
          if (typeof v === "string") {{
            entries.push(k + "=" + v);
          }}
        }}
        if (entries.length === 0) return 0;
        const payload = entries.join("\0") + "\0\0";
        const bytes = encoder.encode(payload);
        if (bytes.length > buf_cap) {{
          return bytes.length;
        }}
        new Uint8Array(memory.buffer, buf_ptr, bytes.length).set(bytes);
        return bytes.length;
      }},
    }};

    // -- instantiate and run --

    const importObject = {{
      env: {{ memory }},
      wasi_snapshot_preview1: wasi,
      taida_host,
    }};

    const instance = await WebAssembly.instantiate(WASM, importObject);

    // If the module exports its own memory, use that instead
    if (instance.exports.memory) {{
      memory = instance.exports.memory;
    }}

    if (instance.exports._start) {{
      instance.exports._start();
    }}

    // Log stderr to console
    if (stderrChunks.length > 0) {{
      const errBytes = concat(stderrChunks);
      console.error(decoder.decode(errBytes));
    }}

    // Return stdout as response
    const outBytes = concat(stdoutChunks);
    return new Response(decoder.decode(outBytes), {{
      headers: {{ "content-type": "text/plain; charset=utf-8" }},
    }});
  }},
}};

function concat(arrays) {{
  let total = 0;
  for (const a of arrays) total += a.length;
  const result = new Uint8Array(total);
  let offset = 0;
  for (const a of arrays) {{
    result.set(a, offset);
    offset += a.length;
  }}
  return result;
}}
"#,
        stem = stem,
        wasm_filename = wasm_filename,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FL-27: which_command should find common executables on the current platform
    #[test]
    fn test_which_command_finds_existing_binary() {
        if cfg!(windows) {
            let result = which_command("cmd.exe");
            assert!(
                result.is_some(),
                "which_command should find 'cmd.exe' on Windows"
            );
        } else {
            // `ls` exists on all Unix systems
            let result = which_command("ls");
            assert!(result.is_some(), "which_command should find 'ls' on Unix");
            assert!(
                result.unwrap().contains("ls"),
                "which_command result should contain 'ls'"
            );
        }
    }

    #[test]
    fn test_which_command_returns_none_for_nonexistent() {
        let result = which_command("__taida_nonexistent_binary_12345__");
        assert!(
            result.is_none(),
            "which_command should return None for nonexistent binaries"
        );
    }

    /// FL-27: find_wasm_ld error message should be platform-appropriate
    #[test]
    fn test_find_wasm_ld_error_message() {
        if find_wasm_ld().is_ok() {
            return; // skip — tool is available (PATH or known paths)
        }
        let err = find_wasm_ld().unwrap_err();
        if cfg!(windows) {
            assert!(!err.message.contains("apt"));
        } else {
            assert!(err.message.contains("apt"));
        }
    }

    /// FL-27: find_clang_for_wasm error message should be platform-appropriate
    #[test]
    fn test_find_clang_error_message() {
        if find_clang_for_wasm().is_ok() {
            return; // skip — tool is available (PATH or versioned)
        }
        let err = find_clang_for_wasm().unwrap_err();
        if cfg!(windows) {
            assert!(!err.message.contains("apt"));
        } else {
            assert!(err.message.contains("apt"));
        }
    }

    /// RC-1k: compute_needed_functions filters out unreferenced private functions
    #[test]
    fn test_compute_needed_functions_basic() {
        use super::super::ir::{IrFunction, IrModule};

        let mut module = IrModule::new();

        // public_fn: no calls to other functions
        let public_fn = IrFunction::new_with_params(
            "_taida_fn_mod1_public_fn".to_string(),
            vec!["x".to_string()],
        );
        module.functions.push(public_fn);

        // _private_helper: internal function, not exported
        let private_fn = IrFunction::new_with_params(
            "_taida_fn_mod1__private_helper".to_string(),
            vec!["x".to_string()],
        );
        module.functions.push(private_fn);

        // _another_private: internal function, not exported
        let another_fn = IrFunction::new_with_params(
            "_taida_fn_mod1__another_private".to_string(),
            vec!["y".to_string()],
        );
        module.functions.push(another_fn);

        // Request only public_fn
        let requested = vec!["_taida_fn_mod1_public_fn".to_string()];
        let needed = compute_needed_functions(&module, &requested);

        assert!(needed.contains("_taida_fn_mod1_public_fn"));
        assert!(!needed.contains("_taida_fn_mod1__private_helper"));
        assert!(!needed.contains("_taida_fn_mod1__another_private"));
    }

    /// RC-1k: compute_needed_functions includes transitive dependencies
    #[test]
    fn test_compute_needed_functions_transitive() {
        use super::super::ir::{IrFunction, IrInst, IrModule};

        let mut module = IrModule::new();

        // public_uses_private: calls _private_helper via CallUser
        let mut pub_fn = IrFunction::new_with_params(
            "_taida_fn_mod1_public_uses_private".to_string(),
            vec!["x".to_string()],
        );
        pub_fn.push(IrInst::CallUser(
            1,
            "_taida_fn_mod1__private_helper".to_string(),
            vec![0],
        ));
        module.functions.push(pub_fn);

        // _private_helper: needed transitively
        let private_fn = IrFunction::new_with_params(
            "_taida_fn_mod1__private_helper".to_string(),
            vec!["x".to_string()],
        );
        module.functions.push(private_fn);

        // _another_private: NOT needed
        let another_fn = IrFunction::new_with_params(
            "_taida_fn_mod1__another_private".to_string(),
            vec!["y".to_string()],
        );
        module.functions.push(another_fn);

        let requested = vec!["_taida_fn_mod1_public_uses_private".to_string()];
        let needed = compute_needed_functions(&module, &requested);

        assert!(needed.contains("_taida_fn_mod1_public_uses_private"));
        assert!(
            needed.contains("_taida_fn_mod1__private_helper"),
            "transitive dependency should be included"
        );
        assert!(
            !needed.contains("_taida_fn_mod1__another_private"),
            "unrelated private function should NOT be included"
        );
    }

    /// RC-1k: compute_needed_functions always includes init functions
    #[test]
    fn test_compute_needed_functions_init() {
        use super::super::ir::{IrFunction, IrModule};

        let mut module = IrModule::new();

        let init_fn = IrFunction::new("_taida_init_mod1".to_string());
        module.functions.push(init_fn);

        let public_fn = IrFunction::new("_taida_fn_mod1_public".to_string());
        module.functions.push(public_fn);

        let private_fn = IrFunction::new("_taida_fn_mod1_private".to_string());
        module.functions.push(private_fn);

        let requested = vec!["_taida_fn_mod1_public".to_string()];
        let needed = compute_needed_functions(&module, &requested);

        assert!(needed.contains("_taida_fn_mod1_public"));
        assert!(
            needed.contains("_taida_init_mod1"),
            "init function should always be included"
        );
        assert!(!needed.contains("_taida_fn_mod1_private"));
    }

    /// 5a: CondBranch 内の関数参照が推移的に追跡されるテスト
    #[test]
    fn test_compute_needed_functions_cond_branch_refs() {
        use super::super::ir::{CondArm, IrFunction, IrInst, IrModule};

        let mut module = IrModule::new();

        // func_a: CondBranch 内で func_b を呼ぶ
        let mut func_a = IrFunction::new("_taida_fn_mod1_func_a".to_string());
        func_a.push(IrInst::CondBranch(
            0,
            vec![CondArm {
                condition: Some(0),
                body: vec![IrInst::CallUser(
                    1,
                    "_taida_fn_mod1_func_b".to_string(),
                    vec![0],
                )],
                result: 1,
            }],
        ));
        module.functions.push(func_a);

        // func_b: leaf function
        let func_b = IrFunction::new("_taida_fn_mod1_func_b".to_string());
        module.functions.push(func_b);

        // func_c: unrelated
        let func_c = IrFunction::new("_taida_fn_mod1_func_c".to_string());
        module.functions.push(func_c);

        let requested = vec!["_taida_fn_mod1_func_a".to_string()];
        let needed = compute_needed_functions(&module, &requested);

        assert!(needed.contains("_taida_fn_mod1_func_a"));
        assert!(
            needed.contains("_taida_fn_mod1_func_b"),
            "function referenced inside CondBranch should be included"
        );
        assert!(
            !needed.contains("_taida_fn_mod1_func_c"),
            "unrelated function should NOT be included"
        );
    }

    /// 5a: 循環参照（A -> B -> A）で BFS が安全に終了するテスト
    #[test]
    fn test_compute_needed_functions_circular_ref() {
        use super::super::ir::{IrFunction, IrInst, IrModule};

        let mut module = IrModule::new();

        // func_a calls func_b
        let mut func_a = IrFunction::new("_taida_fn_mod1_func_a".to_string());
        func_a.push(IrInst::CallUser(
            1,
            "_taida_fn_mod1_func_b".to_string(),
            vec![0],
        ));
        module.functions.push(func_a);

        // func_b calls func_a (circular)
        let mut func_b = IrFunction::new("_taida_fn_mod1_func_b".to_string());
        func_b.push(IrInst::CallUser(
            1,
            "_taida_fn_mod1_func_a".to_string(),
            vec![0],
        ));
        module.functions.push(func_b);

        let requested = vec!["_taida_fn_mod1_func_a".to_string()];
        let needed = compute_needed_functions(&module, &requested);

        assert!(needed.contains("_taida_fn_mod1_func_a"));
        assert!(
            needed.contains("_taida_fn_mod1_func_b"),
            "circular dependency should be resolved without infinite loop"
        );
        // Test passes if it terminates (no infinite loop)
    }

    /// 5a: 空の requested_syms では init 関数のみが返るテスト
    #[test]
    fn test_compute_needed_functions_empty_requested() {
        use super::super::ir::{IrFunction, IrModule};

        let mut module = IrModule::new();

        let init_fn = IrFunction::new("_taida_init_mod1".to_string());
        module.functions.push(init_fn);

        let public_fn = IrFunction::new("_taida_fn_mod1_public".to_string());
        module.functions.push(public_fn);

        let private_fn = IrFunction::new("_taida_fn_mod1_private".to_string());
        module.functions.push(private_fn);

        let requested: Vec<String> = vec![];
        let needed = compute_needed_functions(&module, &requested);

        assert!(
            needed.contains("_taida_init_mod1"),
            "init function should always be included even with empty requested_syms"
        );
        assert!(
            !needed.contains("_taida_fn_mod1_public"),
            "non-init functions should NOT be included when nothing is requested"
        );
        assert!(!needed.contains("_taida_fn_mod1_private"));
    }

    // -----------------------------------------------------------------------
    // S-4: WasmRuntimeCache unit tests
    // -----------------------------------------------------------------------

    // D-2: Mutex to serialize tests that mutate TAIDA_WASM_RT_CACHE env var,
    // preventing races when cargo test runs them in parallel.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// D-2: RAII guard for env var mutation. Saves the current value on
    /// construction and restores it on drop.
    struct EnvGuard {
        key: &'static str,
        saved: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new(key: &'static str) -> Self {
            let lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
            let saved = std::env::var(key).ok();
            Self {
                key,
                saved,
                _lock: lock,
            }
        }

        fn set(&self, value: &str) {
            // SAFETY: serialized by ENV_MUTEX
            unsafe { std::env::set_var(self.key, value) }
        }

        fn remove(&self) {
            // SAFETY: serialized by ENV_MUTEX
            unsafe { std::env::remove_var(self.key) }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.saved {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    /// S-4: cache_key produces different keys for different source content.
    #[test]
    fn test_cache_key_differs_on_source_change() {
        use std::path::PathBuf;

        let cache_dir = PathBuf::from("target/test-wasm-cache-key");
        let _ = std::fs::create_dir_all(&cache_dir);

        // We cannot easily construct a WasmRuntimeCache without clang,
        // so test the FNV-1a logic directly with the same algorithm.
        fn fnv1a_cache_key(source: &str, version: &str) -> String {
            let mut state: u64 = 0xcbf29ce484222325;
            for byte in source.bytes() {
                state ^= byte as u64;
                state = state.wrapping_mul(0x100000001b3);
            }
            for byte in version.bytes() {
                state ^= byte as u64;
                state = state.wrapping_mul(0x100000001b3);
            }
            // L-1: Use the same shared constant as cache_key()
            for flag in WASM_CLANG_FLAGS {
                for byte in flag.bytes() {
                    state ^= byte as u64;
                    state = state.wrapping_mul(0x100000001b3);
                }
            }
            format!("{:016x}", state)
        }

        let key_a = fnv1a_cache_key("int main() { return 0; }", "clang 17.0.0");
        let key_b = fnv1a_cache_key("int main() { return 1; }", "clang 17.0.0");
        let key_c = fnv1a_cache_key("int main() { return 0; }", "clang 18.0.0");

        assert_ne!(
            key_a, key_b,
            "different source should produce different keys"
        );
        assert_ne!(
            key_a, key_c,
            "different clang version should produce different keys"
        );

        // Same inputs should produce the same key
        let key_a2 = fnv1a_cache_key("int main() { return 0; }", "clang 17.0.0");
        assert_eq!(key_a, key_a2, "same inputs should produce identical keys");

        // Key should be a 16-char hex string
        assert_eq!(key_a.len(), 16, "cache key should be 16 hex chars");
        assert!(
            key_a.chars().all(|c| c.is_ascii_hexdigit()),
            "cache key should only contain hex digits"
        );

        let _ = std::fs::remove_dir_all(&cache_dir);
    }

    /// S-4: default_wasm_cache_dir respects environment variable priority.
    #[test]
    fn test_default_wasm_cache_dir_env_override() {
        let guard = EnvGuard::new("TAIDA_WASM_RT_CACHE");
        guard.set("/tmp/test-wasm-cache-override");

        let dir = default_wasm_cache_dir(Some(Path::new("/some/project")));
        assert_eq!(
            dir,
            PathBuf::from("/tmp/test-wasm-cache-override"),
            "env variable should take highest priority"
        );
        // guard restores env on drop
    }

    /// S-4: default_wasm_cache_dir falls back to target/ when no .taida/ exists.
    #[test]
    fn test_default_wasm_cache_dir_fallback() {
        let guard = EnvGuard::new("TAIDA_WASM_RT_CACHE");
        guard.remove();

        let tmp = PathBuf::from("target/test-cache-dir-no-taida");
        let _ = std::fs::create_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(tmp.join(".taida"));

        let dir = default_wasm_cache_dir(Some(&tmp));
        assert_eq!(
            dir,
            PathBuf::from("target/wasm-rt-cache"),
            "should fall back to target/wasm-rt-cache when .taida/ does not exist"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        // guard restores env on drop
    }

    /// S-4: default_wasm_cache_dir uses .taida/cache/wasm-rt/ when project root found.
    #[test]
    fn test_default_wasm_cache_dir_taida_dir() {
        let guard = EnvGuard::new("TAIDA_WASM_RT_CACHE");
        guard.remove();

        let tmp = PathBuf::from("target/test-cache-dir-taida");
        let taida_dir = tmp.join(".taida");
        let _ = std::fs::create_dir_all(&taida_dir);
        // packages.tdm is the project marker required alongside .taida/
        let _ = std::fs::write(tmp.join("packages.tdm"), "");

        let dir = default_wasm_cache_dir(Some(&tmp));
        assert_eq!(
            dir,
            taida_dir.join("cache").join("wasm-rt"),
            "should use .taida/cache/wasm-rt/ when project root found"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        // guard restores env on drop
    }

    /// S-4: .taida/ without packages.tdm falls back to target/wasm-rt-cache.
    #[test]
    fn test_default_wasm_cache_dir_taida_without_manifest() {
        let guard = EnvGuard::new("TAIDA_WASM_RT_CACHE");
        guard.remove();

        let tmp = PathBuf::from("target/test-cache-dir-no-manifest");
        let _ = std::fs::create_dir_all(tmp.join(".taida"));
        // No packages.tdm — not a Taida project root
        let _ = std::fs::remove_file(tmp.join("packages.tdm"));

        let dir = default_wasm_cache_dir(Some(&tmp));
        assert_eq!(
            dir,
            PathBuf::from("target/wasm-rt-cache"),
            "should fall back when .taida/ exists but packages.tdm is missing"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        // guard restores env on drop
    }

    /// RCB-56: default_wasm_cache_dir walks up parent directories to find project root.
    #[test]
    fn test_default_wasm_cache_dir_parent_traversal() {
        let guard = EnvGuard::new("TAIDA_WASM_RT_CACHE");
        guard.remove();

        // Create proj/.taida/ + proj/packages.tdm and proj/src/deep/
        let tmp = PathBuf::from("target/test-cache-dir-nested");
        let taida_dir = tmp.join(".taida");
        let nested = tmp.join("src").join("deep");
        let _ = std::fs::create_dir_all(&taida_dir);
        let _ = std::fs::create_dir_all(&nested);
        let _ = std::fs::write(tmp.join("packages.tdm"), "");

        // Pass the nested subdirectory — should still find proj/.taida/
        let dir = default_wasm_cache_dir(Some(&nested));
        assert_eq!(
            dir,
            taida_dir.join("cache").join("wasm-rt"),
            "should find project root by walking up from subdirectory"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        // guard restores env on drop
    }

    /// RCB-56: Does not pick up ancestor .taida/ without packages.tdm.
    #[test]
    fn test_default_wasm_cache_dir_ignores_non_project_taida() {
        let guard = EnvGuard::new("TAIDA_WASM_RT_CACHE");
        guard.remove();

        // ancestor/.taida/ exists but no packages.tdm — not a project root
        let tmp = PathBuf::from("target/test-cache-dir-ancestor");
        let _ = std::fs::create_dir_all(tmp.join(".taida"));
        let nested = tmp.join("sub").join("deep");
        let _ = std::fs::create_dir_all(&nested);
        // No packages.tdm anywhere

        let dir = default_wasm_cache_dir(Some(&nested));
        assert_eq!(
            dir,
            PathBuf::from("target/wasm-rt-cache"),
            "should not pick up ancestor .taida/ without packages.tdm"
        );

        let _ = std::fs::remove_dir_all(&tmp);
        // guard restores env on drop
    }
}
