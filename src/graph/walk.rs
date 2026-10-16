//! Shared traversal of the AST for the graph, verify and release-gate
//! analyses.
//!
//! Each analysis used to carry its own recursion over [`Expr`], and every
//! copy stopped at a different set of expression kinds: a call inside a
//! unary operator, a pack field or a template interpolation was visible to
//! one analysis and invisible to the next. The functions here list the
//! children of a node with no catch-all arm, so a new AST variant is a
//! compile error here instead of a silent gap in every analysis.
//!
//! The analyses recurse once per level of the tree, and a left-associative
//! chain such as `1 + 1 + ... + 1` is a tree as deep as it is long.
//! [`with_stack_for`] runs an analysis on a thread whose stack is sized for
//! the program it walks.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::lexer::Span;
use crate::parser::*;

/// A direct child of an expression or statement.
#[derive(Clone, Copy)]
pub enum Node<'a> {
    Expr(&'a Expr),
    Stmt(&'a Statement),
}

/// Calls `f` for every direct child of `expr`, in source order.
///
/// A template literal keeps its `${...}` bodies as source text, so they are
/// parsed here and handed to `f` for the duration of the call; the
/// expressions carry the line of the interpolation.
pub fn for_each_child_of_expr(expr: &Expr, f: &mut dyn FnMut(Node<'_>)) {
    if let Expr::TemplateLit(template, span) = expr {
        for inner in template_interpolations(template, span) {
            f(Node::Expr(&inner));
        }
        return;
    }
    let mut children = Vec::new();
    direct_children_of_expr(expr, &mut children);
    for child in children {
        f(child);
    }
}

/// Appends the direct children of `expr` that live in the syntax tree
/// itself, in source order. The parsed `${...}` bodies of a template literal
/// do not; [`for_each_child_of_expr`] covers those.
pub fn direct_children_of_expr<'a>(expr: &'a Expr, out: &mut Vec<Node<'a>>) {
    match expr {
        Expr::IntLit(..)
        | Expr::FloatLit(..)
        | Expr::StringLit(..)
        | Expr::TemplateLit(..)
        | Expr::BoolLit(..)
        | Expr::Gorilla(..)
        | Expr::Ident(..)
        | Expr::Placeholder(..)
        | Expr::Hole(..)
        | Expr::EnumVariant(..)
        | Expr::TypeLiteral(..) => {}
        Expr::Block(stmts, _) => out.extend(stmts.iter().map(Node::Stmt)),
        Expr::BuchiPack(fields, _) | Expr::TypeInst(_, fields, _) => {
            out.extend(fields.iter().map(|field| Node::Expr(&field.value)));
        }
        Expr::ListLit(items, _) | Expr::Pipeline(items, _) => {
            out.extend(items.iter().map(Node::Expr));
        }
        Expr::BinaryOp(left, _, right, _) => {
            out.push(Node::Expr(left));
            out.push(Node::Expr(right));
        }
        Expr::UnaryOp(_, inner, _)
        | Expr::FieldAccess(inner, _, _)
        | Expr::Unmold(inner, _)
        | Expr::Throw(inner, _) => out.push(Node::Expr(inner)),
        Expr::FuncCall(callee, args, _) => {
            out.push(Node::Expr(callee));
            out.extend(args.iter().map(Node::Expr));
        }
        Expr::MethodCall(receiver, _, args, _) => {
            out.push(Node::Expr(receiver));
            out.extend(args.iter().map(Node::Expr));
        }
        Expr::CondBranch(arms, _) => {
            for arm in arms {
                if let Some(cond) = &arm.condition {
                    out.push(Node::Expr(cond));
                }
                out.extend(arm.body.iter().map(Node::Stmt));
            }
        }
        Expr::MoldInst(_, type_args, fields, _) => {
            out.extend(type_args.iter().map(Node::Expr));
            out.extend(fields.iter().map(|field| Node::Expr(&field.value)));
        }
        Expr::Lambda(params, body, _) => {
            out.extend(
                params
                    .iter()
                    .filter_map(|param| param.default_value.as_ref().map(Node::Expr)),
            );
            out.push(Node::Expr(body));
        }
    }
}

/// Calls `f` for every direct child of `stmt`, in source order.
///
/// The bodies of functions and methods are children too: an analysis that
/// wants to treat a function as a separate scope stops recursing at
/// [`Statement::FuncDef`] itself.
pub fn for_each_child_of_stmt(stmt: &Statement, f: &mut dyn FnMut(Node<'_>)) {
    let mut children = Vec::new();
    direct_children_of_stmt(stmt, &mut children);
    for child in children {
        f(child);
    }
}

/// Appends the direct children of `stmt`, in source order.
pub fn direct_children_of_stmt<'a>(stmt: &'a Statement, out: &mut Vec<Node<'a>>) {
    match stmt {
        Statement::Expr(expr) => out.push(Node::Expr(expr)),
        Statement::EnumDef(_) | Statement::Import(_) | Statement::Export(_) => {}
        Statement::ClassLikeDef(cl) => {
            for field in &cl.fields {
                if let Some(default) = &field.default_value {
                    out.push(Node::Expr(default));
                }
                if let Some(method) = &field.method_def {
                    direct_children_of_func(method, out);
                }
            }
        }
        Statement::FuncDef(fd) => direct_children_of_func(fd, out),
        Statement::Assignment(assign) => out.push(Node::Expr(&assign.value)),
        Statement::ErrorCeiling(ec) => out.extend(ec.handler_body.iter().map(Node::Stmt)),
        Statement::UnmoldForward(unmold) => out.push(Node::Expr(&unmold.source)),
        Statement::UnmoldBackward(unmold) => out.push(Node::Expr(&unmold.source)),
    }
}

/// Calls `f` for the parameter defaults and the body statements of `fd`.
pub fn for_each_child_of_func(fd: &FuncDef, f: &mut dyn FnMut(Node<'_>)) {
    let mut children = Vec::new();
    direct_children_of_func(fd, &mut children);
    for child in children {
        f(child);
    }
}

fn direct_children_of_func<'a>(fd: &'a FuncDef, out: &mut Vec<Node<'a>>) {
    out.extend(
        fd.params
            .iter()
            .filter_map(|param| param.default_value.as_ref().map(Node::Expr)),
    );
    out.extend(fd.body.iter().map(Node::Stmt));
}

/// Calls `f` for `stmt` and every statement nested inside it (bodies of
/// functions, methods, arms and blocks), depth first.
pub fn for_each_nested_stmt(stmt: &Statement, f: &mut dyn FnMut(&Statement)) {
    f(stmt);
    for_each_child_of_stmt(stmt, &mut |child| nested_in_child(child, f));
}

/// [`for_each_nested_stmt`] for the statements inside `expr`.
pub fn for_each_nested_stmt_in_expr(expr: &Expr, f: &mut dyn FnMut(&Statement)) {
    for_each_child_of_expr(expr, &mut |child| nested_in_child(child, f));
}

fn nested_in_child(child: Node<'_>, f: &mut dyn FnMut(&Statement)) {
    match child {
        Node::Stmt(stmt) => for_each_nested_stmt(stmt, f),
        Node::Expr(expr) => for_each_nested_stmt_in_expr(expr, f),
    }
}

/// The expressions written in the `${...}` slots of a template literal, in
/// source order. A slot whose body does not parse to a single expression is
/// left out, as the evaluators treat it as literal text or reject it.
///
/// `span` is the template literal's own span: each expression is parsed as
/// starting on the line where its `${` is written.
pub fn template_interpolations(template: &str, span: &Span) -> Vec<Expr> {
    interpolation_sources(template, span)
        .into_iter()
        .filter_map(|(body, line)| {
            let (program, errors) = parse_from_line(&body, line);
            if errors.is_empty()
                && let Some(Statement::Expr(expr)) = program.statements.into_iter().next()
            {
                Some(expr)
            } else {
                None
            }
        })
        .collect()
}

/// The source text of each `${...}` slot of a template and the line it starts
/// on, in source order.
fn interpolation_sources(template: &str, span: &Span) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let mut chars = template.chars().peekable();
    let mut line = span.line.max(1);
    while let Some(ch) = chars.next() {
        if ch == '\n' {
            line += 1;
        } else if ch == '$' && chars.peek() == Some(&'{') {
            chars.next();
            let mut body = String::new();
            let mut depth = 1;
            let body_line = line;
            for c in chars.by_ref() {
                if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                if c == '\n' {
                    line += 1;
                }
                body.push(c);
            }
            out.push((body, body_line));
        }
    }
    out
}

/// What a bound name stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// A parameter, a local value or a lambda parameter.
    Value,
    /// A function defined inside another one, by the id of its node.
    Function(String),
}

/// The names bound at a point of a function, for telling a call of a local
/// value from a call of a top-level function that happens to be named alike.
///
/// A parameter binds its name for the whole body, a lambda parameter for the
/// lambda's body, a binding for what comes after it in the block it is
/// written in (the right-hand side of a binding is read before the name is
/// bound), and a nested function for its own body and what comes after it. The analyses keep one while they walk: they
/// take a [`mark`](Bindings::mark) on entering a scope and
/// [`release`](Bindings::release) it on leaving.
#[derive(Default)]
pub struct Bindings {
    stack: Vec<(String, Binding)>,
    /// Where in `stack` each name is bound, the innermost binding last.
    positions: HashMap<String, Vec<usize>>,
}

impl Bindings {
    /// The point to go back to when the scope being entered ends.
    pub fn mark(&self) -> usize {
        self.stack.len()
    }

    /// Forget every name bound since `mark`.
    pub fn release(&mut self, mark: usize) {
        while self.stack.len() > mark {
            let Some((name, _)) = self.stack.pop() else {
                break;
            };
            if let Some(positions) = self.positions.get_mut(&name) {
                positions.pop();
                if positions.is_empty() {
                    self.positions.remove(&name);
                }
            }
        }
    }

    pub fn bind(&mut self, name: &str) {
        self.push(name, Binding::Value);
    }

    /// Bind `name` to a function defined in the code being walked.
    pub fn bind_function(&mut self, name: &str, id: String) {
        self.push(name, Binding::Function(id));
    }

    pub fn bind_params(&mut self, params: &[Param]) {
        for param in params {
            self.push(&param.name, Binding::Value);
        }
    }

    pub fn is_bound(&self, name: &str) -> bool {
        self.positions.contains_key(name)
    }

    /// What `name` is bound to at this point, if it is.
    pub fn lookup(&self, name: &str) -> Option<&Binding> {
        let at = *self.positions.get(name)?.last()?;
        self.stack.get(at).map(|(_, binding)| binding)
    }

    fn push(&mut self, name: &str, binding: Binding) {
        self.positions
            .entry(name.to_string())
            .or_default()
            .push(self.stack.len());
        self.stack.push((name.to_string(), binding));
    }
}

/// Nesting depth up to which an analysis runs on the calling thread.
const INLINE_DEPTH_LIMIT: usize = 32;
/// Stack reserved for each level of nesting. Generous on purpose: an
/// unoptimised build uses tens of kilobytes per level in the type checker
/// (the heaviest walker), and only the pages actually touched are committed.
const STACK_PER_LEVEL: usize = 256 * 1024;
/// Smallest stack of a thread started for a deep program.
const MIN_STACK: usize = 64 * 1024 * 1024;
/// Largest stack of a thread started for a deep program.
const MAX_STACK: usize = 2 * 1024 * 1024 * 1024;
/// Deepest nesting the type checker takes on. Deeper programs are reported
/// instead of checked; the other analyses run on the largest stack.
pub const MAX_DEPTH: usize = MAX_STACK / STACK_PER_LEVEL;

thread_local! {
    /// The nesting depth the stack of the current thread was sized for, when
    /// [`with_stack_for_depth`] started the thread; 0 on any other thread.
    static PROVISIONED_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// The deepest nesting of statements and expressions in `program`, and the
/// span of the deepest expression. Walks with an explicit stack: the point is
/// that the tree may be too deep to recurse over. The `${...}` bodies of
/// template literals are not part of the tree, but the analyses walk them as
/// if they were, so a body long enough to nest deeply is measured too.
pub fn nesting_depth(program: &Program) -> (usize, Option<Span>) {
    deepest(
        program
            .statements
            .iter()
            .map(|stmt| (Node::Stmt(stmt), 1))
            .collect(),
    )
}

/// An expression of fewer characters than this cannot nest deeper than the
/// depth up to which an analysis runs on the calling thread (each level takes
/// at least two characters), so a shorter `${...}` body is not parsed to be
/// measured.
const TEMPLATE_MEASURE_MIN: usize = 2 * INLINE_DEPTH_LIMIT;

fn deepest(mut pending: Vec<(Node<'_>, usize)>) -> (usize, Option<Span>) {
    let mut deepest = 0;
    let mut deepest_span = None;
    let mut children = Vec::new();
    while let Some((node, depth)) = pending.pop() {
        if depth > deepest {
            deepest = depth;
            deepest_span = match node {
                Node::Expr(expr) => Some(expr.span().clone()),
                Node::Stmt(_) => deepest_span,
            };
        }
        children.clear();
        match node {
            Node::Expr(Expr::TemplateLit(template, span)) => {
                for (body, line) in interpolation_sources(template, span) {
                    if body.len() < TEMPLATE_MEASURE_MIN {
                        continue;
                    }
                    let (slot, errors) = parse_from_line(&body, line);
                    if !errors.is_empty() {
                        continue;
                    }
                    // A slot's spans are relative to the slot, so the depth
                    // is reported at the template it is written in.
                    let (inner, _) = deepest_in(&slot, depth + 1);
                    if inner > deepest {
                        deepest = inner;
                        deepest_span = Some(span.clone());
                    }
                }
            }
            Node::Expr(expr) => direct_children_of_expr(expr, &mut children),
            Node::Stmt(stmt) => direct_children_of_stmt(stmt, &mut children),
        }
        pending.extend(children.drain(..).map(|child| (child, depth + 1)));
    }
    (deepest, deepest_span)
}

/// [`nesting_depth`] of `program`, its top level counted as `depth`.
fn deepest_in(program: &Program, depth: usize) -> (usize, Option<Span>) {
    deepest(
        program
            .statements
            .iter()
            .map(|stmt| (Node::Stmt(stmt), depth))
            .collect(),
    )
}

/// Runs `pass` over a program nested `depth` levels deep. A shallow program
/// is walked on the calling thread; a deep one on a thread whose stack is
/// sized for its depth, since the walkers use a few frames per level and a
/// few thousand levels use up an ordinary thread stack. A pass already
/// running on a thread sized for at least `depth` is not moved again.
pub fn with_stack_for_depth<R: Send>(depth: usize, pass: impl FnOnce() -> R + Send) -> R {
    if depth <= INLINE_DEPTH_LIMIT || depth <= PROVISIONED_DEPTH.with(Cell::get) {
        return pass();
    }
    let stack_size = depth
        .saturating_mul(STACK_PER_LEVEL)
        .clamp(MIN_STACK, MAX_STACK);
    let provisioned = stack_size / STACK_PER_LEVEL;
    let slot = Mutex::new(Some(pass));
    let outcome = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("taida-deep".to_string())
            .stack_size(stack_size)
            .spawn_scoped(scope, || {
                PROVISIONED_DEPTH.with(|d| d.set(provisioned));
                take_pass(&slot)()
            })
            .map(|handle| handle.join())
    });
    match outcome {
        Ok(Ok(result)) => result,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        // No thread could be started (address space or thread limits): walk
        // where we are, as small programs do.
        Err(_) => take_pass(&slot)(),
    }
}

fn take_pass<F>(slot: &Mutex<Option<F>>) -> F {
    slot.lock()
        .ok()
        .and_then(|mut pass| pass.take())
        .expect("a pass is run once")
}

/// [`with_stack_for_depth`] for the depth of `program`.
pub fn with_stack_for<R: Send>(program: &Program, pass: impl FnOnce() -> R + Send) -> R {
    with_stack_for_depth(nesting_depth(program).0, pass)
}

/// [`with_stack_for`] for a program the pass takes over. The program is
/// dropped on the deep stack too, as dropping a deeply nested tree recurses
/// as well.
pub fn with_stack_for_owned<R: Send>(
    program: Program,
    pass: impl FnOnce(&Program) -> R + Send,
) -> R {
    let depth = nesting_depth(&program).0;
    with_stack_for_depth(depth, move || pass(&program))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(source: &str) -> Program {
        let (program, errors) = parse(source);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        program
    }

    fn assigned_value(program: &Program) -> &Expr {
        match &program.statements[0] {
            Statement::Assignment(assign) => &assign.value,
            other => panic!("expected an assignment, got {other:?}"),
        }
    }

    #[test]
    fn template_interpolations_are_children_with_their_line() {
        let program = parse_ok("\n\nt <= `a ${f(1)} b\n${g(2)}`\n");
        let expr = assigned_value(&program);
        let mut lines = Vec::new();
        for_each_child_of_expr(expr, &mut |child| {
            if let Node::Expr(Expr::FuncCall(_, _, span)) = child {
                lines.push(span.line);
            }
        });
        assert_eq!(lines, vec![3, 4]);
    }

    #[test]
    fn unary_operand_pack_field_and_type_field_are_children() {
        let program =
            parse_ok("P = @(name: Str)\nx <= !ok(1)\ny <= @(v <= inc(1))\nz <= P(name <= f(2))\n");
        let mut counts = Vec::new();
        for stmt in &program.statements[1..] {
            let mut n = 0;
            for_each_child_of_stmt(stmt, &mut |child| {
                if let Node::Expr(expr) = child {
                    for_each_child_of_expr(expr, &mut |_| n += 1);
                }
            });
            counts.push(n);
        }
        assert_eq!(counts, vec![1, 1, 1]);
    }

    #[test]
    fn method_bodies_are_children_of_the_type_definition() {
        let program =
            parse_ok("Counter = @(\n  count: Int\n  bump =\n    inc(count)\n  => :Int\n)\n");
        let mut bodies = 0;
        for_each_child_of_stmt(&program.statements[0], &mut |child| {
            if matches!(child, Node::Stmt(_)) {
                bodies += 1;
            }
        });
        assert_eq!(bodies, 1);
    }

    /// A left-associative chain of `terms` literals, `1 + 1 + ... + 1`.
    /// Parsing numbers the nodes of the tree, which recurses too, so it is
    /// done on a large stack: a test thread's is small.
    fn chain(terms: usize) -> Program {
        let source = format!("x <= {}\n", vec!["1"; terms].join(" + "));
        std::thread::Builder::new()
            .stack_size(256 * 1024 * 1024)
            .spawn(move || parse_ok(&source))
            .expect("spawn a parsing thread")
            .join()
            .expect("parse the chain")
    }

    #[test]
    fn depth_counts_statements_and_expressions() {
        let program = parse_ok("x <= 1 + 2 + 3\n");
        // statement, then `(1 + 2) + 3`, then `1 + 2`, then `1`.
        assert_eq!(nesting_depth(&program).0, 4);
    }

    #[test]
    fn a_pass_over_a_deep_chain_runs_on_a_stack_sized_for_it() {
        // Far past what the 2 MiB stack of a test thread can recurse over,
        // even in an optimised build.
        fn height(expr: &Expr) -> usize {
            let mut below = 0;
            for_each_child_of_expr(expr, &mut |child| {
                if let Node::Expr(inner) = child {
                    below = below.max(height(inner));
                }
            });
            below + 1
        }
        let deep = with_stack_for_owned(chain(20_000), |program| height(assigned_value(program)));
        assert_eq!(deep, 20_000);
    }

    #[test]
    fn a_pass_already_on_a_deep_stack_is_not_moved_again() {
        let program = chain(1_000);
        let (outer, inner) = with_stack_for(&program, || {
            let outer = std::thread::current().id();
            let inner = with_stack_for(&program, || std::thread::current().id());
            (outer, inner)
        });
        assert_eq!(outer, inner);
        assert_ne!(outer, std::thread::current().id());
    }

    #[test]
    fn a_shallow_program_is_walked_where_it_is() {
        let program = parse_ok("x <= 1 + 2\n");
        let here = std::thread::current().id();
        assert_eq!(
            with_stack_for(&program, || std::thread::current().id()),
            here
        );
    }

    #[test]
    #[should_panic(expected = "the pass failed")]
    fn a_panic_in_a_pass_on_a_deep_stack_reaches_the_caller() {
        let program = chain(1_000);
        with_stack_for(&program, || panic!("the pass failed"));
    }

    #[test]
    fn bindings_last_until_their_scope_is_released() {
        let mut bindings = Bindings::default();
        bindings.bind("outer");
        let mark = bindings.mark();
        bindings.bind("inner");
        assert!(bindings.is_bound("outer") && bindings.is_bound("inner"));
        bindings.release(mark);
        assert!(bindings.is_bound("outer") && !bindings.is_bound("inner"));
    }

    #[test]
    fn a_long_template_slot_counts_towards_the_depth() {
        let slot = vec!["1"; 300].join(" + ");
        let program = parse_ok(&format!("t <= `a ${{{slot}}} b`\n"));
        assert!(nesting_depth(&program).0 > 300);
        // A short slot is not measured.
        let program = parse_ok("t <= `a ${1 + 2} b`\n");
        assert_eq!(nesting_depth(&program).0, 2);
    }

    #[test]
    fn a_slot_is_parsed_on_the_line_it_is_written() {
        let program = parse_ok("\n\nt <= `a\nb\n${f(1)}`\n");
        let expr = assigned_value(&program);
        let mut lines = Vec::new();
        for_each_child_of_expr(expr, &mut |child| {
            if let Node::Expr(Expr::FuncCall(_, _, span)) = child {
                lines.push(span.line);
            }
        });
        assert_eq!(lines, vec![5]);
    }
}
