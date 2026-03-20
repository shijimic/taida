//! Taida IR — AST と CLIF IR の間に位置する中間表現。
//!
//! ## ABI 正規化メモ（W-0）
//!
//! IR は **型なし (untyped)** である。全 IrVar は emit.rs 層で boxed `value_ty`（I64）
//! として扱われる。値・ポインタ・関数ポインタの区別は IR 層では行わず、
//! emit.rs の runtime ABI 境界（`runtime_abi()` + `resolve_abi()`）でのみ型を区別する。
//!
//! 各命令の「意味的な型」は以下のとおり（emit.rs が boundary cast を挿入する際の指針）:
//! - `ConstInt`, `ConstBool` → Val（整数値）
//! - `ConstStr` → Ptr（ヒープ参照）だが、boxed value_ty として保持
//! - `ConstFloat` → F64 だが、boxed value_ty に bitcast して保持
//! - `FuncAddr` → FnPtr（関数ポインタ）だが、boxed value_ty として保持
//! - `PackNew`/`PackGet`/`PackSet` → runtime ABI は Ptr 混在だが、IR 上は全て untyped
//! - `GlobalSet`/`GlobalGet` → name_hash は Val
//! - `CallIndirect` → ユーザー関数 ABI（全パラメータ・戻り値が value_ty）

/// IR 内の仮想変数（SSA 値への参照）
pub type IrVar = u32;

/// IR 命令
///
/// 全 IrVar は untyped。emit.rs で boxed `value_ty` に解決される。
#[derive(Debug, Clone)]
pub enum IrInst {
    // ── リテラル ──
    /// 整数定数: 意味的に Val
    ConstInt(IrVar, i64),
    /// 浮動小数点定数: 意味的に F64（emit 時に value_ty へ bitcast される場合あり）
    ConstFloat(IrVar, f64),
    /// 文字列定数: 意味的に Ptr（ヒープ参照）、boxed value_ty として保持
    ConstStr(IrVar, String),
    /// 真偽値定数: 意味的に Val
    ConstBool(IrVar, bool),

    // ── 変数操作（Phase N2） ──
    /// 変数定義: `name <= value` → DefVar(name, source_var)
    DefVar(String, IrVar),
    /// 変数参照: UseVar(dst, name)
    UseVar(IrVar, String),

    // ── ぶちパック操作（Phase N3） ──
    /// ぶちパック生成: PackNew(dst, field_count)
    /// field_count は Val、戻り値は Ptr
    PackNew(IrVar, usize),
    /// フィールド設定: PackSet(pack_var, field_index, value_var)
    /// pack_var: Ptr, field_index: Val, value_var: Val
    PackSet(IrVar, usize, IrVar),
    /// フィールド型タグ設定: PackSetTag(pack_var, field_index, type_tag)
    /// type_tag: 0=Int, 1=Float, 2=Bool, 3=Str, 4=Pack, 5=List, 6=Closure
    /// pack_var: Ptr, field_index: Val, type_tag: Val
    PackSetTag(IrVar, usize, i64),
    /// フィールド取得: PackGet(dst, pack_var, field_index)
    /// pack_var: Ptr, field_index: Val, 戻り値: Val
    PackGet(IrVar, IrVar, usize),

    // ── 関数呼び出し ──
    /// ランタイム関数呼び出し: `result = call(func_name, args...)`
    /// ABI は runtime_abi() で決まる（Val/Ptr/FnPtr/F64 混在）
    Call(IrVar, String, Vec<IrVar>),
    /// ユーザー定義関数呼び出し: `result = call_user(func_name, args...)`
    /// 全パラメータ・戻り値が value_ty
    CallUser(IrVar, String, Vec<IrVar>),
    /// 間接関数呼び出し（ラムダ/クロージャ経由）: `result = call_indirect(fn_var, args...)`
    /// fn_var: 意味的に FnPtr だが boxed value_ty として保持
    /// 全パラメータ・戻り値が value_ty
    CallIndirect(IrVar, IrVar, Vec<IrVar>),

    // ── ラムダ/クロージャ ──
    /// クロージャ生成: MakeClosure(dst, func_name, captures)
    /// captures: キャプチャ変数名のリスト
    /// 戻り値: Ptr（クロージャ構造体）、boxed value_ty として保持
    MakeClosure(IrVar, String, Vec<String>),
    /// 関数アドレス取得: FuncAddr(dst, func_name)
    /// 意味的に FnPtr だが、boxed value_ty として保持
    FuncAddr(IrVar, String),

    // ── 制御フロー ──
    /// 条件分岐: CondBranch(result, arms)
    /// arms: Vec<(Option<IrVar>, Vec<IrInst>, IrVar)>
    ///   - condition (None = default case)
    ///   - body instructions
    ///   - result var of body
    CondBranch(IrVar, Vec<CondArm>),

    // ── 参照カウント（Phase N7） ──
    /// Retain: refcount++ (ヒープオブジェクトの参照カウントをインクリメント)
    /// 引数: Ptr（boxed value_ty として渡される）
    Retain(IrVar),
    /// Release: refcount-- (0になったらfree)
    /// 引数: Ptr（boxed value_ty として渡される）
    Release(IrVar),

    /// 末尾再帰呼び出し: 引数を再代入してエントリブロックにジャンプ
    /// TailCall(args) — 引数の IrVar リスト
    TailCall(Vec<IrVar>),

    // ── グローバル変数（トップレベル変数の関数間共有） ──
    /// グローバル変数への書き込み: GlobalSet(name_hash, value_var)
    /// name_hash: Val, value_var: Val (boxed)
    GlobalSet(i64, IrVar),
    /// グローバル変数からの読み取り: GlobalGet(dst, name_hash)
    /// name_hash: Val
    GlobalGet(IrVar, i64),

    Return(IrVar),
}

/// IR 関数定義
#[derive(Debug, Clone)]
pub struct IrFunction {
    pub name: String,
    pub params: Vec<String>,
    pub body: Vec<IrInst>,
    pub next_var: IrVar,
}

impl IrFunction {
    pub fn new(name: String) -> Self {
        Self {
            name,
            params: Vec::new(),
            body: Vec::new(),
            next_var: 0,
        }
    }

    pub fn new_with_params(name: String, params: Vec<String>) -> Self {
        let next_var = params.len() as u32;
        Self {
            name,
            params,
            body: Vec::new(),
            next_var,
        }
    }

    pub fn alloc_var(&mut self) -> IrVar {
        let var = self.next_var;
        self.next_var += 1;
        var
    }

    pub fn push(&mut self, inst: IrInst) {
        self.body.push(inst);
    }
}

/// 条件分岐のアーム
#[derive(Debug, Clone)]
pub struct CondArm {
    /// 条件（None = デフォルトケース `| _ |>`）
    pub condition: Option<IrVar>,
    /// 本体の命令列
    pub body: Vec<IrInst>,
    /// 本体の結果変数
    pub result: IrVar,
}

/// IR モジュール（1ファイル = 1モジュール）
#[derive(Debug, Clone)]
pub struct IrModule {
    pub functions: Vec<IrFunction>,
    /// エクスポートされるリンクシンボル名
    pub exports: Vec<String>,
    /// インポート: (モジュールパス, 依存する関数リンクシンボル名リスト, version)
    pub imports: Vec<(String, Vec<String>, Option<String>)>,
    /// ライブラリモジュール（エクスポートあり）かどうか
    pub is_library: bool,
    /// モジュール一意キー
    pub module_key: Option<String>,
}

impl Default for IrModule {
    fn default() -> Self {
        Self::new()
    }
}

impl IrModule {
    pub fn new() -> Self {
        Self {
            functions: Vec::new(),
            exports: Vec::new(),
            imports: Vec::new(),
            is_library: false,
            module_key: None,
        }
    }
}
