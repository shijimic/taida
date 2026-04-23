// C12B-024: `src/codegen/lower.rs` split into submodules (FB-21 / C12-9 Step 2).
// C13-2: Further mechanical split carried over from C12B-024 — the
// `taida-lang/net` surface moved to `lower/net.rs`, the `taida-lang/os`
// + `taida-lang/pool` surfaces moved to `lower/os.rs`. `stdlib.rs`
// now retains only the stdlib IO / crypto / field-tag registry helpers.
//
// This `mod.rs` keeps the module-level types (`Lowering`, `LowerError`,
// `AddonFuncRef`, `AddonFacadeSummary`, `ImportedSymbolKind`,
// `InheritanceChainFields`), the helper constants, the free function
// `simple_hash`, and the trivial `impl Display` / `impl Default` blocks.
// The `impl Lowering` method set is distributed across the submodules
// listed below per placement table §2 of
// `.dev/taida-logs/docs/design/file_boundaries.md`.

//! AST → Taida IR 変換（Lowering）
//!
//! Module-level declarations (struct `Lowering`, error types, addon
//! facade types, free helpers). The `impl Lowering` method set lives in
//! the submodules `core` / `imports` / `stdlib` / `net` / `os` /
//! `molds` / `stmt` / `expr` / `infer` / `tag_prop` per placement
//! table §2 (extended by C13-2).

use super::ir::*;
use crate::parser::*;

/// 簡易フィールド名ハッシュ（FNV-1a）
pub(crate) fn simple_hash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

const OS_NET_DEFAULT_TIMEOUT_MS: i64 = 30_000;

pub struct Lowering {
    /// ユーザー定義関数名のセット
    pub(crate) user_funcs: std::collections::HashSet<String>,
    /// 関数名 → パラメータ定義（実効デフォルト補完/arity診断用）
    func_param_defs: std::collections::HashMap<String, Vec<crate::parser::Param>>,
    /// TypeDef 名 → フィールド名リスト
    type_fields: std::collections::HashMap<String, Vec<String>>,
    /// TypeDef 名 → フィールド名+型アノテーションリスト（JSON スキーマ解決用）
    pub(crate) type_field_types:
        std::collections::HashMap<String, Vec<(String, Option<crate::parser::TypeExpr>)>>,
    /// TypeDef 名 → フィールド定義（型注釈/デフォルト注入用）
    type_field_defs: std::collections::HashMap<String, Vec<crate::parser::FieldDef>>,
    /// ラムダカウンター（一意名生成用）
    lambda_counter: usize,
    /// ラムダから生成された関数（後で module に追加）
    lambda_funcs: Vec<IrFunction>,
    /// 変数名 → ラムダ関数名のマッピング（キャプチャなしラムダの直接呼び出し用）
    lambda_vars: std::collections::HashMap<String, String>,
    /// 変数名 → ラムダ関数名（キャプチャありクロージャ）
    closure_vars: std::collections::HashSet<String>,
    /// 現在の関数内でヒープに確保された変数名のリスト（RC用）
    current_heap_vars: Vec<String>,
    /// stdlib ランタイム関数: インポート名 → C ランタイム関数名
    /// std/math 等の stdlib 関数はユーザー関数ではなくランタイム関数として呼び出す
    stdlib_runtime_funcs: std::collections::HashMap<String, String>,
    /// stdlib 定数: インポート名 → f64 値
    stdlib_constants: std::collections::HashMap<String, f64>,
    /// int 値を保持する変数名のセット（FL-16: poly_add 誤発火防止）
    int_vars: std::collections::HashSet<String>,
    /// float を返す stdlib 関数の結果変数（型追跡用）
    /// float 値を保持する変数名のセット
    float_vars: std::collections::HashSet<String>,
    /// 文字列値を保持する変数名のセット
    string_vars: std::collections::HashSet<String>,
    /// bool 値を保持する変数名のセット
    bool_vars: std::collections::HashSet<String>,
    /// 現在処理中の関数名（末尾再帰検出用）
    current_func_name: Option<String>,
    /// BuchiPack フィールド名のセット（jsonEncode 用フィールド名レジストリ）
    field_names: std::collections::HashSet<String>,
    /// フィールド名 → 型タグ (0=unknown, 1=Int, 2=Float, 3=Str, 4=Bool, 5=Enum)
    field_type_tags: std::collections::HashMap<String, i64>,
    /// C18-2: フィールド名 → Enum variants CSV (例: "Red,Green,Blue")。
    /// `taida_register_field_enum` 呼び出しで native runtime に渡され、
    /// jsonEncode が variant-name Str 出力に使用する。
    field_enum_descriptors: std::collections::HashMap<String, String>,
    /// C18-2: 変数名 → その変数が保持する Enum 型名。
    /// `a <= HiveState:Running()` のような束縛を検出して記録し、後で
    /// `@(state <= a)` のような anonymous BuchiPack 構築時に field を
    /// Enum-tagged として登録できるようにする。型注釈付き束縛
    /// (`state: HiveState <= ...`) も同様に記録する。
    enum_vars: std::collections::HashMap<String, String>,
    /// C18-2: 関数名 → 宣言戻り値が Enum 型名である場合の型名。
    /// `pickColor n = ... => :HiveState` を検出して、呼び出しサイトで
    /// `@(state <= pickColor(n))` の field を Enum-tagged として登録する。
    enum_returning_funcs: std::collections::HashMap<String, String>,
    /// Mold 定義レジストリ（custom mold lowering 用）
    pub(crate) mold_defs: std::collections::HashMap<String, crate::parser::MoldDef>,
    /// Enum definitions: enum_name -> variants in ordinal order
    pub(crate) enum_defs: std::collections::HashMap<String, Vec<String>>,
    /// B11-6d: Inheritance parent map (child_name -> parent_name) for TypeExtends resolution.
    pub(crate) type_parents: std::collections::HashMap<String, String>,
    /// Mold 名 → solidify ヘルパー関数シンボル（mangled）
    pub(crate) mold_solidify_funcs: std::collections::HashMap<String, String>,
    /// 戻り値が Str のユーザー定義関数名セット
    string_returning_funcs: std::collections::HashSet<String>,
    /// 戻り値が Bool のユーザー定義関数名セット
    bool_returning_funcs: std::collections::HashSet<String>,
    /// C12B-022: 関数本体で `TypeIs[param, :T]()` を呼び出す関数。
    /// 呼び出し側で param tag を full propagation する必要がある
    /// (INT=0 も明示的に `taida_set_call_arg_tag` する)
    param_type_check_funcs: std::collections::HashSet<String>,
    /// 戻り値が Float のユーザー定義関数名セット
    float_returning_funcs: std::collections::HashSet<String>,
    /// NB-31: 戻り値が Int/Num のユーザー定義関数名セット
    int_returning_funcs: std::collections::HashSet<String>,
    /// BuchiPack/TypeInst を保持する変数名のセット（F-58 メソッド名衝突回避用）
    pack_vars: std::collections::HashSet<String>,
    /// BuchiPack/TypeInst を返すユーザー定義関数名セット
    pack_returning_funcs: std::collections::HashSet<String>,
    /// List を保持する変数名のセット（retain-on-store 型タグ推論用）
    list_vars: std::collections::HashSet<String>,
    /// List を返すユーザー定義関数名セット（retain-on-store 型タグ推論用）
    list_returning_funcs: std::collections::HashSet<String>,
    /// C21-4: List 変数の要素型名 (`"Float"` / `"Int"` / `"Str"` / `"Bool"`)。
    /// `a: @[Float]` のような型注釈付きパラメータ / 代入から取り出し、
    /// `a.get(i) ]=> av` の unmold 結果型推論 (= `av` を float_vars に入れる) に使う。
    /// これが無いと内積計算 `av * bv` が `taida_int_mul` に降り、Float bits が破壊される。
    list_element_types: std::collections::HashMap<String, String>,
    /// TypeDef 名 → メソッド定義リスト（メソッド名, FuncDef）
    type_method_defs: std::collections::HashMap<String, Vec<(String, crate::parser::FuncDef)>>,
    /// トップレベルで定義される変数名のセット（Native グローバル変数テーブル用）
    top_level_vars: std::collections::HashSet<String>,
    /// 関数から参照されるトップレベル変数名のセット（GlobalSet の emit 対象）
    globals_referenced: std::collections::HashSet<String>,
    /// 変数名 → TypeDef 名のマッピング（QF-10: フィールドアクセス時の型解決用）
    var_type_names: std::collections::HashMap<String, String>,
    /// ローカル関数定義でキャプチャが必要なもの: 関数名 → (ラムダ名, キャプチャ変数リスト)
    /// lower_statement の FuncDef 分岐で MakeClosure を発行するために使用
    pending_local_closures: std::collections::HashMap<String, (String, Vec<String>)>,
    /// インポートされた値シンボル（BuchiPack 等）。
    /// モジュール init 後に GlobalGet してローカル名へ束縛する。
    /// (alias名, original名, import元 module_key) のタプル
    imported_value_symbols: Vec<(String, String, String)>,
    /// init が必要な依存モジュール関数シンボルのリスト（重複排除済み）
    module_inits_needed: Vec<String>,
    /// 現在のモジュールを一意に識別するキー
    module_key: Option<String>,
    /// ライブラリモジュールかどうか（is_library の早期判定用）
    is_library_module: bool,
    /// QF-17: インポートされた TypeDef シンボル
    /// lower_type_inst で型メタデータが無い場合に constructor 呼び出しにフォールバック
    imported_type_symbols: std::collections::HashSet<String>,
    /// ソースファイルのディレクトリ（インポートモジュール解決用）
    source_dir: Option<std::path::PathBuf>,
    /// import 関数名（alias 含む）→ 実リンクシンボル名のマッピング
    imported_func_links: std::collections::HashMap<String, String>,
    /// 関数本体から参照可能な import 値名
    imported_value_names: std::collections::HashSet<String>,
    /// 現在のモジュールで export されるシンボル名
    exported_symbols: std::collections::HashSet<String>,
    /// Lax 変数の内部型追跡（変数名 → 内部型名）
    /// 例: `x <= Bool["maybe"]()` → lax_inner_types["x"] = "Bool"
    /// `x ]=> val` で val の型を正しく推定するために使用
    lax_inner_types: std::collections::HashMap<String, String>,
    /// Net builtin names shadowed by a parameter in the current function scope.
    /// When a name is here, stdlib_runtime_funcs dispatch for that name is skipped
    /// and the call is treated as a parameter/variable call instead.
    shadowed_net_builtins: std::collections::HashSet<String>,
    /// NB-14: Parameter name -> IrVar holding the runtime type tag from the caller.
    /// Used to propagate Bool/Int distinction through function boundaries.
    /// Populated at function entry via taida_get_call_arg_tag().
    param_tag_vars: std::collections::HashMap<String, IrVar>,
    /// NB-14: IrVar (CallUser result) -> IrVar (return type tag from that call).
    /// Populated after CallUser by calling taida_get_return_tag().
    /// Used to propagate type tags through function return values.
    return_tag_vars: std::collections::HashMap<IrVar, IrVar>,
    /// NB-14: When true, the current CallUser is in tail position (return value).
    /// Skip get_return_tag to preserve C compiler tail call optimization (WASM/mutual recursion).
    in_tail_call_return: bool,
    /// NB3-4: Variable alias tracking for identity assignments (e.g., `h <= handler`).
    /// Maps target variable name to source variable name.
    var_aliases: std::collections::HashMap<String, String>,
    /// NB3-4: Lambda parameter count tracking for lambda assignments (e.g., `h <= req, writer => @(...)`).
    /// Maps variable name to the number of lambda parameters.
    lambda_param_counts: std::collections::HashMap<String, usize>,
    /// NB3-4 fix: Parameter names whose type was inferred from return-type annotation
    /// (not from explicit type annotations or literal assignments).
    /// These are unreliable for callable_type_tag because the parameter might actually
    /// be a function/closure passed at runtime.
    return_type_inferred_params: std::collections::HashSet<String>,
    /// RC2.5: addon function reference table.
    /// Maps an imported symbol (alias or original name) to the addon dispatch
    /// metadata needed by `lower_func_call` to emit a `taida_addon_call` IR
    /// call. Populated in `lower_addon_import` during the `Statement::Import`
    /// pass.
    addon_func_refs: std::collections::HashMap<String, AddonFuncRef>,
    /// RC2.5 Phase 2: facade-declared pure-Taida value bindings pulled in
    /// through an addon-backed package import. Each entry is an assignment
    /// of the form `Name <= <expr>` (e.g. `KeyKind <= @(Char <= 0, ...)`)
    /// that the facade file exports. They are replayed at the top of
    /// `_taida_main` so user code can reference them without the main
    /// program ever parsing the facade file itself.
    ///
    /// Keyed by the local binding name. The order field controls
    /// replay ordering so facade authors can express value dependencies.
    addon_facade_pack_bindings: Vec<(String, Expr)>,
    /// C25B-030 Phase 1E-β: facade-declared FuncDefs harvested across
    /// every addon-backed package imported by the current lowering
    /// run.
    ///
    /// Each entry is `(local_name, func_def, mangled_link_symbol)`
    /// where `local_name` is the name the facade author wrote
    /// (e.g. `ClearScreen` or `_makeCellsLoop`) and
    /// `mangled_link_symbol` is the IR function symbol actually
    /// emitted (e.g. `_taida_fn_facade_<hash>_ClearScreen`). The
    /// mangle carries a per-addon hash so FuncDefs from two
    /// different addons can coexist without colliding, and so user
    /// code can still declare a local `ClearScreen` without trouble.
    ///
    /// Populated during `lower_addon_import` via the facade loader
    /// and drained during `lower_program`'s 2nd pass: each FuncDef
    /// is fed through [`Lowering::lower_func_def`] under the
    /// mangled name. User imports of a facade symbol rewrite to the
    /// mangled symbol through `imported_func_links` so call sites
    /// go through the normal `resolve_user_func_symbol` path.
    ///
    /// Deduplicated by mangled symbol: if the same facade file is
    /// imported twice (two user imports referencing the same addon)
    /// only the first entry wins. The loader canonicalises paths
    /// before deriving the mangle so this check is O(1).
    addon_facade_funcs: Vec<(String, FuncDef, String)>,
    /// C25B-030 Phase 1E-β: set of mangled facade link symbols
    /// already collected. Used by the facade loader for O(1)
    /// dedup when the same addon is referenced by more than one
    /// import statement (`>>> taida-lang/terminal => @(A)` then
    /// `>>> taida-lang/terminal => @(B)`).
    addon_facade_mangled: std::collections::HashSet<String>,
    /// RC2.5: the addon backend this lowering run targets. Only `Native`
    /// accepts addon imports; all WASM targets and JS/Interpreter path
    /// through the backend-policy error with a deterministic message.
    /// Defaults to `Native` so existing Cranelift callers do not need to
    /// change.
    addon_backend: crate::addon::AddonBackend,
}

/// RC2.5: metadata for a single addon function import.
///
/// `package_id` / `cdylib_path` / `function_name` become static strings
/// emitted into `.rodata` via `IrInst::ConstStr`; `arity` is enforced at
/// the IR call site and re-checked by the C-side dispatcher.
#[derive(Debug, Clone)]
struct AddonFuncRef {
    package_id: String,
    cdylib_path: String,
    function_name: String,
    arity: u32,
}

/// RC2.5 Phase 2 / C25B-030 Phase 1E-α + 1E-β: shallow summary of an
/// addon facade file.
///
/// Facades are parsed for top-level bindings and passed through to
/// the native lowering pipeline. The following constructs are
/// understood:
///
/// - Alias assignments `Name <= lowercaseFn` (`aliases`)
/// - Pure-Taida pack assignments `Name <= @(...)` (`pack_bindings`)
/// - Facade-internal relative imports `>>> ./X.td => @(syms...)`
///   (C25B-030 Phase 1E-α) — the referenced file is recursively
///   loaded under the same rules and its exports for the requested
///   symbols are merged into the parent summary
/// - **Function definitions** `Name args = body => :Type`
///   (C25B-030 Phase 1E-β) — lowered as sibling IR functions under
///   a mangled symbol derived from the addon package id so they do
///   not collide with user-defined functions of the same name.
/// - A single `<<<` export clause (`exports`)
///
/// TypeDef / EnumDef / MoldDef statements are still rejected
/// deterministically; the public addon authoring contract only
/// requires FuncDef + Assignment + Import. Lifting those remaining
/// constraints is tracked as C25B-030 Phase 1E-γ (module-graph
/// integration and full sibling module linkage via
/// `src/addon/facade.rs`).
/// C25B-030 Phase 1G: the codegen's addon-facade view is now a
/// thin wrapper around the shared `crate::addon::facade::
/// AddonFacadeSummary` produced by the backend-agnostic loader.
/// We keep the private `struct` shape instead of a `pub use` to
/// preserve the lowering crate's encapsulation (downstream code
/// should route through `lower_addon_import`, not build facade
/// summaries by hand).
#[derive(Debug, Default, Clone)]
struct AddonFacadeSummary {
    aliases: std::collections::HashMap<String, String>,
    pack_bindings: std::collections::HashMap<String, Expr>,
    exports: std::collections::HashSet<String>,
    facade_funcs: std::collections::HashMap<String, FuncDef>,
}

impl AddonFacadeSummary {
    /// Adopt the shared facade loader's output verbatim. Field
    /// shapes are identical so this is a move, not a conversion;
    /// the method exists purely to pin the codegen/addon
    /// boundary in one grep-friendly place.
    pub(super) fn from_shared(shared: crate::addon::facade::AddonFacadeSummary) -> Self {
        Self {
            aliases: shared.aliases,
            pack_bindings: shared.pack_bindings,
            exports: shared.exports,
            facade_funcs: shared.facade_funcs,
        }
    }
}

#[derive(Debug)]
pub struct LowerError {
    pub message: String,
}

impl std::fmt::Display for LowerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lowering error: {}", self.message)
    }
}

/// QF-16/17: インポートされたシンボルの種類
enum ImportedSymbolKind {
    /// 通常の関数定義
    Function,
    /// トップレベル代入値（BuchiPack, ラムダ等）
    Value,
    /// TypeDef / InheritanceDef
    TypeDef,
}

type InheritanceChainFields = (
    Vec<String>,
    Vec<(String, Option<crate::parser::TypeExpr>)>,
    Vec<crate::parser::FieldDef>,
    Vec<(String, crate::parser::FuncDef)>,
);

impl Default for Lowering {
    fn default() -> Self {
        Self::new()
    }
}

mod core;
mod expr;
mod imports;
mod infer;
mod molds;
mod net;
mod os;
mod stdlib;
mod stmt;
mod tag_prop;
