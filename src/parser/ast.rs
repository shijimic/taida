use crate::lexer::Span;

/// Top-level program: a sequence of statements.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub statements: Vec<Statement>,
}

/// A statement in Taida.
#[derive(Debug, Clone, PartialEq)]
pub enum Statement {
    /// Expression statement (expression evaluated for its value or side effects).
    Expr(Expr),
    /// Enum definition: `Enum => Name = :A :B`
    EnumDef(EnumDef),
    /// Type definition: `Name = @(...)`
    TypeDef(TypeDef),
    /// Function definition: `name params = body => :ReturnType`
    FuncDef(FuncDef),
    /// Variable assignment: `name <= expr` or `expr => name`
    Assignment(Assignment),
    /// Mold type definition: `Mold[T] => Name[T] = @(...)`
    MoldDef(MoldDef),
    /// Inheritance definition: `Parent => Child = @(...)`
    InheritanceDef(InheritanceDef),
    /// Error ceiling: `|== error: Type = body => :ReturnType`
    ErrorCeiling(ErrorCeiling),
    /// Import: `>>> path => @(symbols)`
    Import(ImportStmt),
    /// Export: `<<< @(symbols)` or `<<< symbol`
    Export(ExportStmt),
    /// Unmold forward: `expr ]=> name`
    UnmoldForward(UnmoldForwardStmt),
    /// Unmold backward: `name <=[ expr`
    UnmoldBackward(UnmoldBackwardStmt),
}

impl Statement {
    /// C13-1: For a "value-yielding" statement (the tail of an expression
    /// block), return a reference to the `Expr` whose value is the block
    /// result. Tail bindings (`name <= expr`, `expr => name`, `expr ]=> name`,
    /// `name <=[ expr`) yield their RHS source expression; a plain
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

    /// C13-1: True if this statement represents a tail binding form
    /// (`name <= expr`, `expr => name`, `expr ]=> name`, `name <=[ expr`)
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

/// Type definition: `Name = @(field: Type, ...)`
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDef {
    pub name: String,
    pub fields: Vec<FieldDef>,
    /// Documentation comments (`///@`) attached to this type definition.
    pub doc_comments: Vec<String>,
    pub span: Span,
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

/// Mold header argument in `Mold[...]` / `Name[...]`.
#[derive(Debug, Clone, PartialEq)]
pub enum MoldHeaderArg {
    /// Type variable, optionally with a constraint.
    TypeParam(TypeParam),
    /// Concrete type expression introduced with `:`.
    Concrete(TypeExpr),
}

/// Mold type definition: `Mold[...] => Name[...] = @(...)`
#[derive(Debug, Clone, PartialEq)]
pub struct MoldDef {
    pub name: String,
    /// Header arguments declared on the `Mold[...]` side.
    pub mold_args: Vec<MoldHeaderArg>,
    /// Header arguments declared on the `Name[...]` side, if explicitly present.
    pub name_args: Option<Vec<MoldHeaderArg>>,
    /// Declared type variables extracted from `mold_args`.
    pub type_params: Vec<TypeParam>,
    pub fields: Vec<FieldDef>,
    /// Documentation comments (`///@`) attached to this mold definition.
    pub doc_comments: Vec<String>,
    pub span: Span,
}

/// Type parameter, optionally with constraint.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeParam {
    pub name: String,
    pub constraint: Option<TypeExpr>,
}

/// Inheritance definition: `Parent => Child = @(...)`
#[derive(Debug, Clone, PartialEq)]
pub struct InheritanceDef {
    pub parent: String,
    /// Header arguments declared on the `Parent[...]` side, if present.
    pub parent_args: Option<Vec<MoldHeaderArg>>,
    pub child: String,
    /// Header arguments declared on the `Child[...]` side, if present.
    pub child_args: Option<Vec<MoldHeaderArg>>,
    pub fields: Vec<FieldDef>,
    /// Documentation comments (`///@`) attached to this inheritance definition.
    pub doc_comments: Vec<String>,
    pub span: Span,
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

/// Unmold forward: `expr ]=> name`
#[derive(Debug, Clone, PartialEq)]
pub struct UnmoldForwardStmt {
    pub source: Expr,
    pub target: String,
    pub span: Span,
}

/// Unmold backward: `name <=[ expr`
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
    /// Hole: empty slot in function call for partial application `f(5, )`
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

    /// Unmold expression: `expr.unmold()` or `]=>` as expr
    Unmold(Box<Expr>, Span),

    /// Anonymous function: `_ x = x * 2`
    Lambda(Vec<Param>, Box<Expr>, Span),

    /// Type instantiation: `TypeName(field <= value, ...)`
    TypeInst(String, Vec<BuchiField>, Span),

    /// Enum value constructor: `Name:Variant()`
    EnumVariant(String, String, Span),

    /// B11-6a: Restricted type literal inside mold args.
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
