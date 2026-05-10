//! Typed HIR side table.
//!
//! The type-checker records the inferred `Type` of every observed
//! `Expr` here. Codegen lowering consumes the table by id so the
//! "is this expression a Bool?" decision becomes a typed lookup,
//! removing the historical method-name allow-list and reducing the
//! number of places that have to re-derive type information from
//! syntax.
//!
//! ## Design notes
//!
//! - **`ExprId` is a span-based hash** so the parser does not have to
//!   carry a node id field. The id is `(start, end, discriminant_tag)`.
//!   Within a single program every observed span is unique.
//! - **Side table, not an AST mirror**. The table lives next to the
//!   untyped AST and is keyed by id; consumers query it explicitly.
//! - **`record(&Expr, Type)` is idempotent**: a second record for the
//!   same expression overwrites the first, which lets bidirectional
//!   lambda inference replace an earlier `Type::Unknown` placeholder
//!   with a hint-resolved function type.

use std::collections::HashMap;

use crate::parser::Expr;

use super::types::Type;

/// Stable identifier of an `Expr` in the AST.
///
/// `(start, end, discriminant_tag)` triple keyed off the span. Within a
/// single program a `(span, variant)` pair uniquely identifies an
/// expression without changing the parser to carry a dedicated node id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprId {
    start: usize,
    end: usize,
    discriminant: u8,
}

impl ExprId {
    /// Compute a stable id from an expression's span + variant discriminant.
    pub fn from_expr(expr: &Expr) -> Self {
        let span = expr.span();
        Self {
            start: span.start,
            end: span.end,
            discriminant: expr_discriminant(expr),
        }
    }
}

/// AST → Type side table populated by the type-checker and consumed by
/// codegen lowering for typed queries (`is_bool`, `lookup`).
#[derive(Debug, Clone, Default)]
pub struct TypedExprTable {
    types: HashMap<ExprId, Type>,
}

impl TypedExprTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the inferred type of `expr`. Idempotent: a second record
    /// for the same expression overwrites the first (last write wins),
    /// which lets bidirectional lambda inference upgrade an earlier
    /// placeholder with the hint-resolved function type.
    pub fn record(&mut self, expr: &Expr, ty: Type) {
        let id = ExprId::from_expr(expr);
        self.types.insert(id, ty);
    }

    /// Lookup the type of `expr`. Returns `None` if not recorded.
    pub fn lookup(&self, expr: &Expr) -> Option<&Type> {
        let id = ExprId::from_expr(expr);
        self.types.get(&id)
    }

    /// Convenience for codegen: returns `true` iff the recorded type
    /// for `expr` is exactly `Type::Bool`.
    pub fn is_bool(&self, expr: &Expr) -> bool {
        matches!(self.lookup(expr), Some(Type::Bool))
    }

    /// Number of recorded entries. Used for diagnostics / introspection.
    pub fn len(&self) -> usize {
        self.types.len()
    }

    /// True if no entries recorded.
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    /// Returns `true` if any recorded type still contains a residual
    /// `Type::Unknown`. The full-pin contract for `Lax` / `Result` /
    /// `Async` method signatures requires the table to be free of
    /// residuals after the type-checker completes; this helper drives
    /// that fixture-level assertion.
    pub fn has_residual_unknown(&self) -> bool {
        self.types.values().any(|t| t.contains_concrete_unknown())
    }

    /// Iterate over all recorded `(ExprId, Type)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&ExprId, &Type)> {
        self.types.iter()
    }
}

/// Discriminant tag for each `Expr` variant. Used in `ExprId` to disambiguate
/// expressions that happen to share a `Span` (rare but theoretically possible
/// when parser nests expressions).
fn expr_discriminant(expr: &Expr) -> u8 {
    match expr {
        Expr::IntLit(_, _) => 1,
        Expr::FloatLit(_, _) => 2,
        Expr::StringLit(_, _) => 3,
        Expr::TemplateLit(_, _) => 4,
        Expr::BoolLit(_, _) => 5,
        Expr::Gorilla(_) => 6,
        Expr::Ident(_, _) => 7,
        Expr::Placeholder(_) => 8,
        Expr::Hole(_) => 9,
        Expr::BuchiPack(_, _) => 10,
        Expr::ListLit(_, _) => 11,
        Expr::BinaryOp(_, _, _, _) => 12,
        Expr::UnaryOp(_, _, _) => 13,
        Expr::FuncCall(_, _, _) => 14,
        Expr::MethodCall(_, _, _, _) => 15,
        Expr::FieldAccess(_, _, _) => 16,
        Expr::CondBranch(_, _) => 17,
        Expr::Pipeline(_, _) => 18,
        Expr::MoldInst(_, _, _, _) => 19,
        Expr::Unmold(_, _) => 20,
        Expr::Lambda(_, _, _) => 21,
        Expr::TypeInst(_, _, _) => 22,
        Expr::EnumVariant(_, _, _) => 23,
        Expr::TypeLiteral(_, _, _) => 24,
        Expr::Throw(_, _) => 25,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::Span;

    fn span(start: usize, end: usize) -> Span {
        Span::new(start, end, 1, 1)
    }

    #[test]
    fn record_and_lookup_basic() {
        let mut table = TypedExprTable::new();
        let expr = Expr::IntLit(42, span(0, 2));
        assert!(table.is_empty());
        table.record(&expr, Type::Int);
        assert_eq!(table.len(), 1);
        assert_eq!(table.lookup(&expr), Some(&Type::Int));
    }

    #[test]
    fn record_idempotent_overwrites_with_last_write() {
        let mut table = TypedExprTable::new();
        let expr = Expr::IntLit(42, span(0, 2));
        table.record(&expr, Type::Unknown);
        table.record(&expr, Type::Int);
        // Last-write wins (Phase 1.4 hint-付き path 上書き対応)
        assert_eq!(table.lookup(&expr), Some(&Type::Int));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn is_bool_query() {
        let mut table = TypedExprTable::new();
        let bool_expr = Expr::BoolLit(true, span(0, 4));
        let int_expr = Expr::IntLit(42, span(5, 7));
        table.record(&bool_expr, Type::Bool);
        table.record(&int_expr, Type::Int);
        assert!(table.is_bool(&bool_expr));
        assert!(!table.is_bool(&int_expr));
    }

    #[test]
    fn has_residual_unknown_detects_unknown() {
        let mut table = TypedExprTable::new();
        let e1 = Expr::IntLit(1, span(0, 1));
        let e2 = Expr::IntLit(2, span(2, 3));
        table.record(&e1, Type::Int);
        assert!(!table.has_residual_unknown());
        table.record(&e2, Type::Unknown);
        assert!(table.has_residual_unknown());
    }

    #[test]
    fn has_residual_unknown_detects_nested_unknown() {
        let mut table = TypedExprTable::new();
        let e = Expr::ListLit(vec![], span(0, 2));
        table.record(&e, Type::List(Box::new(Type::Unknown)));
        assert!(table.has_residual_unknown());
    }

    #[test]
    fn discriminant_disambiguates_same_span() {
        // 2 expressions with the same span (theoretical edge case) get
        // different ids via discriminant.
        let mut table = TypedExprTable::new();
        let int_expr = Expr::IntLit(0, span(0, 1));
        let bool_expr = Expr::BoolLit(true, span(0, 1));
        table.record(&int_expr, Type::Int);
        table.record(&bool_expr, Type::Bool);
        assert_eq!(table.lookup(&int_expr), Some(&Type::Int));
        assert_eq!(table.lookup(&bool_expr), Some(&Type::Bool));
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn lookup_returns_none_for_unrecorded() {
        let table = TypedExprTable::new();
        let expr = Expr::IntLit(42, span(0, 2));
        assert!(table.lookup(&expr).is_none());
        assert!(!table.is_bool(&expr));
    }

    #[test]
    fn iter_yields_all_recorded() {
        let mut table = TypedExprTable::new();
        let e1 = Expr::IntLit(1, span(0, 1));
        let e2 = Expr::StringLit("x".to_string(), span(2, 5));
        table.record(&e1, Type::Int);
        table.record(&e2, Type::Str);
        let entries: Vec<_> = table.iter().collect();
        assert_eq!(entries.len(), 2);
    }
}
