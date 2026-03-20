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
            let dep_path =
                resolve_module_path(&importer_dir, &module_path, version.as_deref());
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
            return PathBuf::from(format!(
                "<path traversal rejected: {}>",
                module_path
            ));
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
    vec![
        "--target=wasm32-unknown-wasi".to_string(),
        "-nostdlib".to_string(),
        "-O2".to_string(),
        "-c".to_string(),
        "-I".to_string(),
        include_dir.display().to_string(),
    ]
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
        let dep_path =
            resolve_module_path(&importer_dir, &module_path, version.as_deref());
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
            // 差分シンボルがある場合: モジュールを再パースし差分の推移的依存のみを追加融合
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

            // 差分シンボルの推移的依存のみを計算
            let needed = compute_needed_functions(&dep_ir, &new_syms);
            // 既に融合済みの関数は除外して追加
            for func in dep_ir.functions {
                if !needed.contains(&func.name) {
                    continue;
                }
                if !main_module.functions.iter().any(|f| f.name == func.name) {
                    main_module.functions.push(func);
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

        for func in dep_ir.functions {
            if !needed.contains(&func.name) {
                continue; // 非公開・不要な関数はスキップ
            }
            if !main_module.functions.iter().any(|f| f.name == func.name) {
                main_module.functions.push(func);
            }
        }

        // 融合済みシンボルを記録
        compiled.insert(canonical, needed);

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

/// .td ファイルを wasm-min ターゲットでコンパイルし .wasm を生成する
///
/// モジュールインポート対応: 依存モジュールを IR レベルでインライン展開し、
/// 単一の IR モジュールとして C emit に渡す (Option C: AST/IR インライン展開)。
/// パイプライン: .td → parse → IR → (依存 IR 融合) → C source → clang(wasm32) → .o → wasm-ld → .wasm
///
/// Cranelift の ISA に wasm32 が存在しないため、IR → C → clang ルートを採用する。
pub fn compile_file_wasm(
    input_path: &Path,
    output_path: Option<&Path>,
) -> Result<PathBuf, CompileError> {
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

    // wasm-min モジュールインライン展開: 依存モジュールの IR 関数をメインモジュールに融合
    if !ir_module.imports.is_empty() {
        let base_dir = input_path.parent().unwrap_or(Path::new("."));
        inline_wasm_module_imports(&mut ir_module, base_dir, input_path)?;
    }

    // RC 最適化パス（retain/release は C emitter で無視されるが、IR を整える）
    rc_opt::optimize(&mut ir_module);

    // IR → C ソースコード
    let generated_c =
        emit_wasm_c::emit_c(&ir_module, emit_wasm_c::WasmProfile::Min).map_err(|e| {
            CompileError {
                message: format!("wasm-min C emission failed: {}", e),
            }
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

    // 一時ファイルパスの生成（出力パスベースで一意化し、並列コンパイルの衝突を防ぐ）
    let tmp_base = wasm_path.with_extension("_wasm_tmp");
    let gen_c_path = tmp_base.with_extension("gen.c");
    let gen_obj_path = tmp_base.with_extension("gen.o");
    let rt_c_path = tmp_base.with_extension("rt.c");
    let rt_obj_path = tmp_base.with_extension("rt.o");
    let include_dir = tmp_base.with_extension("include");

    // 生成された C ソースを書き出し
    fs::write(&gen_c_path, &generated_c).map_err(|e| CompileError {
        message: format!("failed to write generated C: {}", e),
    })?;

    // runtime_core_wasm.c を書き出し
    let rt_source = include_str!("runtime_core_wasm.c");
    fs::write(&rt_c_path, rt_source).map_err(|e| CompileError {
        message: format!("failed to write wasm runtime source: {}", e),
    })?;
    let clang = find_clang_for_wasm()?;
    if let Err(err) = write_wasm_stdint_header(&include_dir) {
        let _ = fs::remove_file(&gen_c_path);
        let _ = fs::remove_file(&rt_c_path);
        return Err(err);
    }

    // 生成 C をコンパイル
    let clang_args = wasm_clang_base_args(&include_dir);

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
        // C ソースをデバッグ用に残す
        let _ = fs::remove_file(&rt_c_path);
        let _ = fs::remove_file(&rt_obj_path);
        return Err(CompileError {
            message: format!(
                "clang wasm32 compilation of generated code failed (source preserved at: {}; shim preserved at: {})",
                gen_c_path.display(),
                include_dir.display()
            ),
        });
    }

    // runtime をコンパイル
    let rt_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &rt_c_path,
        &rt_obj_path,
        &include_dir,
        &[&gen_c_path, &rt_c_path, &gen_obj_path],
        true,
        None,
    )?;

    let _ = fs::remove_file(&gen_c_path);
    let _ = fs::remove_file(&rt_c_path);
    let _ = fs::remove_dir_all(&include_dir);

    if !rt_status.success() {
        let _ = fs::remove_file(&gen_obj_path);
        let _ = fs::remove_file(&rt_obj_path);
        return Err(CompileError {
            message: "clang wasm32 compilation of runtime failed.".to_string(),
        });
    }

    // wasm-ld でリンク
    let wasm_ld = find_wasm_ld()?;
    let ld_status = Command::new(&wasm_ld)
        .args([
            "--no-entry",
            "--export=_start",
            "--strip-all",
            "--gc-sections",
        ])
        .arg(&rt_obj_path)
        .arg(&gen_obj_path)
        .arg("-o")
        .arg(&wasm_path)
        .status()
        .map_err(|e| {
            let _ = fs::remove_file(&gen_obj_path);
            let _ = fs::remove_file(&rt_obj_path);
            CompileError {
                message: format!("wasm-ld invocation failed: {}", e),
            }
        })?;

    // 一時ファイルの削除
    let _ = fs::remove_file(&gen_obj_path);
    let _ = fs::remove_file(&rt_obj_path);

    if !ld_status.success() {
        return Err(CompileError {
            message: format!("wasm-ld failed with exit code: {:?}", ld_status.code()),
        });
    }

    Ok(wasm_path)
}

// ---------------------------------------------------------------------------
// WW-2: wasm-wasi コンパイルパス
// ---------------------------------------------------------------------------

/// .td ファイルを wasm-wasi ターゲットでコンパイルし .wasm を生成する
///
/// wasm-wasi は wasm-min の上位互換で、WASI I/O (env, file read/write) を追加する。
/// パイプライン: .td → parse → IR → C source → clang(wasm32) → .o → wasm-ld → .wasm
///
/// リンク構成: gen.o + rt_core.o + rt_wasi.o → wasm-ld → output.wasm
pub fn compile_file_wasm_wasi(
    input_path: &Path,
    output_path: Option<&Path>,
) -> Result<PathBuf, CompileError> {
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

    // wasm-wasi モジュールインライン展開
    if !ir_module.imports.is_empty() {
        let base_dir = input_path.parent().unwrap_or(Path::new("."));
        inline_wasm_module_imports(&mut ir_module, base_dir, input_path)?;
    }

    // RC 最適化パス
    rc_opt::optimize(&mut ir_module);

    // IR → C ソースコード (wasm-wasi profile: OS API prototypes allowed)
    let generated_c =
        emit_wasm_c::emit_c(&ir_module, emit_wasm_c::WasmProfile::Wasi).map_err(|e| {
            CompileError {
                message: format!("wasm-wasi C emission failed: {}", e),
            }
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

    // 一時ファイルパスの生成
    let tmp_base = wasm_path.with_extension("_wasm_wasi_tmp");
    let gen_c_path = tmp_base.with_extension("gen.c");
    let gen_obj_path = tmp_base.with_extension("gen.o");
    let rt_core_c_path = tmp_base.with_extension("rt_core.c");
    let rt_core_obj_path = tmp_base.with_extension("rt_core.o");
    let rt_wasi_c_path = tmp_base.with_extension("rt_wasi.c");
    let rt_wasi_obj_path = tmp_base.with_extension("rt_wasi.o");
    let include_dir = tmp_base.with_extension("include");

    // 生成された C ソースを書き出し
    fs::write(&gen_c_path, &generated_c).map_err(|e| CompileError {
        message: format!("failed to write generated C: {}", e),
    })?;

    // runtime_core_wasm.c を書き出し（凍結、wasm-min と同一）
    let rt_core_source = include_str!("runtime_core_wasm.c");
    fs::write(&rt_core_c_path, rt_core_source).map_err(|e| CompileError {
        message: format!("failed to write wasm core runtime source: {}", e),
    })?;

    // runtime_wasi_io.c を書き出し（wasm-wasi 専用 I/O 層）
    let rt_wasi_source = include_str!("runtime_wasi_io.c");
    fs::write(&rt_wasi_c_path, rt_wasi_source).map_err(|e| CompileError {
        message: format!("failed to write wasm WASI I/O source: {}", e),
    })?;
    let clang = find_clang_for_wasm()?;
    if let Err(err) = write_wasm_stdint_header(&include_dir) {
        let _ = fs::remove_file(&gen_c_path);
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_wasi_c_path);
        return Err(err);
    }

    // 生成 C をコンパイル
    let clang_args = wasm_clang_base_args(&include_dir);

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
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_wasi_c_path);
        return Err(CompileError {
            message: format!(
                "clang wasm32 compilation of generated code failed (source preserved at: {}; shim preserved at: {})",
                gen_c_path.display(),
                include_dir.display()
            ),
        });
    }

    // runtime core をコンパイル
    let rt_core_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &rt_core_c_path,
        &rt_core_obj_path,
        &include_dir,
        &[&gen_c_path, &gen_obj_path, &rt_core_c_path, &rt_wasi_c_path],
        true,
        None,
    )?;

    if !rt_core_status.success() {
        let _ = fs::remove_file(&gen_c_path);
        let _ = fs::remove_file(&gen_obj_path);
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_wasi_c_path);
        let _ = fs::remove_dir_all(&include_dir);
        return Err(CompileError {
            message: "clang wasm32 compilation of core runtime failed.".to_string(),
        });
    }

    // runtime WASI I/O をコンパイル
    let rt_wasi_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &rt_wasi_c_path,
        &rt_wasi_obj_path,
        &include_dir,
        &[
            &gen_c_path,
            &gen_obj_path,
            &rt_core_c_path,
            &rt_core_obj_path,
            &rt_wasi_c_path,
        ],
        true,
        None,
    )?;

    // 一時 C ソースを削除
    let _ = fs::remove_file(&gen_c_path);
    let _ = fs::remove_file(&rt_core_c_path);
    let _ = fs::remove_file(&rt_wasi_c_path);
    let _ = fs::remove_dir_all(&include_dir);

    if !rt_wasi_status.success() {
        let _ = fs::remove_file(&gen_obj_path);
        let _ = fs::remove_file(&rt_core_obj_path);
        let _ = fs::remove_file(&rt_wasi_obj_path);
        return Err(CompileError {
            message: "clang wasm32 compilation of WASI I/O runtime failed.".to_string(),
        });
    }

    // wasm-ld でリンク（3 object: gen.o + rt_core.o + rt_wasi.o）
    let wasm_ld = find_wasm_ld()?;
    let ld_status = Command::new(&wasm_ld)
        .args([
            "--no-entry",
            "--export=_start",
            "--strip-all",
            "--gc-sections",
        ])
        .arg(&rt_core_obj_path)
        .arg(&rt_wasi_obj_path)
        .arg(&gen_obj_path)
        .arg("-o")
        .arg(&wasm_path)
        .status()
        .map_err(|e| {
            let _ = fs::remove_file(&gen_obj_path);
            let _ = fs::remove_file(&rt_core_obj_path);
            let _ = fs::remove_file(&rt_wasi_obj_path);
            CompileError {
                message: format!("wasm-ld invocation failed: {}", e),
            }
        })?;

    // 一時ファイルの削除
    let _ = fs::remove_file(&gen_obj_path);
    let _ = fs::remove_file(&rt_core_obj_path);
    let _ = fs::remove_file(&rt_wasi_obj_path);

    if !ld_status.success() {
        return Err(CompileError {
            message: format!("wasm-ld failed with exit code: {:?}", ld_status.code()),
        });
    }

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

    // wasm-edge モジュールインライン展開
    if !ir_module.imports.is_empty() {
        let base_dir = input_path.parent().unwrap_or(Path::new("."));
        inline_wasm_module_imports(&mut ir_module, base_dir, input_path)?;
    }

    // RC 最適化パス
    rc_opt::optimize(&mut ir_module);

    // IR -> C ソースコード (wasm-edge profile: env API allowed, file I/O rejected)
    let generated_c =
        emit_wasm_c::emit_c(&ir_module, emit_wasm_c::WasmProfile::Edge).map_err(|e| {
            CompileError {
                message: format!("wasm-edge C emission failed: {}", e),
            }
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

    // 一時ファイルパスの生成
    let tmp_base = wasm_path.with_extension("_wasm_edge_tmp");
    let gen_c_path = tmp_base.with_extension("gen.c");
    let gen_obj_path = tmp_base.with_extension("gen.o");
    let rt_core_c_path = tmp_base.with_extension("rt_core.c");
    let rt_core_obj_path = tmp_base.with_extension("rt_core.o");
    let rt_edge_c_path = tmp_base.with_extension("rt_edge.c");
    let rt_edge_obj_path = tmp_base.with_extension("rt_edge.o");
    let include_dir = tmp_base.with_extension("include");

    // 生成された C ソースを書き出し
    fs::write(&gen_c_path, &generated_c).map_err(|e| CompileError {
        message: format!("failed to write generated C: {}", e),
    })?;

    // runtime_core_wasm.c を書き出し（凍結、全 profile 共通）
    let rt_core_source = include_str!("runtime_core_wasm.c");
    fs::write(&rt_core_c_path, rt_core_source).map_err(|e| CompileError {
        message: format!("failed to write wasm core runtime source: {}", e),
    })?;

    // runtime_edge_host.c を書き出し（wasm-edge 専用 host import 層）
    let rt_edge_source = include_str!("runtime_edge_host.c");
    fs::write(&rt_edge_c_path, rt_edge_source).map_err(|e| CompileError {
        message: format!("failed to write wasm edge host source: {}", e),
    })?;
    let clang = find_clang_for_wasm()?;
    if let Err(err) = write_wasm_stdint_header(&include_dir) {
        let _ = fs::remove_file(&gen_c_path);
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_edge_c_path);
        return Err(err);
    }

    // 生成 C をコンパイル
    let clang_args = wasm_clang_base_args(&include_dir);

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
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_edge_c_path);
        return Err(CompileError {
            message: format!(
                "clang wasm32 compilation of generated code failed (source preserved at: {}; shim preserved at: {})",
                gen_c_path.display(),
                include_dir.display()
            ),
        });
    }

    // runtime core をコンパイル
    let rt_core_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &rt_core_c_path,
        &rt_core_obj_path,
        &include_dir,
        &[&gen_c_path, &gen_obj_path, &rt_core_c_path, &rt_edge_c_path],
        true,
        None,
    )?;

    if !rt_core_status.success() {
        let _ = fs::remove_file(&gen_c_path);
        let _ = fs::remove_file(&gen_obj_path);
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_edge_c_path);
        let _ = fs::remove_dir_all(&include_dir);
        return Err(CompileError {
            message: "clang wasm32 compilation of core runtime failed.".to_string(),
        });
    }

    // runtime edge host をコンパイル
    let rt_edge_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &rt_edge_c_path,
        &rt_edge_obj_path,
        &include_dir,
        &[
            &gen_c_path,
            &gen_obj_path,
            &rt_core_c_path,
            &rt_core_obj_path,
            &rt_edge_c_path,
        ],
        true,
        None,
    )?;

    // 一時 C ソースを削除
    let _ = fs::remove_file(&gen_c_path);
    let _ = fs::remove_file(&rt_core_c_path);
    let _ = fs::remove_file(&rt_edge_c_path);
    let _ = fs::remove_dir_all(&include_dir);

    if !rt_edge_status.success() {
        let _ = fs::remove_file(&gen_obj_path);
        let _ = fs::remove_file(&rt_core_obj_path);
        let _ = fs::remove_file(&rt_edge_obj_path);
        return Err(CompileError {
            message: "clang wasm32 compilation of edge host runtime failed.".to_string(),
        });
    }

    // wasm-ld でリンク（3 object: gen.o + rt_core.o + rt_edge.o）
    let wasm_ld = find_wasm_ld()?;
    let ld_status = Command::new(&wasm_ld)
        .args([
            "--no-entry",
            "--export=_start",
            "--strip-all",
            "--gc-sections",
        ])
        .arg(&rt_core_obj_path)
        .arg(&rt_edge_obj_path)
        .arg(&gen_obj_path)
        .arg("-o")
        .arg(&wasm_path)
        .status()
        .map_err(|e| {
            let _ = fs::remove_file(&gen_obj_path);
            let _ = fs::remove_file(&rt_core_obj_path);
            let _ = fs::remove_file(&rt_edge_obj_path);
            CompileError {
                message: format!("wasm-ld invocation failed: {}", e),
            }
        })?;

    // 一時ファイルの削除
    let _ = fs::remove_file(&gen_obj_path);
    let _ = fs::remove_file(&rt_core_obj_path);
    let _ = fs::remove_file(&rt_edge_obj_path);

    if !ld_status.success() {
        return Err(CompileError {
            message: format!("wasm-ld failed with exit code: {:?}", ld_status.code()),
        });
    }

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

    // wasm-full モジュールインライン展開
    if !ir_module.imports.is_empty() {
        let base_dir = input_path.parent().unwrap_or(Path::new("."));
        inline_wasm_module_imports(&mut ir_module, base_dir, input_path)?;
    }

    // RC 最適化パス
    rc_opt::optimize(&mut ir_module);

    // IR -> C ソースコード (wasm-full profile)
    let generated_c =
        emit_wasm_c::emit_c(&ir_module, emit_wasm_c::WasmProfile::Full).map_err(|e| {
            CompileError {
                message: format!("wasm-full C emission failed: {}", e),
            }
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

    // 一時ファイルパスの生成
    let tmp_base = wasm_path.with_extension("_wasm_full_tmp");
    let gen_c_path = tmp_base.with_extension("gen.c");
    let gen_obj_path = tmp_base.with_extension("gen.o");
    let rt_core_c_path = tmp_base.with_extension("rt_core.c");
    let rt_core_obj_path = tmp_base.with_extension("rt_core.o");
    let rt_wasi_c_path = tmp_base.with_extension("rt_wasi.c");
    let rt_wasi_obj_path = tmp_base.with_extension("rt_wasi.o");
    let rt_full_c_path = tmp_base.with_extension("rt_full.c");
    let rt_full_obj_path = tmp_base.with_extension("rt_full.o");
    let include_dir = tmp_base.with_extension("include");

    // 生成された C ソースを書き出し
    fs::write(&gen_c_path, &generated_c).map_err(|e| CompileError {
        message: format!("failed to write generated C: {}", e),
    })?;

    // runtime_core_wasm.c (凍結)
    let rt_core_source = include_str!("runtime_core_wasm.c");
    fs::write(&rt_core_c_path, rt_core_source).map_err(|e| CompileError {
        message: format!("failed to write wasm core runtime source: {}", e),
    })?;

    // runtime_wasi_io.c (wasm-wasi I/O 層)
    let rt_wasi_source = include_str!("runtime_wasi_io.c");
    fs::write(&rt_wasi_c_path, rt_wasi_source).map_err(|e| CompileError {
        message: format!("failed to write wasm WASI I/O source: {}", e),
    })?;

    // runtime_full_wasm.c (wasm-full 拡張層)
    let rt_full_source = include_str!("runtime_full_wasm.c");
    fs::write(&rt_full_c_path, rt_full_source).map_err(|e| CompileError {
        message: format!("failed to write wasm full runtime source: {}", e),
    })?;
    let clang = find_clang_for_wasm()?;
    if let Err(err) = write_wasm_stdint_header(&include_dir) {
        let _ = fs::remove_file(&gen_c_path);
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_wasi_c_path);
        let _ = fs::remove_file(&rt_full_c_path);
        return Err(err);
    }

    // 生成 C をコンパイル
    let clang_args = wasm_clang_base_args(&include_dir);

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
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_wasi_c_path);
        let _ = fs::remove_file(&rt_full_c_path);
        return Err(CompileError {
            message: format!(
                "clang wasm32 compilation of generated code failed (source preserved at: {}; shim preserved at: {})",
                gen_c_path.display(),
                include_dir.display()
            ),
        });
    }

    // runtime core をコンパイル
    let rt_core_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &rt_core_c_path,
        &rt_core_obj_path,
        &include_dir,
        &[
            &gen_c_path,
            &gen_obj_path,
            &rt_core_c_path,
            &rt_wasi_c_path,
            &rt_full_c_path,
        ],
        true,
        None,
    )?;

    if !rt_core_status.success() {
        let _ = fs::remove_file(&gen_c_path);
        let _ = fs::remove_file(&gen_obj_path);
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_wasi_c_path);
        let _ = fs::remove_file(&rt_full_c_path);
        let _ = fs::remove_dir_all(&include_dir);
        return Err(CompileError {
            message: "clang wasm32 compilation of core runtime failed.".to_string(),
        });
    }

    // runtime WASI I/O をコンパイル
    let rt_wasi_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &rt_wasi_c_path,
        &rt_wasi_obj_path,
        &include_dir,
        &[
            &gen_c_path,
            &gen_obj_path,
            &rt_core_c_path,
            &rt_core_obj_path,
            &rt_wasi_c_path,
            &rt_full_c_path,
        ],
        true,
        None,
    )?;

    if !rt_wasi_status.success() {
        let _ = fs::remove_file(&gen_c_path);
        let _ = fs::remove_file(&gen_obj_path);
        let _ = fs::remove_file(&rt_core_obj_path);
        let _ = fs::remove_file(&rt_core_c_path);
        let _ = fs::remove_file(&rt_wasi_c_path);
        let _ = fs::remove_file(&rt_full_c_path);
        let _ = fs::remove_dir_all(&include_dir);
        return Err(CompileError {
            message: "clang wasm32 compilation of WASI I/O runtime failed.".to_string(),
        });
    }

    // runtime full をコンパイル
    let rt_full_status = run_wasm_clang_object(
        &clang,
        &clang_args,
        &rt_full_c_path,
        &rt_full_obj_path,
        &include_dir,
        &[
            &gen_c_path,
            &gen_obj_path,
            &rt_core_c_path,
            &rt_core_obj_path,
            &rt_wasi_c_path,
            &rt_wasi_obj_path,
            &rt_full_c_path,
        ],
        true,
        None,
    )?;

    // 一時 C ソースを削除
    let _ = fs::remove_file(&gen_c_path);
    let _ = fs::remove_file(&rt_core_c_path);
    let _ = fs::remove_file(&rt_wasi_c_path);
    let _ = fs::remove_file(&rt_full_c_path);
    let _ = fs::remove_dir_all(&include_dir);

    if !rt_full_status.success() {
        let _ = fs::remove_file(&gen_obj_path);
        let _ = fs::remove_file(&rt_core_obj_path);
        let _ = fs::remove_file(&rt_wasi_obj_path);
        let _ = fs::remove_file(&rt_full_obj_path);
        return Err(CompileError {
            message: "clang wasm32 compilation of full runtime failed.".to_string(),
        });
    }

    // wasm-ld でリンク（4 object: gen.o + rt_core.o + rt_wasi.o + rt_full.o）
    let wasm_ld = find_wasm_ld()?;
    let ld_status = Command::new(&wasm_ld)
        .args([
            "--no-entry",
            "--export=_start",
            "--strip-all",
            "--gc-sections",
        ])
        .arg(&rt_core_obj_path)
        .arg(&rt_wasi_obj_path)
        .arg(&rt_full_obj_path)
        .arg(&gen_obj_path)
        .arg("-o")
        .arg(&wasm_path)
        .status()
        .map_err(|e| {
            let _ = fs::remove_file(&gen_obj_path);
            let _ = fs::remove_file(&rt_core_obj_path);
            let _ = fs::remove_file(&rt_wasi_obj_path);
            let _ = fs::remove_file(&rt_full_obj_path);
            CompileError {
                message: format!("wasm-ld invocation failed: {}", e),
            }
        })?;

    // 一時ファイルの削除
    let _ = fs::remove_file(&gen_obj_path);
    let _ = fs::remove_file(&rt_core_obj_path);
    let _ = fs::remove_file(&rt_wasi_obj_path);
    let _ = fs::remove_file(&rt_full_obj_path);

    if !ld_status.success() {
        return Err(CompileError {
            message: format!("wasm-ld failed with exit code: {:?}", ld_status.code()),
        });
    }

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
}
