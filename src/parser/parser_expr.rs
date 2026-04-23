use super::*;

impl Parser {
    pub(super) fn parse_expression(&mut self) -> Result<Expr, ParseError> {
        // RCB-301: Guard against stack overflow from deeply nested expressions.
        self.depth += 1;
        if self.depth > super::MAX_PARSE_DEPTH {
            let span = self.current_span();
            self.depth -= 1;
            return Err(ParseError {
                message: format!(
                    "Maximum nesting depth ({}) exceeded. Simplify the expression.",
                    super::MAX_PARSE_DEPTH
                ),
                span,
            });
        }
        let result = self.parse_or_expr();
        self.depth -= 1;
        result
    }

    pub(super) fn parse_or_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and_expr()?;
        while self.check(&TokenKind::Or) {
            let span = self.current_span();
            self.advance();
            let right = self.parse_and_expr()?;
            left = Expr::BinaryOp(Box::new(left), BinOp::Or, Box::new(right), span);
        }
        Ok(left)
    }

    pub(super) fn parse_and_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_equality_expr()?;
        while self.check(&TokenKind::And) {
            let span = self.current_span();
            self.advance();
            let right = self.parse_equality_expr()?;
            left = Expr::BinaryOp(Box::new(left), BinOp::And, Box::new(right), span);
        }
        Ok(left)
    }

    pub(super) fn parse_equality_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_comparison_expr()?;
        while matches!(self.peek_kind(), TokenKind::EqEq | TokenKind::BangEq) {
            let span = self.current_span();
            let op = match self.advance().kind {
                TokenKind::EqEq => BinOp::Eq,
                TokenKind::BangEq => BinOp::NotEq,
                _ => unreachable!(),
            };
            let right = self.parse_comparison_expr()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right), span);
        }
        Ok(left)
    }

    pub(super) fn parse_comparison_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_additive_expr()?;
        while matches!(
            self.peek_kind(),
            TokenKind::Lt | TokenKind::Gt | TokenKind::GtEq
        ) {
            let span = self.current_span();
            let op = match self.advance().kind {
                TokenKind::Lt => BinOp::Lt,
                TokenKind::Gt => BinOp::Gt,
                TokenKind::GtEq => BinOp::GtEq,
                _ => unreachable!(),
            };
            let right = self.parse_additive_expr()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right), span);
        }
        Ok(left)
    }

    pub(super) fn parse_additive_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_multiplicative_expr()?;
        while matches!(self.peek_kind(), TokenKind::Plus | TokenKind::Minus) {
            let span = self.current_span();
            let op = match self.advance().kind {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => unreachable!(),
            };
            let right = self.parse_multiplicative_expr()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right), span);
        }
        Ok(left)
    }

    pub(super) fn parse_multiplicative_expr(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_unary_expr()?;
        while matches!(self.peek_kind(), TokenKind::Star) {
            let span = self.current_span();
            let op = match self.advance().kind {
                TokenKind::Star => BinOp::Mul,
                _ => unreachable!(),
            };
            let right = self.parse_unary_expr()?;
            left = Expr::BinaryOp(Box::new(left), op, Box::new(right), span);
        }
        // `/` and `%` operators are removed — use Div[x, y]() and Mod[x, y]() molds instead
        if matches!(self.peek_kind(), TokenKind::Slash) {
            return Err(self.error_at_current(
                "The `/` operator has been removed. Use Div[x, y]() mold instead",
            ));
        }
        if matches!(self.peek_kind(), TokenKind::Percent) {
            return Err(self.error_at_current(
                "The `%` operator has been removed. Use Mod[x, y]() mold instead",
            ));
        }
        Ok(left)
    }

    pub(super) fn parse_unary_expr(&mut self) -> Result<Expr, ParseError> {
        match self.peek_kind() {
            TokenKind::Bang => {
                let span = self.current_span();
                self.advance();
                let expr = self.parse_unary_expr()?;
                Ok(Expr::UnaryOp(UnaryOp::Not, Box::new(expr), span))
            }
            TokenKind::Minus => {
                let span = self.current_span();
                self.advance();
                let expr = self.parse_unary_expr()?;
                Ok(Expr::UnaryOp(UnaryOp::Neg, Box::new(expr), span))
            }
            _ => self.parse_postfix_expr(),
        }
    }

    pub(super) fn parse_postfix_expr(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary_expr()?;

        loop {
            match self.peek_kind() {
                // Function call: `expr(args)`
                TokenKind::LParen => {
                    let span = self.current_span();
                    self.advance(); // consume `(`
                    let args = self.parse_arg_list()?;
                    self.expect(&TokenKind::RParen)?;

                    // Check for `.throw()` pattern
                    if let Expr::FieldAccess(inner, method, _) = &expr
                        && method == "throw"
                        && args.is_empty()
                    {
                        expr = Expr::Throw(inner.clone(), span);
                        continue;
                    }

                    // Check if it's a method call (field access + call)
                    if let Expr::FieldAccess(obj, method, _) = expr {
                        expr = Expr::MethodCall(obj, method, args, span);
                    } else {
                        expr = Expr::FuncCall(Box::new(expr), args, span);
                    }
                }
                // Field/method access: `expr.field`
                TokenKind::Dot => {
                    let span = self.current_span();
                    self.advance(); // consume `.`
                    let field = self.expect_ident()?;
                    expr = Expr::FieldAccess(Box::new(expr), field, span);
                }
                // Index access: `expr[index]` — REMOVED in v0.5.0
                // Use .get(index) instead. MoldInst `Name[args](fields)` is handled in parse_primary_expr.
                TokenKind::LBracket => {
                    let span = self.current_span();
                    return Err(ParseError {
                        message: "Index access `expr[i]` has been removed. Use `.get(i)` instead"
                            .to_string(),
                        span,
                    });
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    pub(super) fn parse_primary_expr(&mut self) -> Result<Expr, ParseError> {
        let span = self.current_span();
        match self.peek_kind().clone() {
            TokenKind::IntLiteral(n) => {
                self.advance();
                Ok(Expr::IntLit(n, span))
            }
            TokenKind::FloatLiteral(n) => {
                self.advance();
                Ok(Expr::FloatLit(n, span))
            }
            TokenKind::StringLiteral(s) => {
                let s = s.clone();
                self.advance();
                Ok(Expr::StringLit(s, span))
            }
            TokenKind::TemplateLiteral(s) => {
                let s = s.clone();
                self.advance();
                Ok(Expr::TemplateLit(s, span))
            }
            TokenKind::BoolLiteral(b) => {
                self.advance();
                Ok(Expr::BoolLit(b, span))
            }
            TokenKind::Gorilla => {
                self.advance();
                Ok(Expr::Gorilla(span))
            }
            // C-2a: `_` (Placeholder) が許可される構文位置:
            //
            // 1. **ラムダ定義** — `_ x = expr` / `_ = expr`
            //    → Lambda AST に変換。Placeholder にはならない。
            //
            // 2. **パイプライン内の関数呼び出し引数** — `data => f(_, 3)`
            //    → Expr::Placeholder(Span)。パイプの前段の値を参照。
            //    → checker: in_pipeline=true の場合のみ許可。
            //
            // 3. **パイプライン内の Mold 型引数** — `data => Trim[_]()`
            //    → Expr::Placeholder(Span)。Mold の型引数として。
            //    → checker: in_pipeline=true の場合のみ許可 (E1504)。
            //
            // 4. **条件分岐のワイルドカード** — `| _ |> expr`
            //    → parse_cond_branch で直接処理。catch-all パターン。
            //
            // 5. **型注釈の省略** — `:Result[T, _]`
            //    → parse_type_expr で処理。型推論プレースホルダ。
            //
            // 拒否される位置 (checker で reject):
            // - 関数呼び出し引数（パイプライン外）— `add(5, _)` → E1502
            // - TypeDef/BuchiPack インスタンス化 — `Point(_, 2)` → E1503
            // - Mold 型引数（パイプライン外）— `Trim[_]()` → E1504
            TokenKind::Placeholder => {
                self.advance();
                // Check if this is a lambda: `_ x = expr` or `_ = expr` (zero-param lambda)
                if self.check_ident() {
                    self.parse_lambda(span)
                } else if self.check(&TokenKind::Eq) {
                    // `_ = expr` — zero-parameter lambda (e.g., `_ = true`, `_ = false`)
                    self.parse_lambda(span)
                } else {
                    Ok(Expr::Placeholder(span))
                }
            }

            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();

                if self.check(&TokenKind::Colon)
                    && matches!(self.peek_at(1).kind, TokenKind::Ident(_))
                    && matches!(self.peek_at(2).kind, TokenKind::LParen)
                    && matches!(self.peek_at(3).kind, TokenKind::RParen)
                {
                    self.advance(); // consume `:`
                    let variant = self.expect_ident()?;
                    self.expect(&TokenKind::LParen)?;
                    self.expect(&TokenKind::RParen)?;
                    return Ok(Expr::EnumVariant(name, variant, span));
                }

                // Check for type/mold instantiation: `Name[args](...)`
                // Uses backtracking: if bracket-args parsing fails or the
                // result is not followed by `(`, we restore `self.pos = save`
                // and treat the identifier as a plain Ident.
                if self.check(&TokenKind::LBracket) {
                    let save = self.pos;
                    self.advance(); // consume `[`

                    // Try to parse as mold instantiation
                    let mut type_args = Vec::new();
                    let mut depth = 1;
                    let mut arg_tokens_valid = true;

                    // Parse comma-separated expressions inside brackets.
                    // B11-6a: Also handles restricted type-literal surface:
                    //   `:Int` → TypeLiteral("Int", None)
                    //   `EnumName:Variant` (without `()`) → TypeLiteral("EnumName", Some("Variant"))
                    // These are only valid inside mold brackets and do not leak
                    // into general expression parsing (B11B-008).
                    let is_type_mold = name == "TypeIs" || name == "TypeExtends";
                    loop {
                        if self.check(&TokenKind::RBracket) {
                            depth -= 1;
                            if depth == 0 {
                                self.advance();
                                break;
                            }
                        }
                        if self.is_at_end() {
                            arg_tokens_valid = false;
                            break;
                        }
                        // B11-6a: `:TypeName` → TypeLiteral (restricted to TypeIs/TypeExtends)
                        if is_type_mold
                            && self.check(&TokenKind::Colon)
                            && matches!(self.peek_at(1).kind, TokenKind::Ident(_))
                        {
                            let lit_span = self.current_span();
                            self.advance(); // consume `:`
                            let type_name = self.expect_ident()?;
                            type_args.push(Expr::TypeLiteral(type_name, None, lit_span));
                            self.match_token(&TokenKind::Comma);
                            continue;
                        }
                        // B11-6a: `EnumName:Variant` without `()` → TypeLiteral
                        // (restricted to TypeIs/TypeExtends)
                        // Distinguishes from EnumVariant `Name:Variant()` by absence of `()`.
                        if is_type_mold
                            && matches!(self.peek_kind(), TokenKind::Ident(_))
                            && matches!(self.peek_at(1).kind, TokenKind::Colon)
                            && matches!(self.peek_at(2).kind, TokenKind::Ident(_))
                            && !matches!(self.peek_at(3).kind, TokenKind::LParen)
                        {
                            let lit_span = self.current_span();
                            let enum_name = self.expect_ident()?;
                            self.advance(); // consume `:`
                            let variant_name = self.expect_ident()?;
                            type_args.push(Expr::TypeLiteral(
                                enum_name,
                                Some(variant_name),
                                lit_span,
                            ));
                            self.match_token(&TokenKind::Comma);
                            continue;
                        }
                        match self.parse_expression() {
                            Ok(arg) => type_args.push(arg),
                            Err(_) => {
                                arg_tokens_valid = false;
                                break;
                            }
                        }
                        self.match_token(&TokenKind::Comma);
                    }

                    if arg_tokens_valid && self.check(&TokenKind::LParen) {
                        // `Name[args](fields)` -> MoldInst or FuncCall
                        self.advance(); // consume `(`
                        let fields = self.parse_buchi_field_list()?;
                        self.expect(&TokenKind::RParen)?;
                        return Ok(Expr::MoldInst(name, type_args, fields, span));
                    } else if arg_tokens_valid {
                        // Index access `name[expr]` has been removed in v0.5.0.
                        // Use `.get(index)` instead.
                        // `Name[args]` without `()` is treated as MoldInst with no fields.
                        if type_args.len() == 1 {
                            // Could be `list[0]` (removed) or `Optional[T]` (MoldInst)
                            // If the name starts with uppercase, treat as MoldInst
                            if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                                return Ok(Expr::MoldInst(name, type_args, Vec::new(), span));
                            }
                            // Otherwise it's an index access attempt — error
                            return Err(ParseError {
                                message:
                                    "Index access `name[i]` has been removed. Use `.get(i)` instead"
                                        .to_string(),
                                span,
                            });
                        }
                        return Ok(Expr::MoldInst(name, type_args, Vec::new(), span));
                    }

                    // Backtrack if parsing failed
                    self.pos = save;
                    return Ok(Expr::Ident(name, span));
                }

                // Check for type instantiation: `Name(field <= value, ...)`
                if name.chars().next().is_some_and(|c| c.is_uppercase())
                    && self.check(&TokenKind::LParen)
                {
                    let save = self.pos;
                    self.advance(); // consume `(`

                    // C26B-019: allow newlines after `(` before the first field
                    // so multi-line TypeDef constructors like
                    //   Ctx(
                    //     a <= x,
                    //     b <= x,
                    //   )
                    // are recognised as type instantiations (matching the
                    // parity with `@(...)` literals). The lookahead below must
                    // see the first ident/<= pair, so we skip continuation
                    // whitespace here. `parse_buchi_field_list` already
                    // re-skips newlines internally.
                    self.skip_newlines();

                    // Check if args look like buchi fields (name <= value)
                    if self.check_ident() {
                        let peek_ahead = self.peek_at(1);
                        if matches!(peek_ahead.kind, TokenKind::LtEq) {
                            let fields = self.parse_buchi_field_list()?;
                            self.expect(&TokenKind::RParen)?;
                            return Ok(Expr::TypeInst(name, fields, span));
                        }
                    }

                    // Not a type instantiation, backtrack
                    self.pos = save;
                }

                Ok(Expr::Ident(name, span))
            }

            // Buchi pack literal: `@(...)`
            TokenKind::At => {
                self.advance(); // consume `@`
                if self.check(&TokenKind::LParen) {
                    self.advance(); // consume `(`
                    let fields = self.parse_buchi_field_list()?;
                    self.expect(&TokenKind::RParen)?;
                    Ok(Expr::BuchiPack(fields, span))
                } else if self.check(&TokenKind::LBracket) {
                    self.advance(); // consume `[`
                    let mut items = Vec::new();
                    while !self.check(&TokenKind::RBracket) && !self.is_at_end() {
                        self.skip_newlines();
                        if self.check(&TokenKind::RBracket) {
                            break;
                        }
                        items.push(self.parse_expression()?);
                        self.match_token(&TokenKind::Comma);
                        self.skip_newlines();
                    }
                    self.expect(&TokenKind::RBracket)?;
                    Ok(Expr::ListLit(items, span))
                } else {
                    Err(self.error_at_current("Expected '(' or '[' after '@'"))
                }
            }

            // Parenthesized expression
            TokenKind::LParen => {
                self.advance(); // consume `(`
                // C20-1 (ROOT-5): parentheses restore `TopLevel` context so
                // that a parenthesised multi-line guard (`name <= (| ... |> ...)`)
                // is legal — the parens make the boundary unambiguous, so
                // the `[E0303]` restriction that applies to bare `<=` rhs
                // does not apply inside `(...)`.
                //
                // Allow an optional newline+indent immediately after `(`
                // so that the parenthesised escape hatch can be written
                // on multiple lines:
                //     name <= (
                //       | cond |> a
                //       | _    |> b
                //     )
                self.skip_newlines();
                let saved_ctx =
                    std::mem::replace(&mut self.cond_branch_context, CondBranchContext::TopLevel);
                let expr_result = self.parse_expression();
                self.cond_branch_context = saved_ctx;
                let expr = expr_result?;
                // Tolerate a trailing newline before `)` as well.
                self.skip_newlines();
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }

            // Condition branch: `| cond |> value`
            TokenKind::Pipe => self.parse_cond_branch(),

            // Return type annotation in expression context: `=> :Type`
            TokenKind::FatArrow => {
                self.advance();
                if self.match_token(&TokenKind::Colon) {
                    let _type_expr = self.parse_type_expr()?;
                }
                // Return a placeholder for now
                Ok(Expr::Placeholder(span))
            }

            _ => Err(self.error_at_current(&format!("Unexpected token: {:?}", self.peek_kind()))),
        }
    }

    pub(super) fn parse_lambda(&mut self, start_span: Span) -> Result<Expr, ParseError> {
        // `_ x y = expr` -> Lambda([x, y], expr)
        let mut params = Vec::new();
        while self.check_ident() {
            let param_span = self.current_span();
            let param_name = self.expect_ident()?;
            params.push(Param {
                name: param_name,
                type_annotation: None,
                default_value: None,
                span: param_span,
            });
        }
        self.expect(&TokenKind::Eq)?;
        let body = self.parse_expression()?;
        Ok(Expr::Lambda(params, Box::new(body), start_span))
    }

    pub(super) fn parse_cond_branch(&mut self) -> Result<Expr, ParseError> {
        let span = self.current_span();
        let mut arms = Vec::new();

        // Record the column of the first `|` to detect nested CondBranch boundaries.
        // Only `|` tokens at this same column belong to this CondBranch;
        // deeper-indented `|` tokens belong to a nested CondBranch.
        let branch_column = self.peek().span.column;
        let branch_line = self.peek().span.line;

        while self.check(&TokenKind::Pipe) {
            let arm_span = self.current_span();
            self.advance(); // consume `|`

            // Check for default case: `| _ |>`
            if self.check(&TokenKind::Placeholder) && self.peek_at(1).kind == TokenKind::PipeGt {
                self.advance(); // consume `_`
                self.advance(); // consume `|>`
                let body = self.parse_cond_arm_body()?;
                arms.push(CondArm {
                    condition: None,
                    body,
                    span: arm_span,
                });
            } else {
                let condition = self.parse_expression()?;
                self.expect(&TokenKind::PipeGt)?;
                let body = self.parse_cond_arm_body()?;
                arms.push(CondArm {
                    condition: Some(condition),
                    body,
                    span: arm_span,
                });
            }

            // Speculatively skip newlines/indents to check for a continuation `|` arm.
            // If the next non-whitespace token is NOT `|`, restore position so that
            // the caller's block parser can see the indent tokens and determine scope correctly.
            // This prevents eating indent tokens that belong to the enclosing block
            // (e.g., when a CondBranch appears inside a lambda body within a function).
            // Additionally, check that the `|` is at the same column as the first `|`
            // to prevent a nested CondBranch from consuming outer arms.
            let save_pos = self.pos;
            self.skip_newlines();
            if !self.check(&TokenKind::Pipe) || self.peek().span.column != branch_column {
                self.pos = save_pos;
            } else {
                // C20-1 (ROOT-5 / C19B-009): a continuation `|` arm appears
                // on a later line than the first arm. In `<=` rhs context
                // this is the exact shape that silently swallows the
                // enclosing block's subsequent statements. Reject with
                // `[E0303]` — see the companion guard in
                // `parse_cond_arm_body` for the escape hatches (parens,
                // helper function, `If[]()`). Single-line multi-arm
                // guards (`name <= | a |> 1 | _ |> 2`) stay legal because
                // `self.peek().span.line == branch_line` in that case.
                if self.cond_branch_context == CondBranchContext::LetRhs
                    && self.peek().span.line != branch_line
                {
                    return Err(self.error_at_current(
                        "[E0303] 単一方向制約違反 — `<=` の右辺に複数行の `| cond |> body` \
                         多アーム条件を書くことはできません。代替: \
                         `name <= If[cond, then, else]()`、または `pickX = | ... |> ... | _ |> ...` \
                         のようなヘルパ関数抽出、または `name <= (| ... |> ... | _ |> ...)` のように \
                         丸括弧で包んでください (see docs/reference/diagnostic_codes.md#E0303).",
                    ));
                }
            }
        }

        Ok(Expr::CondBranch(arms, span))
    }

    /// Parse the body of a condition arm after `|>`.
    /// If the body is on the same line, parse a single expression.
    /// If the body continues on the next line (indented), parse as a block.
    ///
    /// C12-4 (FB-17): Pure expression discipline.
    /// An arm body must be a sequence of **let-bindings** followed by
    /// **exactly one final result expression**. Allowed non-final
    /// statement kinds: `<=` assignment, `]=>` unmold-forward,
    /// `<=[` unmold-backward. Any other statement kind in a non-final
    /// position — including a bare function-call / pipeline statement
    /// (used for side effects) and any definition (`Name = ...`,
    /// `Mold[] => ...`, `|== ... =`, `>>> ...`, `<<< ...`) — is
    /// rejected with `[E1616]`. The final statement must be an
    /// expression statement that produces the arm's value.
    pub(super) fn parse_cond_arm_body(&mut self) -> Result<Vec<Statement>, ParseError> {
        // Check if body is a multi-line block (newline follows |>)
        //
        // NB: The C20-1 (ROOT-5) silent-bug guard lives in
        // `parse_cond_branch` — it fires when a continuation `|` arm
        // is seen on a later line while in `LetRhs` context. A single
        // arm with a multi-line body inside a single-arm CondBranch is
        // still legal here (the subsequent block boundary is
        // unambiguous when there is no sibling `|` to steal indent
        // tokens from).
        if matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Indent(_)) {
            // Multi-line body: parse as block
            let block = self.parse_block()?;
            if block.is_empty() {
                return Err(self.error_at_current("Expected expression in condition arm body"));
            }
            Self::validate_cond_arm_body(&block)?;
            Ok(block)
        } else {
            // Single-line body: parse expression and wrap
            let expr = self.parse_expression()?;
            Ok(vec![Statement::Expr(expr)])
        }
    }

    /// C12-4 (FB-17) + C13-1: pure-expression discipline on a parsed
    /// condition-arm body, relaxed for C13.
    ///
    /// Rules:
    ///   1. The **final** statement may be:
    ///      - `Statement::Expr(_)` — arm's result expression (classic form), or
    ///      - `Statement::Assignment(_)` / `Statement::UnmoldForward(_)` /
    ///        `Statement::UnmoldBackward(_)` — C13-1 tail-binding form
    ///        that yields the bound value as the arm's result.
    ///   2. **Non-final** statements must be `Assignment`,
    ///      `UnmoldForward`, or `UnmoldBackward` — i.e. let-bindings
    ///      that name a value for subsequent statements.
    ///   3. Any other statement kind anywhere in the arm body is
    ///      rejected with `[E1616]`.
    ///
    /// The rule stops the FB-17 "context leak" pattern where a
    /// discarded side-effect call (`writeFile(...) => _wr`, a bare
    /// function-call statement, or a top-level definition) can hide
    /// inside what reads like a conditional branch.
    ///
    /// C13-1 loosens rule 1 so the tail bind (`name <= expr`,
    /// `expr => name`, `expr ]=> name`, `name <=[ expr`) is accepted
    /// as an expression-block result without requiring a redundant
    /// trailing `name` line. FB-17's safety boundary is preserved:
    /// bare call statements, discard pipelines, and nested definitions
    /// in non-final positions remain rejected.
    fn validate_cond_arm_body(block: &[Statement]) -> Result<(), ParseError> {
        debug_assert!(!block.is_empty(), "empty arm body should be caught earlier");
        Self::reject_discard_bindings_in_expression_block(block, "`| |>` arm body")?;
        let last_idx = block.len() - 1;
        for (idx, stmt) in block.iter().enumerate() {
            if idx == last_idx {
                // C13-1: final statement may be an expression OR a binding.
                match stmt {
                    Statement::Expr(_)
                    | Statement::Assignment(_)
                    | Statement::UnmoldForward(_)
                    | Statement::UnmoldBackward(_) => {}
                    _ => {
                        let span = Self::statement_span(stmt);
                        return Err(ParseError {
                            message: format!(
                                "[E1616] `| |>` arm body must end with a result expression or a binding, not a {} statement. \
                                 A condition arm is a pure expression: optional let-bindings \
                                 (`name <= expr`, `expr ]=> name`, `name <=[ expr`) may appear, \
                                 and the last line may be either a result expression or a tail binding. \
                                 See docs/guide/07_control_flow.md for the pure-expression rule.",
                                Self::statement_kind_label(stmt),
                            ),
                            span,
                        });
                    }
                }
            } else {
                // Non-final statement must be a let-binding.
                match stmt {
                    Statement::Assignment(_)
                    | Statement::UnmoldForward(_)
                    | Statement::UnmoldBackward(_) => {}
                    Statement::Expr(_) => {
                        let span = Self::statement_span(stmt);
                        return Err(ParseError {
                            message: "[E1616] side-effect statement is not allowed inside a `| |>` arm body. \
                                      Only let-bindings (`name <= expr`, `expr ]=> name`, `name <=[ expr`) may \
                                      appear before the final result expression — a bare function call or \
                                      pipeline used for side effects breaks the pure-expression rule. \
                                      See docs/guide/07_control_flow.md.".to_string(),
                            span,
                        });
                    }
                    _ => {
                        let span = Self::statement_span(stmt);
                        return Err(ParseError {
                            message: format!(
                                "[E1616] `{}` is not allowed inside a `| |>` arm body. \
                                 A condition arm is a pure expression; definitions and module-level \
                                 constructs must live at the top level of a function or module. \
                                 See docs/guide/07_control_flow.md.",
                                Self::statement_kind_label(stmt),
                            ),
                            span,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// C13B-010: Reject discard bindings (`expr => _name`, `_name <= expr`,
    /// `expr ]=> _name`, `_name <=[ expr`) at any position inside an
    /// expression-block body — arm body, function body, `|==` handler body,
    /// or method body. The FB-17 "throw the value away for its side effects"
    /// pattern breaks the pure-expression rule at every C13-1 tail-binding
    /// position, so the rejection is context-independent.
    pub(crate) fn reject_discard_bindings_in_expression_block(
        block: &[Statement],
        context: &str,
    ) -> Result<(), ParseError> {
        for stmt in block {
            if let Some(discard_target) = Self::discard_binding_target(stmt) {
                let span = Self::statement_span(stmt);
                return Err(ParseError {
                    message: format!(
                        "[E1616] `{} <= ...` / `... => {}` / `... ]=> {}` is a discard binding \
                         and is not allowed inside a {}. The underscore-prefixed \
                         target indicates a value being thrown away for side effects, which \
                         breaks the pure-expression rule. Remove the binding or use a meaningful \
                         name. See docs/guide/07_control_flow.md.",
                        discard_target, discard_target, discard_target, context
                    ),
                    span,
                });
            }
        }
        Ok(())
    }

    /// C13-1: Return `Some(target)` if `stmt` is a discard-style binding
    /// (`expr => _name`, `_name <= expr`, `expr ]=> _name`, `_name <=[ expr`)
    /// — i.e. the binding target starts with `_`. Used by
    /// `reject_discard_bindings_in_expression_block`.
    fn discard_binding_target(stmt: &Statement) -> Option<&str> {
        let target = match stmt {
            Statement::Assignment(a) => a.target.as_str(),
            Statement::UnmoldForward(u) => u.target.as_str(),
            Statement::UnmoldBackward(u) => u.target.as_str(),
            _ => return None,
        };
        if target.starts_with('_') {
            Some(target)
        } else {
            None
        }
    }

    /// Human-readable label for a Statement kind, used in E1616 diagnostics.
    fn statement_kind_label(stmt: &Statement) -> &'static str {
        match stmt {
            Statement::Expr(_) => "expression",
            Statement::Assignment(_) => "assignment",
            Statement::UnmoldForward(_) => "]=> binding",
            Statement::UnmoldBackward(_) => "<=[ binding",
            Statement::EnumDef(_) => "enum definition",
            Statement::TypeDef(_) => "type definition",
            Statement::FuncDef(_) => "function definition",
            Statement::MoldDef(_) => "mold definition",
            Statement::InheritanceDef(_) => "inheritance definition",
            Statement::ErrorCeiling(_) => "error ceiling",
            Statement::Import(_) => "import",
            Statement::Export(_) => "export",
        }
    }

    /// Span of a statement (covers all arms of `Statement`), for diagnostics.
    fn statement_span(stmt: &Statement) -> Span {
        match stmt {
            Statement::Expr(e) => e.span().clone(),
            Statement::Assignment(a) => a.span.clone(),
            Statement::UnmoldForward(u) => u.span.clone(),
            Statement::UnmoldBackward(u) => u.span.clone(),
            Statement::EnumDef(d) => d.span.clone(),
            Statement::TypeDef(d) => d.span.clone(),
            Statement::FuncDef(d) => d.span.clone(),
            Statement::MoldDef(d) => d.span.clone(),
            Statement::InheritanceDef(d) => d.span.clone(),
            Statement::ErrorCeiling(d) => d.span.clone(),
            Statement::Import(d) => d.span.clone(),
            Statement::Export(d) => d.span.clone(),
        }
    }

    pub(super) fn parse_arg_list(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        // C26B-019: allow newlines after `(` so multi-line constructor and
        // function calls parse the same as `@(...)` literals and bracket
        // argument lists. Newlines inside `(` ... `)` are never statement
        // terminators in Taida — the parenthesis pair is a continuation group.
        self.skip_newlines();
        // Empty arg list: `f()`
        if self.check(&TokenKind::RParen) {
            return Ok(args);
        }
        // Parse first slot
        loop {
            if self.check(&TokenKind::Comma) || self.check(&TokenKind::RParen) {
                // Empty slot (hole): no expression before comma or rparen
                let span = self.current_span();
                args.push(Expr::Hole(span));
            } else {
                args.push(self.parse_expression()?);
            }
            // C26B-019: permit an optional line break between the slot and
            // the separator / closing paren so `Ctx(\n  a <= x,\n  b <= x\n)`
            // parses identically to the single-line form.
            self.skip_newlines();
            // After each slot, expect comma or rparen
            if self.check(&TokenKind::Comma) {
                self.advance(); // consume comma
                // C26B-019: newlines after a comma are also continuation
                // whitespace (trailing comma + closing paren on the next line).
                self.skip_newlines();
                // If RParen follows, there's one more trailing hole
                if self.check(&TokenKind::RParen) {
                    let span = self.current_span();
                    args.push(Expr::Hole(span));
                    break;
                }
                // Otherwise continue to parse next slot
            } else {
                // No comma → must be RParen (end of arg list)
                break;
            }
        }
        Ok(args)
    }

    pub(super) fn parse_buchi_field_list(&mut self) -> Result<Vec<BuchiField>, ParseError> {
        let mut fields = Vec::new();
        self.skip_newlines();
        while !self.check(&TokenKind::RParen) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(&TokenKind::RParen) {
                break;
            }
            let field_span = self.current_span();

            if self.check_ident() && self.peek_at(1).kind == TokenKind::LtEq {
                let name = self.expect_ident()?;
                self.expect(&TokenKind::LtEq)?;
                let value = self.parse_expression()?;
                fields.push(BuchiField {
                    name,
                    value,
                    span: field_span,
                });
            } else {
                // Positional argument — use index as name
                let value = self.parse_expression()?;
                fields.push(BuchiField {
                    name: format!("_{}", fields.len()),
                    value,
                    span: field_span,
                });
            }
            self.match_token(&TokenKind::Comma);
            self.skip_newlines();
        }
        Ok(fields)
    }
}
