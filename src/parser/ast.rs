use crate::lexer::Span;

/// Top-level program: a sequence of statements.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Statement>,
}

/// Context tag used while parsing a `| cond |> body` branch.
///
/// The parser switches into `LetRhs` while reading the right-hand side of
/// a `<=` binding (`name <= expr` / `name: T <= expr`). In that context a
/// multi-line `| cond |> A | _ |> B` is ambiguous with the enclosing block
/// (`parse_cond_branch` historically swallowed subsequent top-level statements
/// as continuation arms). `TopLevel` is the default and preserves the classic
/// top-level / `| |>` match expression semantics.
///
/// A parenthesised `(|... |>...)` resets to `TopLevel`, so `name <= (...)`
/// stays a legal escape hatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CondBranchContext {
    /// Top-level / function body / inside parentheses — multi-line arms permitted.
    #[default]
    TopLevel,
    /// `<=` (or typed `: T <=`) right-hand side — multi-line arms rejected with `[E0303]`.
    LetRhs,
}

/// A statement in Taida.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// Expression statement (expression evaluated for its value or side effects).
    Expr(Expr),
    /// Enum definition: `Enum => Name = :A :B`
    EnumDef(EnumDef),
    /// Class-like definition:
    /// 旧 `TypeDef` / `MoldDef` / `InheritanceDef` を統合。`ClassLikeKind`
    /// discriminator で 3 系統 (BuchiPack / Mold / Inheritance) を内部分類する。
    ClassLikeDef(ClassLikeDef),
    /// Function definition: `name params = body => :ReturnType`
    FuncDef(FuncDef),
    /// Variable assignment: `name <= expr` or `expr => name`
    Assignment(Assignment),
    /// Error ceiling: `|== error: Type = body => :ReturnType`
    ErrorCeiling(ErrorCeiling),
    /// Import: `>>> path => @(symbols)`
    Import(ImportStmt),
    /// Export: `<<< @(symbols)` or `<<< symbol`
    Export(ExportStmt),
    /// Unmold forward: `expr >=> name`
    UnmoldForward(UnmoldForwardStmt),
    /// Unmold backward: `name <=< expr`
    UnmoldBackward(UnmoldBackwardStmt),
}

impl Statement {
    /// For a "value-yielding" statement (the tail of an expression
    /// block), return a reference to the `Expr` whose value is the block
    /// result. Tail bindings (`name <= expr`, `expr => name`, `expr >=> name`,
    /// `name <=< expr`) yield their RHS source expression; a plain
    /// `Statement::Expr(e)` yields `e`.
    ///
    /// Returns `None` for statements that do not produce a value
    /// (definitions, imports, exports, error ceilings, ...).
    ///
    /// NB: For unmold bindings the returned `Expr` is the *source* (the
    /// value **before** unmold). Consumers that need the unmolded result
    /// type should unmold it themselves (e.g. the checker's
    /// `unmold_type`), which keeps the helper purely syntactic.
    pub fn yielded_expr(&self) -> Option<&Expr> {
        match self {
            Statement::Expr(e) => Some(e),
            Statement::Assignment(a) => Some(&a.value),
            Statement::UnmoldForward(u) => Some(&u.source),
            Statement::UnmoldBackward(u) => Some(&u.source),
            _ => None,
        }
    }

    /// True if this statement represents a tail binding form
    /// (`name <= expr`, `expr => name`, `expr >=> name`, `name <=< expr`)
    /// whose bound target should be defined in the enclosing scope
    /// before the block result is yielded.
    pub fn is_tail_binding(&self) -> bool {
        matches!(
            self,
            Statement::Assignment(_) | Statement::UnmoldForward(_) | Statement::UnmoldBackward(_)
        )
    }
}

/// Enum definition: `Enum => Name = :A :B`
#[derive(Debug, Clone, PartialEq)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<EnumVariantDef>,
    /// Documentation comments (`///@`) attached to this enum definition.
    pub doc_comments: Vec<String>,
    pub span: Span,
}

/// A single enum variant in an enum definition.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariantDef {
    pub name: String,
    pub span: Span,
}

/// Class-like definition:
/// 旧 `TypeDef` / `MoldDef` / `InheritanceDef` を `Statement::ClassLikeDef`
/// 単一 variant に統合。3 系統の残存差は `ClassLikeKind` discriminator で
/// 内部分類する。
#[derive(Debug, Clone, PartialEq)]
pub struct ClassLikeDef {
    /// 子型名 (旧 `TypeDef::name` / `MoldDef::name` / `InheritanceDef::child` を統合)。
    pub name: String,
    /// Buchi-pack body フィールド群。
    pub fields: Vec<FieldDef>,
    /// Documentation comments (`///@`) attached to this class-like definition.
    pub doc_comments: Vec<String>,
    pub span: Span,
    /// kind discriminator (BuchiPack / Mold / Inheritance)。
    pub kind: ClassLikeKind,
    /// 子側 `Name[...]` ヘッダ引数 (Mold / Inheritance 系で子が独自の type params を持つ場合)。
    /// 旧 `MoldDef::name_args` / `InheritanceDef::child_args` を統合。
    pub name_args: Option<Vec<MoldHeaderArg>>,
    /// declared type variables (旧 `MoldDef::type_params` を継承)。Mold kind でのみ非空。
    pub type_params: Vec<TypeParam>,
}

/// `ClassLikeDef` の kind discriminator。surface 上は migration / docs history 以外には
/// 出さず、内部 dispatch 用のみで使う ( 軸 1: 旧語彙退避)。
#[derive(Debug, Clone, PartialEq)]
pub enum ClassLikeKind {
    /// 旧 `TypeDef` 系: `Pilot = @(...)` (zero-arity sugar `Pilot[] = @(...)` は
    /// で受理予定、本 Sub-step では旧構文のみ)。
    BuchiPack,
    /// 旧 `MoldDef` 系: `Mold[T] => Name[T] = @(...)`。
    /// `mold_args` は親側 `Mold[...]` 内の引数。
    Mold { mold_args: Vec<MoldHeaderArg> },
    /// 旧 `InheritanceDef` 系: `Parent => Child = @(...)` または
    /// `Parent[T] => Child[T] = @(...)`。
    /// `parent` は親型名 (`"Error"` / `"User"` 等)。
    /// `parent_args` は親型適用の引数 (旧 `InheritanceDef::parent_args`)。
    Inheritance {
        parent: String,
        parent_args: Option<Vec<MoldHeaderArg>>,
    },
}

impl ClassLikeDef {
    /// Inheritance kind なら親型名を返す。それ以外は `None`。
    pub fn parent(&self) -> Option<&str> {
        match &self.kind {
            ClassLikeKind::Inheritance { parent, .. } => Some(parent.as_str()),
            _ => None,
        }
    }

    /// Inheritance kind なら親型適用の引数を返す。それ以外は `None`。
    pub fn parent_args(&self) -> Option<&Vec<MoldHeaderArg>> {
        match &self.kind {
            ClassLikeKind::Inheritance { parent_args, .. } => parent_args.as_ref(),
            _ => None,
        }
    }

    /// Mold kind なら `Mold[...]` 側の引数を返す。それ以外は `None`。
    pub fn mold_args(&self) -> Option<&Vec<MoldHeaderArg>> {
        match &self.kind {
            ClassLikeKind::Mold { mold_args } => Some(mold_args),
            _ => None,
        }
    }

    /// Inheritance kind かどうか (旧 `Statement::InheritanceDef` 判定の置換用)。
    pub fn is_inheritance(&self) -> bool {
        matches!(self.kind, ClassLikeKind::Inheritance { .. })
    }

    /// Mold kind かどうか (旧 `Statement::MoldDef` 判定の置換用)。
    pub fn is_mold(&self) -> bool {
        matches!(self.kind, ClassLikeKind::Mold { .. })
    }

    /// BuchiPack kind かどうか (旧 `Statement::TypeDef` 判定の置換用)。
    pub fn is_buchi_pack(&self) -> bool {
        matches!(self.kind, ClassLikeKind::BuchiPack)
    }

    /// Legacy-form detector: 本 ClassLikeDef が旧構文か判定する。
    ///
    /// 以下を旧構文として扱う:
    /// - `Mold[T] => Foo[T] = @(...)` 形式 (`ClassLikeKind::Mold`) — 新構文では
    /// `Mold[T] =>` prefix 撤廃で `Foo[T] = @(...)` (zero-or-more arity の
    /// type-def 形式) として書き換え可能。
    ///
    /// 以下は旧構文ではない:
    /// - `Pilot = @(...)` (`Pilot[] = @(...)` と等価、migration は推奨 ≠ 必須)
    /// - `Error => NotFound = @(...)` (prefix 撤廃は必須ではない。
    /// Error 継承構文は新仕様でも保持される)
    ///
    /// 用途: 旧構文診断 / compatibility audit hook。
    pub fn is_legacy_e30_syntax(&self) -> bool {
        matches!(self.kind, ClassLikeKind::Mold { .. })
    }

    /// Legacy-form 表示ラベル。
    ///
    /// `is_legacy_e30_syntax()` が true のときに、旧構文の category 名を
    /// 返す (dry-run 出力 / diagnostic 用)。
    ///
    /// - `Some("mold")` — 旧 `Mold[T] => Foo[T] = @(...)` 形式
    /// - `None` — 新構文 (migration 対象外)
    pub fn legacy_e30_kind(&self) -> Option<&'static str> {
        match &self.kind {
            ClassLikeKind::Mold { .. } => Some("mold"),
            _ => None,
        }
    }
}

/// A field in a type or buchi pack definition.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub type_annotation: Option<TypeExpr>,
    pub default_value: Option<Expr>,
    /// If this field is a method definition.
    pub is_method: bool,
    pub method_def: Option<FuncDef>,
    /// Documentation comments (`///@`) attached to this field.
    pub doc_comments: Vec<String>,
    pub span: Span,
}

impl FieldDef {
    /// Declare-only function field detection.
    ///
    /// A declare-only function field is a field declared with a function type
    /// annotation (e.g. `greet: Str => :Str`) but **without** a method body
    /// (`is_method == false`) and **without** an explicit default value
    /// (`default_value.is_none()`). Such a field is effectively an interface
    /// member: the type is fixed by the declaration, but the value is supplied
    /// either at instantiation time (via `(name <= ...)`) or by an
    /// automatically-generated `defaultFn`.
    ///
    /// The checker uses this helper to exclude declare-only function fields
    /// from the "required positional `[]` argument" set in
    /// `validate_custom_mold_inst_bindings` and from the extra-type-arg
    /// binding-target count in `validate_mold_extension_bindings`.
    pub fn is_declare_only_fn_field(&self) -> bool {
        if self.is_method || self.default_value.is_some() {
            return false;
        }
        matches!(self.type_annotation, Some(TypeExpr::Function(_, _)))
    }
}

impl BuchiField {
    /// True for parser-synthesized positional call arguments (`_0`, `_1`, ...)
    /// used internally by mold-call lowering.
    pub fn is_synthetic_positional(&self) -> bool {
        self.name
            .strip_prefix('_')
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
    }
}

/// Type expression.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeExpr {
    /// Simple named type: `Int`, `Str`, `Bool`, `User`
    Named(String),
    /// Buchi pack type: `@(name: Str, age: Int)`
    BuchiPack(Vec<FieldDef>),
    /// List type: `@[T]`
    List(Box<TypeExpr>),
    /// Generic type: `Optional[T]`, `Result[T, E]`
    Generic(String, Vec<TypeExpr>),
    /// Function type: `:T => :U`
    Function(Vec<TypeExpr>, Box<TypeExpr>),
}

/// Function definition.
#[derive(Debug, Clone, PartialEq)]
pub struct FuncDef {
    pub name: String,
    /// Generic type parameters declared on the function, e.g. `id[T]`.
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub body: Vec<Statement>,
    pub return_type: Option<TypeExpr>,
    /// Documentation comments (`///@`) attached to this function definition.
    pub doc_comments: Vec<String>,
    pub span: Span,
}

/// Function parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub type_annotation: Option<TypeExpr>,
    pub default_value: Option<Expr>,
    pub span: Span,
}

/// Variable assignment.
#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub target: String,
    pub type_annotation: Option<TypeExpr>,
    pub value: Expr,
    pub doc_comments: Vec<String>,
    pub span: Span,
}

impl Assignment {
    /// If this assignment is the
    /// explicit addon-binding form `target <= RustAddon["fn"](arity <= N)`,
    /// returns `Some((fn_name, arity))`. Otherwise returns `None`.
    ///
    /// This helper drives consumer parity (doc-gen / LSP / graph / pkg
    /// facade / introspection) so a `RustAddon[...]` binding is surfaced
    /// as a **public function** instead of a generic value, even though
    /// the AST representation is `Statement::Assignment(_)`.
    ///
    /// Validation here is **structural only** (matches the surface form);
    /// drift / context errors are emitted by the interpreter
    /// (`eval_rust_addon_binding`) and the addon facade summary loader
    /// (`src/addon/facade.rs::load_facade_summary`).
    pub fn as_rust_addon_binding(&self) -> Option<(String, u32)> {
        if let Expr::MoldInst(name, type_args, fields, _) = &self.value
            && name == "RustAddon"
            && type_args.len() == 1
            && fields.len() == 1
            && fields[0].name == "arity"
        {
            let fn_name = match &type_args[0] {
                Expr::StringLit(s, _) => s.clone(),
                _ => return None,
            };
            let arity = match &fields[0].value {
                Expr::IntLit(n, _) => u32::try_from(*n).ok()?,
                _ => return None,
            };
            return Some((fn_name, arity));
        }
        None
    }
}

/// Mold header argument in `Mold[...]` / `Name[...]`.
#[derive(Debug, Clone, PartialEq)]
pub enum MoldHeaderArg {
    /// Type variable, optionally with a constraint.
    TypeParam(TypeParam),
    /// Concrete type expression introduced with `:`.
    Concrete(TypeExpr),
}

// 旧 `MoldDef` / `InheritanceDef` struct は廃止。
// 統合先は `ClassLikeDef` (上に定義) + `ClassLikeKind::Mold|Inheritance` discriminator。

/// Type parameter, optionally with constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeParam {
    pub name: String,
    pub constraint: Option<TypeExpr>,
}

/// Error ceiling block.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorCeiling {
    pub error_param: String,
    pub error_type: TypeExpr,
    pub handler_body: Vec<Statement>,
    pub return_type: Option<TypeExpr>,
    pub span: Span,
}

/// Import statement: `>>> path => @(symbols)` or `>>> author/pkg@version`
#[derive(Debug, Clone, PartialEq)]
pub struct ImportStmt {
    pub path: String,
    /// Semver version from `@x.y.z` suffix (e.g. Some("1.0.0") for `>>> author/pkg@1.0.0`)
    pub version: Option<String>,
    pub symbols: Vec<ImportSymbol>,
    pub span: Span,
}

/// An imported symbol, optionally aliased.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportSymbol {
    pub name: String,
    pub alias: Option<String>,
}

/// Export statement: `<<< @(symbols)` or `<<<@version @(symbols)` or `<<< path`
#[derive(Debug, Clone, PartialEq)]
pub struct ExportStmt {
    /// Semver version from `<<<@x.y.z` (e.g. Some("1.0.0"))
    pub version: Option<String>,
    pub symbols: Vec<String>,
    /// Re-export path (e.g. Some("./main.td") for `<<< ./main.td`)
    pub path: Option<String>,
    pub span: Span,
}

/// Unmold forward: `expr >=> name`
#[derive(Debug, Clone, PartialEq)]
pub struct UnmoldForwardStmt {
    pub source: Expr,
    pub target: String,
    pub span: Span,
}

/// Unmold backward: `name <=< expr`
#[derive(Debug, Clone, PartialEq)]
pub struct UnmoldBackwardStmt {
    pub target: String,
    pub source: Expr,
    pub span: Span,
}

/// Expression types.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// Integer literal
    IntLit(i64, Span),
    /// Float literal
    FloatLit(f64, Span),
    /// String literal
    StringLit(String, Span),
    /// Template string literal (with interpolation parts)
    TemplateLit(String, Span),
    /// Boolean literal
    BoolLit(bool, Span),
    /// Gorilla literal `><`
    Gorilla(Span),
    /// Identifier / variable reference
    Ident(String, Span),
    /// Placeholder `_`
    Placeholder(Span),
    /// Hole: empty slot in function call for partial application `f(5)`
    Hole(Span),

    /// Buchi pack literal: `@(field <= value, ...)`
    BuchiPack(Vec<BuchiField>, Span),
    /// List literal: `@[expr, ...]`
    ListLit(Vec<Expr>, Span),

    /// Binary operation: `a + b`, `a == b`, etc.
    BinaryOp(Box<Expr>, BinOp, Box<Expr>, Span),
    /// Unary operation: `!x`, `-x`
    UnaryOp(UnaryOp, Box<Expr>, Span),

    /// Function call: `func(args)`
    FuncCall(Box<Expr>, Vec<Expr>, Span),
    /// Method call: `expr.method(args)`
    MethodCall(Box<Expr>, String, Vec<Expr>, Span),
    /// Field access: `expr.field`
    FieldAccess(Box<Expr>, String, Span),
    /// Condition branch: `| cond |> value`
    CondBranch(Vec<CondArm>, Span),

    /// Pipeline: `expr => func(_) => result` (stored as chain of pipe operations)
    Pipeline(Vec<Expr>, Span),

    /// Mold instantiation: `TypeName[args](fields)`
    MoldInst(String, Vec<Expr>, Vec<BuchiField>, Span),

    /// Unmold expression: `expr.unmold()` or `>=>` as expr
    Unmold(Box<Expr>, Span),

    /// Anonymous function: `_ x = x * 2`
    Lambda(Vec<Param>, Box<Expr>, Span),

    /// Type instantiation: `TypeName(field <= value, ...)`
    TypeInst(String, Vec<BuchiField>, Span),

    /// Enum value constructor: `Name:Variant()`
    EnumVariant(String, String, Span),

    /// Restricted type literal inside mold args.
    /// `:Int` → TypeLiteral("Int", None, span)
    /// `EnumName:Variant` (without `()`) → TypeLiteral("EnumName", Some("Variant"), span)
    /// Only valid inside `TypeIs[...]` / `TypeExtends[...]` mold brackets.
    TypeLiteral(String, Option<String>, Span),

    /// Throw expression: `expr.throw()`
    Throw(Box<Expr>, Span),
}

impl Expr {
    pub fn span(&self) -> &Span {
        match self {
            Expr::IntLit(_, span)
            | Expr::FloatLit(_, span)
            | Expr::StringLit(_, span)
            | Expr::TemplateLit(_, span)
            | Expr::BoolLit(_, span)
            | Expr::Gorilla(span)
            | Expr::Ident(_, span)
            | Expr::Placeholder(span)
            | Expr::Hole(span)
            | Expr::BuchiPack(_, span)
            | Expr::ListLit(_, span)
            | Expr::BinaryOp(_, _, _, span)
            | Expr::UnaryOp(_, _, span)
            | Expr::FuncCall(_, _, span)
            | Expr::MethodCall(_, _, _, span)
            | Expr::FieldAccess(_, _, span)
            | Expr::CondBranch(_, span)
            | Expr::Pipeline(_, span)
            | Expr::MoldInst(_, _, _, span)
            | Expr::Unmold(_, span)
            | Expr::Lambda(_, _, span)
            | Expr::TypeInst(_, _, span)
            | Expr::EnumVariant(_, _, span)
            | Expr::TypeLiteral(_, _, span)
            | Expr::Throw(_, span) => span,
        }
    }

    pub fn node_id(&self) -> usize {
        self.span().node_id
    }

    pub fn span_mut(&mut self) -> &mut Span {
        match self {
            Expr::IntLit(_, span)
            | Expr::FloatLit(_, span)
            | Expr::StringLit(_, span)
            | Expr::TemplateLit(_, span)
            | Expr::BoolLit(_, span)
            | Expr::Gorilla(span)
            | Expr::Ident(_, span)
            | Expr::Placeholder(span)
            | Expr::Hole(span)
            | Expr::BuchiPack(_, span)
            | Expr::ListLit(_, span)
            | Expr::BinaryOp(_, _, _, span)
            | Expr::UnaryOp(_, _, span)
            | Expr::FuncCall(_, _, span)
            | Expr::MethodCall(_, _, _, span)
            | Expr::FieldAccess(_, _, span)
            | Expr::CondBranch(_, span)
            | Expr::Pipeline(_, span)
            | Expr::MoldInst(_, _, _, span)
            | Expr::Unmold(_, span)
            | Expr::Lambda(_, _, span)
            | Expr::TypeInst(_, _, span)
            | Expr::EnumVariant(_, _, span)
            | Expr::TypeLiteral(_, _, span)
            | Expr::Throw(_, span) => span,
        }
    }
}

/// Allocator for parser expression node ids.
///
/// Node ids are AST identity, not source location. The parser assigns them
/// after parsing, and AST rewrites that create new expression nodes must
/// reassign ids instead of preserving cloned ids from source expressions.
#[derive(Debug, Clone)]
pub struct NodeIdAllocator {
    next: usize,
}

impl NodeIdAllocator {
    pub fn new() -> Self {
        Self { next: 1 }
    }

    pub fn starting_at(next: usize) -> Self {
        Self { next }
    }

    pub fn fresh(&mut self) -> usize {
        let id = self.next;
        self.next += 1;
        id
    }
}

impl Default for NodeIdAllocator {
    fn default() -> Self {
        Self::new()
    }
}

/// Assign stable expression node ids to a parsed program.
///
/// The ids live in `Span::node_id` so existing expression variants keep their
/// shape while `TypedExprTable` can key by AST identity instead of source span.
pub fn assign_expr_node_ids(program: &mut Program) {
    let mut allocator = NodeIdAllocator::new();
    for stmt in &mut program.statements {
        reassign_statement_expr_node_ids(stmt, &mut allocator);
    }
}

pub fn reassign_statement_expr_node_ids(stmt: &mut Statement, allocator: &mut NodeIdAllocator) {
    match stmt {
        Statement::Expr(expr) => reassign_expr_node_ids(expr, allocator),
        Statement::ClassLikeDef(def) => {
            for field in &mut def.fields {
                if let Some(default_value) = &mut field.default_value {
                    reassign_expr_node_ids(default_value, allocator);
                }
                if let Some(method_def) = &mut field.method_def {
                    reassign_func_expr_node_ids(method_def, allocator);
                }
            }
        }
        Statement::FuncDef(func) => reassign_func_expr_node_ids(func, allocator),
        Statement::Assignment(assign) => reassign_expr_node_ids(&mut assign.value, allocator),
        Statement::ErrorCeiling(ceiling) => {
            for stmt in &mut ceiling.handler_body {
                reassign_statement_expr_node_ids(stmt, allocator);
            }
        }
        Statement::UnmoldForward(stmt) => reassign_expr_node_ids(&mut stmt.source, allocator),
        Statement::UnmoldBackward(stmt) => reassign_expr_node_ids(&mut stmt.source, allocator),
        Statement::EnumDef(_) | Statement::Import(_) | Statement::Export(_) => {}
    }
}

pub fn reassign_func_expr_node_ids(func: &mut FuncDef, allocator: &mut NodeIdAllocator) {
    for param in &mut func.params {
        if let Some(default_value) = &mut param.default_value {
            reassign_expr_node_ids(default_value, allocator);
        }
    }
    for stmt in &mut func.body {
        reassign_statement_expr_node_ids(stmt, allocator);
    }
}

pub fn reassign_expr_node_ids(expr: &mut Expr, allocator: &mut NodeIdAllocator) {
    expr.span_mut().node_id = allocator.fresh();

    match expr {
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
            for field in fields {
                reassign_expr_node_ids(&mut field.value, allocator);
            }
        }
        Expr::ListLit(items, _) | Expr::Pipeline(items, _) => {
            for item in items {
                reassign_expr_node_ids(item, allocator);
            }
        }
        Expr::BinaryOp(left, _, right, _) => {
            reassign_expr_node_ids(left, allocator);
            reassign_expr_node_ids(right, allocator);
        }
        Expr::UnaryOp(_, inner, _)
        | Expr::FieldAccess(inner, _, _)
        | Expr::Unmold(inner, _)
        | Expr::Throw(inner, _) => {
            reassign_expr_node_ids(inner, allocator);
        }
        Expr::FuncCall(func, args, _) => {
            reassign_expr_node_ids(func, allocator);
            for arg in args {
                reassign_expr_node_ids(arg, allocator);
            }
        }
        Expr::MethodCall(receiver, _, args, _) => {
            reassign_expr_node_ids(receiver, allocator);
            for arg in args {
                reassign_expr_node_ids(arg, allocator);
            }
        }
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(condition) = &mut arm.condition {
                    reassign_expr_node_ids(condition, allocator);
                }
                for stmt in &mut arm.body {
                    reassign_statement_expr_node_ids(stmt, allocator);
                }
            }
        }
        Expr::MoldInst(_, type_args, fields, _) => {
            for arg in type_args {
                reassign_expr_node_ids(arg, allocator);
            }
            for field in fields {
                reassign_expr_node_ids(&mut field.value, allocator);
            }
        }
        Expr::Lambda(params, body, _) => {
            for param in params {
                if let Some(default_value) = &mut param.default_value {
                    reassign_expr_node_ids(default_value, allocator);
                }
            }
            reassign_expr_node_ids(body, allocator);
        }
        Expr::IntLit(_, _)
        | Expr::FloatLit(_, _)
        | Expr::StringLit(_, _)
        | Expr::TemplateLit(_, _)
        | Expr::BoolLit(_, _)
        | Expr::Gorilla(_)
        | Expr::Ident(_, _)
        | Expr::Placeholder(_)
        | Expr::Hole(_)
        | Expr::EnumVariant(_, _, _)
        | Expr::TypeLiteral(_, _, _) => {}
    }
}

/// A field in a buchi pack literal or type instantiation.
#[derive(Debug, Clone, PartialEq)]
pub struct BuchiField {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

/// A condition arm in a condition branch.
#[derive(Debug, Clone, PartialEq)]
pub struct CondArm {
    /// `None` means the default case `| _ |>`
    pub condition: Option<Expr>,
    /// Body statements. The last statement's expression value is the arm result.
    pub body: Vec<Statement>,
    pub span: Span,
}

impl CondArm {
    /// Get the last expression in the body (used for type inference, graph extraction, etc.).
    /// Returns None if the body is empty or the last statement is not an expression.
    pub fn last_expr(&self) -> Option<&Expr> {
        self.body.last().and_then(|stmt| match stmt {
            Statement::Expr(e) => Some(e),
            _ => None,
        })
    }

    /// Get the body as a single expression reference (for backward compatibility).
    /// Panics if the body has more than one statement or the statement is not an expression.
    /// For safe access, use `last_expr()` instead.
    pub fn body_expr(&self) -> &Expr {
        debug_assert_eq!(
            self.body.len(),
            1,
            "body_expr() called on multi-statement arm"
        );
        match &self.body[0] {
            Statement::Expr(e) => e,
            other => panic!(
                "body_expr() called on non-expression arm: {:?}",
                std::mem::discriminant(other)
            ),
        }
    }
}

/// Binary operators.
#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    // Arithmetic
    Add,
    Sub,
    Mul,
    // Comparison
    Eq,
    NotEq,
    Lt,
    Gt,
    GtEq,
    // Logical
    And,
    Or,
    // String concatenation (uses +)
    Concat,
}

/// Unary operators.
#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOp {
    Neg,
    Not,
}

/// Check if an expression contains a pipeline placeholder `_` anywhere in
/// its tree (including nested forms like `_ > 3`).
///
/// This single definition decides whether a pipeline stage call receives
/// the piped value by placeholder substitution (`data => f(_)`) or by
/// implicit first-argument injection (`data => f()`). The interpreter's
/// pipeline-step evaluation and the type checker both consume it so the
/// two can never disagree on that rule.
pub fn expr_contains_placeholder(expr: &Expr) -> bool {
    match expr {
        Expr::Placeholder(_) => true,
        Expr::BuchiPack(fields, _) => fields
            .iter()
            .any(|field| expr_contains_placeholder(&field.value)),
        Expr::ListLit(items, _) => items.iter().any(expr_contains_placeholder),
        Expr::BinaryOp(lhs, _, rhs, _) => {
            expr_contains_placeholder(lhs) || expr_contains_placeholder(rhs)
        }
        Expr::UnaryOp(_, inner, _) => expr_contains_placeholder(inner),
        Expr::FuncCall(callee, args, _) => {
            expr_contains_placeholder(callee) || args.iter().any(expr_contains_placeholder)
        }
        Expr::MethodCall(obj, _, args, _) => {
            expr_contains_placeholder(obj) || args.iter().any(expr_contains_placeholder)
        }
        Expr::FieldAccess(obj, _, _) => expr_contains_placeholder(obj),
        Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
            arm.condition
                .as_ref()
                .is_some_and(expr_contains_placeholder)
                || arm.body.iter().any(statement_contains_placeholder)
        }),
        Expr::Pipeline(steps, _) => steps.iter().any(expr_contains_placeholder),
        Expr::MoldInst(_, type_args, fields, _) => {
            type_args.iter().any(expr_contains_placeholder)
                || fields
                    .iter()
                    .any(|field| expr_contains_placeholder(&field.value))
        }
        Expr::Unmold(inner, _) => expr_contains_placeholder(inner),
        Expr::Lambda(params, body, _) => {
            params.iter().any(|param| {
                param
                    .default_value
                    .as_ref()
                    .is_some_and(expr_contains_placeholder)
            }) || expr_contains_placeholder(body)
        }
        Expr::TypeInst(_, fields, _) => fields
            .iter()
            .any(|field| expr_contains_placeholder(&field.value)),
        Expr::Throw(inner, _) => expr_contains_placeholder(inner),
        _ => false,
    }
}

/// Statement-level companion of [`expr_contains_placeholder`].
pub fn statement_contains_placeholder(stmt: &Statement) -> bool {
    match stmt {
        Statement::Expr(expr) => expr_contains_placeholder(expr),
        Statement::Assignment(assign) => expr_contains_placeholder(&assign.value),
        Statement::FuncDef(func) => func.body.iter().any(statement_contains_placeholder),
        Statement::ErrorCeiling(ceiling) => ceiling
            .handler_body
            .iter()
            .any(statement_contains_placeholder),
        Statement::UnmoldForward(unmold) => expr_contains_placeholder(&unmold.source),
        Statement::UnmoldBackward(unmold) => expr_contains_placeholder(&unmold.source),
        Statement::ClassLikeDef(def) => def.fields.iter().any(|field| {
            field
                .default_value
                .as_ref()
                .is_some_and(expr_contains_placeholder)
                || field
                    .method_def
                    .as_ref()
                    .is_some_and(|func| func.body.iter().any(statement_contains_placeholder))
        }),
        Statement::EnumDef(_) | Statement::Import(_) | Statement::Export(_) => false,
    }
}

/// Consume plan for sequential-Append tail recursion
/// (`f(n - 1, Append[acc, x]())`): the runtimes copy the whole
/// accumulator per element — O(n²) — unless they can prove the
/// accumulator's only use in every self-tail-calling arm is the
/// Append's first argument. This shared analysis mirrors the native
/// backend's IR-level pass; the Interpreter and JS backends key their
/// consume fast paths off `sites` (expression `node_id`s), so an
/// `Append` that did not pass the shape check here can never be
/// consumed. Everything fails closed.
pub struct AppendConsumePlan {
    /// The accumulator parameter's name.
    pub param: String,
    /// The accumulator's position in the parameter list (and therefore
    /// in every full-arity self call).
    pub param_idx: usize,
    /// `node_id` of each `Append[p, item]()` MoldInst that is safe to
    /// consume.
    pub sites: std::collections::HashSet<usize>,
}

/// Analyse a function body for the consumable-Append shape.
///
/// Accepted shape (everything else returns `None`):
/// - the body is a single `CondBranch` expression statement,
/// - no parameter declares a default value (a short self call would
///   evaluate defaults after the explicit args and may read the
///   consumed list),
/// - every arm whose single statement is a full-arity self call has
///   the candidate parameter appear exactly once — as the first
///   argument of an `Append[p, item]()` in the parameter's own slot —
///   with every other argument scalar-safe (no calls, no lambdas, no
///   reference to `p` or the function itself),
/// - no arm condition mentions the candidate parameter.
///
/// Arms that are not such a self call cannot reach a consume site (the
/// sites are keyed by node id), so they are unconstrained.
pub fn append_consume_plan(
    fname: &str,
    params: &[Param],
    body: &[Statement],
) -> Option<AppendConsumePlan> {
    if params.is_empty() || params.iter().any(|p| p.default_value.is_some()) {
        return None;
    }
    let arity = params.len();
    let [Statement::Expr(Expr::CondBranch(arms, _))] = body else {
        return None;
    };
    'params: for (p_idx, p) in params.iter().enumerate() {
        let pname = p.name.as_str();
        let mut sites = std::collections::HashSet::new();
        for arm in arms {
            // The condition must mention neither the accumulator nor
            // the function itself — a self call inside a condition would
            // make "every self call is a bare tail call" unprovable.
            if let Some(cond) = &arm.condition
                && (expr_mentions_ident(cond, pname) || expr_mentions_ident(cond, fname))
            {
                continue 'params;
            }
            let [Statement::Expr(Expr::FuncCall(callee, args, _))] = arm.body.as_slice() else {
                continue; // not a bare call — cannot host a consume site
            };
            if !matches!(callee.as_ref(), Expr::Ident(n, _) if n == fname) {
                continue; // call to something else — unconstrained
            }
            if args.len() != arity {
                continue 'params; // default completion may read p
            }
            let Some(Expr::MoldInst(mname, targs, mfields, mspan)) = args.get(p_idx) else {
                continue 'params;
            };
            if mname != "Append" || targs.len() != 2 || !mfields.is_empty() {
                continue 'params;
            }
            if !matches!(&targs[0], Expr::Ident(n, _) if n == pname) {
                continue 'params;
            }
            if !append_consume_scalar_safe(&targs[1], pname, fname) {
                continue 'params;
            }
            for (i, a) in args.iter().enumerate() {
                if i != p_idx && !append_consume_scalar_safe(a, pname, fname) {
                    continue 'params;
                }
            }
            sites.insert(mspan.node_id);
        }
        if !sites.is_empty() {
            return Some(AppendConsumePlan {
                param: pname.to_string(),
                param_idx: p_idx,
                sites,
            });
        }
    }
    None
}

/// Scalar-safe: literals, identifiers other than the accumulator and
/// the function itself, and arithmetic over those. Calls, lambdas,
/// containers — anything that could alias or re-enter — fail closed.
fn append_consume_scalar_safe(e: &Expr, pname: &str, fname: &str) -> bool {
    match e {
        Expr::Ident(n, _) => n != pname && n != fname,
        Expr::IntLit(..) | Expr::FloatLit(..) | Expr::BoolLit(..) | Expr::StringLit(..) => true,
        Expr::BinaryOp(l, _, r, _) => {
            append_consume_scalar_safe(l, pname, fname)
                && append_consume_scalar_safe(r, pname, fname)
        }
        Expr::UnaryOp(_, x, _) => append_consume_scalar_safe(x, pname, fname),
        _ => false,
    }
}

/// Whether `name` is mentioned anywhere inside `e`. Lambdas count as a
/// mention (closure capture reaches every visible binding), as do
/// template literals containing the name and any statement form we do
/// not model — this is a fail-closed guard, not a precise use check.
fn expr_mentions_ident(e: &Expr, name: &str) -> bool {
    match e {
        Expr::Ident(n, _) => n == name,
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::BoolLit(..)
        | Expr::Gorilla(..)
        | Expr::Placeholder(..)
        | Expr::Hole(..)
        | Expr::TypeLiteral(..)
        | Expr::EnumVariant(..) => false,
        Expr::TemplateLit(s, _) => s.contains(name),
        Expr::Lambda(..) => true,
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
            fields.iter().any(|f| expr_mentions_ident(&f.value, name))
        }
        Expr::ListLit(items, _) | Expr::Pipeline(items, _) => {
            items.iter().any(|i| expr_mentions_ident(i, name))
        }
        Expr::BinaryOp(l, _, r, _) => expr_mentions_ident(l, name) || expr_mentions_ident(r, name),
        Expr::UnaryOp(_, x, _) | Expr::Unmold(x, _) | Expr::Throw(x, _) => {
            expr_mentions_ident(x, name)
        }
        Expr::FuncCall(c, args, _) => {
            expr_mentions_ident(c, name) || args.iter().any(|a| expr_mentions_ident(a, name))
        }
        Expr::MethodCall(recv, _, args, _) => {
            expr_mentions_ident(recv, name) || args.iter().any(|a| expr_mentions_ident(a, name))
        }
        Expr::FieldAccess(x, _, _) => expr_mentions_ident(x, name),
        Expr::CondBranch(arms, _) => arms.iter().any(|arm| {
            arm.condition
                .as_ref()
                .is_some_and(|c| expr_mentions_ident(c, name))
                || arm.body.iter().any(|st| stmt_mentions_ident(st, name))
        }),
        Expr::MoldInst(_, targs, fields, _) => {
            targs.iter().any(|t| expr_mentions_ident(t, name))
                || fields.iter().any(|f| expr_mentions_ident(&f.value, name))
        }
    }
}

fn stmt_mentions_ident(st: &Statement, name: &str) -> bool {
    match st {
        Statement::Expr(e) => expr_mentions_ident(e, name),
        Statement::Assignment(a) => expr_mentions_ident(&a.value, name),
        Statement::UnmoldForward(u) => expr_mentions_ident(&u.source, name),
        Statement::UnmoldBackward(u) => expr_mentions_ident(&u.source, name),
        _ => true, // unmodeled statement form — fail closed
    }
}
