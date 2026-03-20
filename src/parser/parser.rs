use super::ast::*;
use crate::lexer::{Span, Token, TokenKind};

/// Parser error.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Parse error at line {}, column {}: {}",
            self.span.line, self.span.column, self.message
        )
    }
}

impl std::error::Error for ParseError {}

/// Recursive descent parser for Taida Lang.
/// Maximum nesting depth for recursive expression parsing.
/// Prevents stack overflow from deeply nested inputs (RCB-301 / SEC-002).
const MAX_PARSE_DEPTH: usize = 256;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    errors: Vec<ParseError>,
    /// Current recursion depth for expression parsing (RCB-301).
    depth: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        // Filter out non-doc comments, but keep newlines, indents, and doc comments
        let tokens: Vec<Token> = tokens
            .into_iter()
            .filter(|t| {
                !matches!(
                    t.kind,
                    TokenKind::LineComment(_) | TokenKind::BlockComment(_)
                )
            })
            .collect();

        // ── Line continuation ──────────────────────────────────
        //
        // A backslash at the end of a line (`\` + Newline) joins the next
        // physical line into the current logical line.  This pass removes the
        // backslash, the newline, and any leading indent tokens on the
        // continuation line.
        //
        // The algorithm is a single forward scan with an auxiliary index `j`
        // that peeks ahead when a Backslash is encountered:
        //   1. If Backslash is followed by Newline (+ optional Indent*),
        //      skip all of them and resume from `j`.
        //   2. Otherwise keep the Backslash (defensive; lexer should not
        //      produce bare backslashes outside of strings).
        let mut filtered = Vec::with_capacity(tokens.len());
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i].kind == TokenKind::Backslash {
                let mut j = i + 1;
                if j < tokens.len() && matches!(tokens[j].kind, TokenKind::Newline) {
                    j += 1;
                    while j < tokens.len() && matches!(tokens[j].kind, TokenKind::Indent(_)) {
                        j += 1;
                    }
                    i = j;
                    continue;
                }
                // Backslash not followed by newline — keep it
                filtered.push(tokens[i].clone());
                i += 1;
            } else {
                filtered.push(tokens[i].clone());
                i += 1;
            }
        }

        Self {
            tokens: filtered,
            pos: 0,
            errors: Vec::new(),
            depth: 0,
        }
    }

    /// Parse the entire token stream into a Program.
    pub fn parse(mut self) -> (Program, Vec<ParseError>) {
        let mut statements = Vec::new();
        self.skip_newlines();
        while !self.is_at_end() {
            match self.parse_statement() {
                Ok(stmt) => statements.push(stmt),
                Err(e) => {
                    self.errors.push(e);
                    self.synchronize();
                }
            }
            self.skip_newlines();
        }
        (Program { statements }, self.errors)
    }

    // ── Helpers ──────────────────────────────────────────────

    fn is_at_end(&self) -> bool {
        self.pos >= self.tokens.len() || self.peek().kind == TokenKind::Eof
    }

    fn peek(&self) -> &Token {
        if self.pos < self.tokens.len() {
            &self.tokens[self.pos]
        } else {
            // Token stream always ends with Eof; empty stream is a lexer bug.
            self.tokens
                .last()
                .expect("token stream must contain at least an Eof token")
        }
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    /// Look ahead by `offset` tokens.  There is no hard cap on `offset` because
    /// the token stream is finite (bounded by source length) and all callers use
    /// small constant offsets (0, 1, or 2).  `saturating_add` prevents overflow.
    fn peek_at(&self, offset: usize) -> &Token {
        let idx = self.pos.saturating_add(offset);
        if idx < self.tokens.len() {
            &self.tokens[idx]
        } else {
            // Token stream always ends with Eof; empty stream is a lexer bug.
            self.tokens
                .last()
                .expect("token stream must contain at least an Eof token")
        }
    }

    fn advance(&mut self) -> &Token {
        // Safety: callers gate on `!is_at_end()` / `peek()` before advancing.
        // The `min` clamp prevents OOB if invariants are violated.
        // R-05: debug_assert catches callers that advance past the end during
        // development; the clamp still prevents UB in release builds.
        debug_assert!(
            self.pos < self.tokens.len(),
            "advance() called at pos {} but token stream length is {} — caller should check is_at_end()",
            self.pos,
            self.tokens.len()
        );
        let pos = self.pos.min(self.tokens.len().saturating_sub(1));
        let token = &self.tokens[pos];
        if self.pos < self.tokens.len().saturating_sub(1) {
            self.pos += 1;
        }
        token
    }

    fn expect(&mut self, kind: &TokenKind) -> Result<Token, ParseError> {
        if self.check(kind) {
            Ok(self.advance().clone())
        } else {
            Err(self.error_at_current(&format!(
                "Expected {:?}, found {:?}",
                kind,
                self.peek_kind()
            )))
        }
    }

    fn check(&self, kind: &TokenKind) -> bool {
        std::mem::discriminant(self.peek_kind()) == std::mem::discriminant(kind)
    }

    fn check_ident(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Ident(_))
    }

    fn match_token(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn current_span(&self) -> Span {
        self.peek().span.clone()
    }

    fn error_at_current(&self, message: &str) -> ParseError {
        ParseError {
            message: message.to_string(),
            span: self.current_span(),
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Indent(_)) {
            self.advance();
        }
    }

    /// Collect consecutive doc comment tokens (skipping newlines/indents between them).
    /// Returns the collected doc comment lines as `Vec<String>`.
    /// Call this before parsing a statement to attach doc comments to the following definition.
    fn collect_doc_comments(&mut self) -> Vec<String> {
        let mut comments = Vec::new();
        loop {
            match self.peek_kind().clone() {
                TokenKind::DocComment(content) => {
                    comments.push(content);
                    self.advance();
                }
                TokenKind::Newline | TokenKind::Indent(_) => {
                    // Peek ahead to see if there's another doc comment after whitespace
                    let mut look = self.pos + 1;
                    while look < self.tokens.len()
                        && matches!(
                            self.tokens[look].kind,
                            TokenKind::Newline | TokenKind::Indent(_)
                        )
                    {
                        look += 1;
                    }
                    if look < self.tokens.len()
                        && matches!(self.tokens[look].kind, TokenKind::DocComment(_))
                    {
                        // Skip whitespace to reach next doc comment
                        self.advance();
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        comments
    }

    #[allow(dead_code)]
    fn skip_to_newline(&mut self) {
        while !self.is_at_end() && !matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }
    }

    /// Error recovery: skip tokens until the next newline (statement boundary).
    ///
    /// Taida is indentation-based, so a newline always terminates the current
    /// (logical) line.  This is intentionally simpler than brace-delimited
    /// languages that must skip to `;` or `}` — here, newline sync is sufficient
    /// because each logical line is a complete statement.
    fn synchronize(&mut self) {
        while !self.is_at_end() {
            if matches!(self.peek_kind(), TokenKind::Newline) {
                self.advance();
                self.skip_newlines();
                return;
            }
            self.advance();
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                let name = name.clone();
                self.advance();
                Ok(name)
            }
            _ => Err(self.error_at_current(&format!(
                "Expected identifier, found {:?}",
                self.peek_kind()
            ))),
        }
    }

    // ── Statement Parsing ────────────────────────────────────

    fn parse_statement(&mut self) -> Result<Statement, ParseError> {
        self.skip_newlines();

        // Collect doc comments before the statement
        let doc_comments = self.collect_doc_comments();
        // Skip any remaining newlines/indents after doc comments
        self.skip_newlines();

        match self.peek_kind().clone() {
            // Import: `>>> ...`
            TokenKind::Import => self.parse_import(),
            // Export: `<<< ...`
            TokenKind::Export => self.parse_export(),
            // Error ceiling: `|== ...`
            TokenKind::ErrorCeiling => self.parse_error_ceiling(),
            // Pipe: condition branch at statement level `| ... |> ...`
            TokenKind::Pipe => {
                let expr = self.parse_cond_branch()?;
                Ok(Statement::Expr(expr))
            }
            // Identifier-starting statements (most common)
            TokenKind::Ident(_) => self.parse_ident_statement_with_docs(doc_comments),
            // FatArrow for return type annotation `=> :Type`
            TokenKind::FatArrow => {
                let expr = self.parse_expression()?;
                Ok(Statement::Expr(expr))
            }
            // Anything else: try as expression, then check for pipeline `=>`
            _ => {
                let start_span = self.current_span();
                let expr = self.parse_expression()?;
                self.finish_expr_as_statement(expr, start_span)
            }
        }
    }

    fn parse_ident_statement_with_docs(
        &mut self,
        doc_comments: Vec<String>,
    ) -> Result<Statement, ParseError> {
        let start_span = self.current_span();
        let save_pos = self.pos;

        // Try to peek ahead to determine what kind of statement this is
        let name = self.expect_ident()?;

        // Check what follows the identifier
        match self.peek_kind().clone() {
            // `Name = ...` -> could be type def, func def (no args), or simple assignment
            TokenKind::Eq => {
                // Check if this is a type definition: `Name = @(...)`
                self.advance(); // consume `=`

                if self.check(&TokenKind::At) {
                    // Type definition
                    let fields = self.parse_buchi_pack_fields()?;
                    Ok(Statement::TypeDef(TypeDef {
                        name,
                        fields,
                        doc_comments,
                        span: start_span,
                    }))
                } else if matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Indent(_)) {
                    // No-argument function definition: `name = \n  body\n=> :Type`
                    // Only skip Newline tokens, NOT Indent tokens — parse_block needs them
                    while matches!(self.peek_kind(), TokenKind::Newline) {
                        self.advance();
                    }
                    let body = self.parse_block()?;
                    self.skip_newlines();
                    let return_type = if self.check(&TokenKind::FatArrow) {
                        self.advance();
                        if self.match_token(&TokenKind::Colon) {
                            Some(self.parse_type_expr()?)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    Ok(Statement::FuncDef(FuncDef {
                        name,
                        type_params: Vec::new(),
                        params: Vec::new(),
                        body,
                        return_type,
                        doc_comments: doc_comments.clone(),
                        span: start_span,
                    }))
                } else {
                    // Could be simple value assignment `name = expr` — but in Taida,
                    // `=` is for definitions. We'll treat it as an expression.
                    let value = self.parse_expression()?;
                    Ok(Statement::Expr(Expr::BinaryOp(
                        Box::new(Expr::Ident(name, start_span.clone())),
                        BinOp::Eq,
                        Box::new(value),
                        start_span,
                    )))
                }
            }

            // `name <= expr` -> assignment
            TokenKind::LtEq => {
                self.advance(); // consume `<=`
                self.skip_newlines(); // allow multiline (e.g., condition branch on next line)
                let value = self.parse_expression()?;
                // Single-direction constraint: <= used, => must not follow on same line
                if self.check(&TokenKind::FatArrow) {
                    return Err(ParseError {
                        message: "E0301: 単一方向制約違反 — 一つの文内で => と <= を混在させることはできません".to_string(),
                        span: self.current_span(),
                    });
                }
                Ok(Statement::Assignment(Assignment {
                    target: name,
                    type_annotation: None,
                    value,
                    span: start_span,
                }))
            }

            // `name: Type <= expr` -> typed assignment
            TokenKind::Colon => {
                self.advance(); // consume `:`
                let type_ann = self.parse_type_expr()?;
                self.expect(&TokenKind::LtEq)?;
                let value = self.parse_expression()?;
                Ok(Statement::Assignment(Assignment {
                    target: name,
                    type_annotation: Some(type_ann),
                    value,
                    span: start_span,
                }))
            }

            // `name params = body => :Type` -> function definition
            // Detected by: Ident followed by another Ident (parameter) or Ident followed by `=`
            TokenKind::Ident(_) => {
                // This could be a function definition with parameters
                // Or an expression like `name otherName`
                // We try to parse as function definition
                self.pos = save_pos;
                self.parse_func_def_with_docs(doc_comments)
            }

            // `Mold[T] => Name[...] = @(...)` -> Mold definition
            // `Parent[T] => Child[...] = @(...)` -> generic inheritance
            TokenKind::LBracket if name == "Mold" => {
                self.pos = save_pos;
                self.parse_mold_def_with_docs(doc_comments)
            }
            TokenKind::LBracket => {
                if self.looks_like_generic_func_def() {
                    self.pos = save_pos;
                    return self.parse_func_def_with_docs(doc_comments);
                }
                self.pos = save_pos;
                match self.try_parse_inheritance_with_headers(doc_comments.clone()) {
                    Ok(stmt) => Ok(stmt),
                    Err(_) => {
                        self.pos = save_pos;
                        let expr = self.parse_expression()?;
                        self.finish_expr_as_statement(expr, start_span)
                    }
                }
            }

            // `name => ...` -> could be pipeline or inheritance
            TokenKind::FatArrow => {
                // Check if this is `Name => ChildName = @(...)` (inheritance)
                // or `name => func(_) => ...` (pipeline)
                let _save = self.pos;
                self.advance(); // consume `=>`
                if self.check_ident() {
                    let next_name = self.expect_ident()?;
                    if self.check(&TokenKind::Eq) {
                        // Inheritance: `Parent => Child = @(...)`
                        self.advance(); // consume `=`
                        let fields = self.parse_buchi_pack_fields()?;
                        return Ok(Statement::InheritanceDef(InheritanceDef {
                            parent: name,
                            parent_args: None,
                            child: next_name,
                            child_args: None,
                            fields,
                            doc_comments,
                            span: start_span,
                        }));
                    }
                }
                // Not inheritance, backtrack and parse as expression + pipeline
                self.pos = save_pos;
                let expr = self.parse_expression()?;
                self.finish_expr_as_statement(expr, start_span)
            }

            // `expr ]=> name` -> unmold forward
            TokenKind::UnmoldForward => {
                self.advance(); // consume `]=>`
                let target = self.expect_ident()?;
                // Single-direction constraint: ]=> used, <=[ must not follow
                if self.check(&TokenKind::UnmoldBackward) {
                    return Err(ParseError {
                        message: "E0302: 単一方向制約違反 — 一つの文内で ]=> と <=[ を混在させることはできません".to_string(),
                        span: self.current_span(),
                    });
                }
                Ok(Statement::UnmoldForward(UnmoldForwardStmt {
                    source: Expr::Ident(name, start_span.clone()),
                    target,
                    span: start_span,
                }))
            }

            // `name <=[ expr` -> unmold backward
            TokenKind::UnmoldBackward => {
                self.advance(); // consume `<=[`
                let source = self.parse_expression()?;
                // Single-direction constraint: <=[ used, ]=> must not follow
                if self.check(&TokenKind::UnmoldForward) {
                    return Err(ParseError {
                        message: "E0302: 単一方向制約違反 — 一つの文内で ]=> と <=[ を混在させることはできません".to_string(),
                        span: self.current_span(),
                    });
                }
                Ok(Statement::UnmoldBackward(UnmoldBackwardStmt {
                    target: name,
                    source,
                    span: start_span,
                }))
            }

            // Anything else: parse the rest as an expression + check for pipeline
            _ => {
                if matches!(self.peek_kind(), TokenKind::LBracket)
                    && self.looks_like_generic_func_def()
                {
                    self.pos = save_pos;
                    return self.parse_func_def_with_docs(doc_comments);
                }
                self.pos = save_pos;
                let expr = self.parse_expression()?;
                self.finish_expr_as_statement(expr, start_span)
            }
        }
    }

    fn parse_func_def_with_docs(
        &mut self,
        doc_comments: Vec<String>,
    ) -> Result<Statement, ParseError> {
        let start_span = self.current_span();
        let name = self.expect_ident()?;
        let type_params = if self.check(&TokenKind::LBracket) {
            self.advance();
            self.parse_func_type_params()?
        } else {
            Vec::new()
        };

        // Parse parameters: `name: Type name: Type ...`
        let mut params = Vec::new();
        while self.check_ident() {
            let param_span = self.current_span();
            let param_name = self.expect_ident()?;
            let type_ann = if self.match_token(&TokenKind::Colon) {
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            let default_value = if self.match_token(&TokenKind::LtEq) {
                Some(self.parse_expression()?)
            } else {
                None
            };
            params.push(Param {
                name: param_name,
                type_annotation: type_ann,
                default_value,
                span: param_span,
            });
        }

        self.expect(&TokenKind::Eq)?;
        // Only skip Newline tokens, NOT Indent tokens — parse_block needs them
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }

        // Parse body (statements until `=> :Type` at same indentation)
        let body = self.parse_block()?;

        // Skip any remaining indent/newline tokens after block
        self.skip_newlines();

        // Parse optional return type `=> :Type`
        let return_type = if self.check(&TokenKind::FatArrow) {
            self.advance(); // consume `=>`
            if self.match_token(&TokenKind::Colon) {
                Some(self.parse_type_expr()?)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Statement::FuncDef(FuncDef {
            name,
            type_params,
            params,
            body,
            return_type,
            doc_comments,
            span: start_span,
        }))
    }

    fn parse_func_type_params(&mut self) -> Result<Vec<TypeParam>, ParseError> {
        let mut type_params = Vec::new();
        if self.check(&TokenKind::RBracket) {
            self.advance();
            return Ok(type_params);
        }

        loop {
            let name = self.expect_ident()?;
            let constraint = if self.check(&TokenKind::LtEq) {
                self.advance();
                self.expect(&TokenKind::Colon)?;
                Some(self.parse_type_expr()?)
            } else {
                None
            };
            type_params.push(TypeParam { name, constraint });
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(&TokenKind::RBracket)?;
        Ok(type_params)
    }

    fn looks_like_generic_func_def(&self) -> bool {
        if !self.check(&TokenKind::LBracket) {
            return false;
        }

        let mut i = self.pos;
        let mut depth = 0usize;
        while let Some(token) = self.tokens.get(i) {
            match &token.kind {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return matches!(
                            self.tokens.get(i + 1).map(|token| &token.kind),
                            Some(TokenKind::Ident(_)) | Some(TokenKind::Eq)
                        );
                    }
                }
                TokenKind::Eof => break,
                _ => {}
            }
            i += 1;
        }

        false
    }

    fn parse_mold_def_with_docs(
        &mut self,
        doc_comments: Vec<String>,
    ) -> Result<Statement, ParseError> {
        let start_span = self.current_span();
        self.expect_ident()?; // consume "Mold"
        self.expect(&TokenKind::LBracket)?;
        let mold_args = self.parse_mold_header_args()?;
        let type_params = Self::collect_mold_type_params(&mold_args);
        self.expect(&TokenKind::FatArrow)?;

        let name = self.expect_ident()?;
        let name_args = if self.check(&TokenKind::LBracket) {
            self.advance();
            Some(self.parse_mold_header_args()?)
        } else {
            None
        };

        self.expect(&TokenKind::Eq)?;
        let fields = self.parse_buchi_pack_fields()?;

        Ok(Statement::MoldDef(MoldDef {
            name,
            mold_args,
            name_args,
            type_params,
            fields,
            doc_comments,
            span: start_span,
        }))
    }

    fn try_parse_inheritance_with_headers(
        &mut self,
        doc_comments: Vec<String>,
    ) -> Result<Statement, ParseError> {
        let start_span = self.current_span();
        let parent = self.expect_ident()?;
        self.expect(&TokenKind::LBracket)?;
        let parent_args = self.parse_mold_header_args()?;
        self.expect(&TokenKind::FatArrow)?;

        let child = self.expect_ident()?;
        let child_args = if self.check(&TokenKind::LBracket) {
            self.advance();
            Some(self.parse_mold_header_args()?)
        } else {
            None
        };

        self.expect(&TokenKind::Eq)?;
        let fields = self.parse_buchi_pack_fields()?;

        Ok(Statement::InheritanceDef(InheritanceDef {
            parent,
            parent_args: Some(parent_args),
            child,
            child_args,
            fields,
            doc_comments,
            span: start_span,
        }))
    }

    fn parse_mold_header_args(&mut self) -> Result<Vec<MoldHeaderArg>, ParseError> {
        let mut args = Vec::new();
        if self.check(&TokenKind::RBracket) {
            self.advance();
            return Ok(args);
        }

        loop {
            let arg = if self.match_token(&TokenKind::Colon) {
                MoldHeaderArg::Concrete(self.parse_type_expr()?)
            } else {
                let name = self.expect_ident()?;
                let constraint = if self.check(&TokenKind::LtEq) {
                    self.advance();
                    self.expect(&TokenKind::Colon)?;
                    Some(self.parse_type_expr()?)
                } else {
                    None
                };
                MoldHeaderArg::TypeParam(TypeParam { name, constraint })
            };
            args.push(arg);
            if !self.match_token(&TokenKind::Comma) {
                break;
            }
        }

        self.expect(&TokenKind::RBracket)?;
        Ok(args)
    }

    fn collect_mold_type_params(args: &[MoldHeaderArg]) -> Vec<TypeParam> {
        args.iter()
            .filter_map(|arg| match arg {
                MoldHeaderArg::TypeParam(tp) => Some(tp.clone()),
                MoldHeaderArg::Concrete(_) => None,
            })
            .collect()
    }

    fn parse_import(&mut self) -> Result<Statement, ParseError> {
        let start_span = self.current_span();
        self.expect(&TokenKind::Import)?; // consume `>>>`

        // Parse path tokens, detecting @version pattern at token level.
        // The lexer tokenizes "1.0" as FloatLiteral(1.0) which loses precision
        // on to_string(), so we must parse version separately via parse_version_string.
        let mut path = String::new();
        let mut version = None;
        while !self.check(&TokenKind::FatArrow)
            && !self.is_at_end()
            && !matches!(self.peek_kind(), TokenKind::Newline)
        {
            // Detect @version: @ followed by a generation identifier (lowercase letters)
            // or a number token (legacy SemVer support)
            if self.check(&TokenKind::At) {
                let next = self.peek_at(1).kind.clone();
                let is_version = match &next {
                    TokenKind::Ident(s) => s.chars().all(|c| c.is_ascii_lowercase()),
                    TokenKind::IntLiteral(_) | TokenKind::FloatLiteral(_) => true,
                    _ => false,
                };
                if is_version {
                    self.advance(); // consume @
                    version = Some(self.parse_version_string()?);
                    break;
                }
            }

            let tok = self.advance();
            match &tok.kind {
                TokenKind::Ident(s) => path.push_str(s),
                TokenKind::Dot => path.push('.'),
                TokenKind::Slash => path.push('/'),
                TokenKind::Minus => path.push('-'),
                TokenKind::At => path.push('@'),
                TokenKind::Colon => path.push(':'),
                TokenKind::IntLiteral(n) => path.push_str(&n.to_string()),
                TokenKind::FloatLiteral(n) => path.push_str(&n.to_string()),
                TokenKind::Placeholder => path.push('_'),
                TokenKind::Gt => path.push('>'),
                _ => {
                    // Unexpected token inside import path — stringify for
                    // recovery.  A parse error for the overall import is
                    // reported separately, so this is best-effort.
                    let text = format!("{:?}", tok.kind);
                    path.push_str(&text);
                }
            }
        }

        // Expect `=> @(symbols)`
        let mut symbols = Vec::new();
        if self.match_token(&TokenKind::FatArrow) && self.match_token(&TokenKind::At) {
            self.expect(&TokenKind::LParen)?;
            while !self.check(&TokenKind::RParen) && !self.is_at_end() {
                let sym_name = self.expect_ident()?;
                let alias = if self.match_token(&TokenKind::FatArrow)
                    || self.match_token(&TokenKind::Colon)
                {
                    Some(self.expect_ident()?)
                } else {
                    None
                };
                symbols.push(ImportSymbol {
                    name: sym_name,
                    alias,
                });
                self.match_token(&TokenKind::Comma);
            }
            self.expect(&TokenKind::RParen)?;
        }

        // Local imports (relative or absolute path) must end with .td extension.
        // Package imports (e.g. author/pkg) are excluded.
        let is_local_import =
            path.starts_with("./") || path.starts_with("../") || path.starts_with('/');
        if is_local_import && !path.ends_with(".td") {
            return Err(ParseError {
                message: format!(
                    "Import '>>> {}' must include .td extension. Use '>>> {}.td' instead.",
                    path, path
                ),
                span: start_span,
            });
        }

        Ok(Statement::Import(ImportStmt {
            path,
            version,
            symbols,
            span: start_span,
        }))
    }

    fn parse_export(&mut self) -> Result<Statement, ParseError> {
        let start_span = self.current_span();
        self.expect(&TokenKind::Export)?; // consume `<<<`

        let mut version = None;
        let mut path = None;

        // Check for version: <<<@a.3 or <<<@b
        // Distinguish from <<<@(symbols) by peeking ahead
        if self.check(&TokenKind::At) {
            // @( = buchi pack symbols (existing behavior)
            // @<gen> or @<digit> = version string
            let next_kind = self.peek_at(1).kind.clone();
            let is_version = match &next_kind {
                TokenKind::Ident(s) => s.chars().all(|c| c.is_ascii_lowercase()),
                TokenKind::IntLiteral(_) | TokenKind::FloatLiteral(_) => true,
                _ => false,
            };
            if is_version {
                self.advance(); // consume @
                version = Some(self.parse_version_string()?);
            }
        }

        // Parse symbols @(...) or single symbol or path
        let mut symbols = Vec::new();
        if self.match_token(&TokenKind::At) {
            self.expect(&TokenKind::LParen)?;
            while !self.check(&TokenKind::RParen) && !self.is_at_end() {
                let sym = self.expect_ident()?;
                symbols.push(sym);
                self.match_token(&TokenKind::Comma);
            }
            self.expect(&TokenKind::RParen)?;
        } else if self.check(&TokenKind::Dot) || self.check(&TokenKind::Slash) {
            // Path: <<< ./main.td or <<< /path
            let mut p = String::new();
            while !self.is_at_end()
                && !matches!(self.peek_kind(), TokenKind::Newline | TokenKind::FatArrow)
            {
                let tok = self.advance();
                match &tok.kind {
                    TokenKind::Ident(s) => p.push_str(s),
                    TokenKind::Dot => p.push('.'),
                    TokenKind::Slash => p.push('/'),
                    TokenKind::Minus => p.push('-'),
                    TokenKind::Placeholder => p.push('_'),
                    _ => break,
                }
            }
            path = Some(p);
        } else if !self.is_at_end() && !matches!(self.peek_kind(), TokenKind::Newline) {
            let sym = self.expect_ident()?;
            symbols.push(sym);
        }

        Ok(Statement::Export(ExportStmt {
            version,
            symbols,
            path,
            span: start_span,
        }))
    }

    /// Parse a version string in `@gen.num` or `@gen` format.
    ///
    /// Grammar (state machine):
    /// ```text
    ///   start ──Ident(gen)──> gen_only ──Dot──Int(num)──> exact
    ///                                                       │
    ///                                            Dot──label──> labeled
    ///   start ──Float/Int──> legacy_semver ──Dot──Int(patch)──> semver_full
    /// ```
    ///
    /// Examples:
    /// - `@a.3` -> "a.3" (exact: generation a, publish #3)
    /// - `@b`   -> "b"   (generation-only: latest in generation b)
    /// - `@aa.12` -> "aa.12" (multi-letter generation)
    /// - `@a.1.beta` -> "a.1.beta" (labeled release)
    /// - `@a.3.gen-2-stable` -> "a.3.gen-2-stable" (hyphenated label)
    ///
    /// Also supports legacy SemVer format for backward compatibility:
    /// - `@1.0.0` -> "1.0.0"
    fn parse_version_string(&mut self) -> Result<String, ParseError> {
        let mut ver = String::new();

        match self.peek_kind().clone() {
            TokenKind::Ident(generation) => {
                // gen.num format: @a.3, @b.12, @aa.1
                // gen.num.label format: @a.1.alpha, @x.34.gen-2-stable
                // gen-only format: @a, @b, @aa
                if !generation.chars().all(|c| c.is_ascii_lowercase()) {
                    return Err(self.error_at_current(
                        "Generation must be lowercase letters (a-z, aa-zz, ...)",
                    ));
                }
                ver.push_str(&generation);
                self.advance();

                // Optional .num
                if self.check(&TokenKind::Dot) {
                    let after_dot = self.peek_at(1).kind.clone();
                    if matches!(after_dot, TokenKind::IntLiteral(_)) {
                        self.advance(); // consume dot
                        ver.push('.');
                        if let TokenKind::IntLiteral(n) = self.peek_kind().clone() {
                            ver.push_str(&n.to_string());
                            self.advance();
                        }

                        // Optional .label ([a-z0-9][a-z0-9-]*)
                        // Labels may contain hyphens, which the lexer splits into
                        // multiple tokens: Ident/Int + Minus + Ident/Int + ...
                        // e.g. "gen-2-stable" → Ident("gen") Minus Int(2) Minus Ident("stable")
                        if self.check(&TokenKind::Dot) {
                            let after_label_dot = self.peek_at(1).kind.clone();
                            let label_starts = matches!(
                                after_label_dot,
                                TokenKind::Ident(_) | TokenKind::IntLiteral(_)
                            );
                            if label_starts {
                                // Don't consume dot yet — verify it's a valid label start
                                let label = self.try_parse_version_label();
                                if let Some(l) = label {
                                    ver.push('.');
                                    ver.push_str(&l);
                                }
                            }
                        }
                    }
                    // Dot followed by non-integer → gen-only (don't consume dot)
                }
            }
            // Legacy SemVer support (e.g. core-bundled packages still use 1.0.0)
            TokenKind::FloatLiteral(f) => {
                let s = f.to_string();
                if s.contains('.') {
                    ver.push_str(&s);
                } else {
                    ver.push_str(&format!("{}.0", s));
                }
                self.advance();
            }
            TokenKind::IntLiteral(n) => {
                ver.push_str(&n.to_string());
                self.advance();
                self.expect(&TokenKind::Dot)?;
                ver.push('.');
                if let TokenKind::IntLiteral(n) = self.peek_kind().clone() {
                    ver.push_str(&n.to_string());
                    self.advance();
                } else {
                    return Err(self.error_at_current("Expected minor version number"));
                }
            }
            _ => {
                return Err(self.error_at_current(
                    "Expected generation identifier (a, b, ..., z, aa, ...) or version number after @"
                ));
            }
        }

        // Legacy SemVer: optional .patch
        if self.check(&TokenKind::Dot) {
            let after_dot = self.peek_at(1).kind.clone();
            if matches!(after_dot, TokenKind::IntLiteral(_)) {
                self.advance(); // consume dot
                ver.push('.');
                if let TokenKind::IntLiteral(n) = self.peek_kind().clone() {
                    ver.push_str(&n.to_string());
                    self.advance();
                }
            }
        }

        Ok(ver)
    }

    /// Try to parse a version label after gen.num.
    /// Consumes the dot and label tokens if successful. Returns None if not a valid label.
    /// Label pattern: [a-z0-9][a-z0-9-]* (no trailing hyphen)
    /// Lexer splits "gen-2-stable" into Ident("gen") Minus Int(2) Minus Ident("stable")
    fn try_parse_version_label(&mut self) -> Option<String> {
        // Save position for backtracking
        let saved_pos = self.pos;

        self.advance(); // consume dot

        let mut label = String::new();
        loop {
            match self.peek_kind().clone() {
                TokenKind::Ident(s) => {
                    // Label segments must be [a-z0-9] only (no uppercase)
                    if !s
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                    {
                        break;
                    }
                    if label.is_empty() {
                        // First segment must start with [a-z] (not digit — digits come as IntLiteral)
                        if !s.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
                            break;
                        }
                    }
                    label.push_str(&s);
                    self.advance();
                }
                TokenKind::IntLiteral(n) => {
                    label.push_str(&n.to_string());
                    self.advance();
                }
                _ => break,
            }

            // Check for hyphen continuation
            if self.check(&TokenKind::Minus) {
                // Peek ahead: hyphen must be followed by ident or int to continue the label
                let after_minus = self.peek_at(1).kind.clone();
                if matches!(after_minus, TokenKind::Ident(_) | TokenKind::IntLiteral(_)) {
                    label.push('-');
                    self.advance(); // consume minus
                } else {
                    break; // trailing hyphen → stop
                }
            } else {
                break;
            }
        }

        if label.is_empty() {
            // Backtrack
            self.pos = saved_pos;
            None
        } else {
            Some(label)
        }
    }

    fn parse_error_ceiling(&mut self) -> Result<Statement, ParseError> {
        let start_span = self.current_span();
        self.expect(&TokenKind::ErrorCeiling)?; // consume `|==`

        let error_param = self.expect_ident()?;
        self.expect(&TokenKind::Colon)?;
        let error_type = self.parse_type_expr()?;
        self.expect(&TokenKind::Eq)?;
        // Only skip Newline tokens, NOT Indent tokens — parse_block needs them
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }

        let handler_body = self.parse_block()?;

        // Skip any remaining indent/newline tokens after block
        self.skip_newlines();

        let return_type = if self.check(&TokenKind::FatArrow) {
            self.advance();
            if self.match_token(&TokenKind::Colon) {
                Some(self.parse_type_expr()?)
            } else {
                None
            }
        } else {
            None
        };

        Ok(Statement::ErrorCeiling(ErrorCeiling {
            error_param,
            error_type,
            handler_body,
            return_type,
            span: start_span,
        }))
    }

    // ── Block parsing ────────────────────────────────────────
    //
    // Taida uses indentation-sensitive block parsing.  The lexer emits
    // `Indent(n)` tokens at the start of every non-blank line, and the
    // parser uses these to decide when a block begins and ends:
    //   1. `detect_block_indent()` skips leading newlines and reads the
    //      first `Indent(n)` token to establish the block's indentation level.
    //   2. Each iteration checks `current_line_indent()`: if the indent is
    //      less than the block level the block is considered closed (dedent).
    //   3. `skip_blank_lines()` absorbs consecutive newlines between statements.

    fn parse_block(&mut self) -> Result<Vec<Statement>, ParseError> {
        let mut stmts = Vec::new();

        // Skip newlines and detect block indentation level
        // The block starts at the indentation of the first statement
        let block_indent = self.detect_block_indent();

        // Parse statements at this indentation level or deeper
        while !self.is_at_end() {
            // Skip blank lines (consecutive newlines)
            self.skip_blank_lines();

            // Check for return type: `=> :Type` at block boundary
            if self.check(&TokenKind::FatArrow) {
                break;
            }
            // Check for closing tokens
            if matches!(
                self.peek_kind(),
                TokenKind::RParen | TokenKind::RBracket | TokenKind::Eof
            ) {
                break;
            }

            // Check indentation level: if current line's indent < block_indent, dedent
            if block_indent > 0 {
                let current_indent = self.current_line_indent();
                if current_indent < block_indent {
                    break;
                }
                // Consume the indent token(s) for this line
                if matches!(self.peek_kind(), TokenKind::Indent(_)) {
                    self.advance();
                }
            }

            match self.parse_statement() {
                Ok(stmt) => stmts.push(stmt),
                Err(e) => {
                    self.errors.push(e);
                    self.synchronize();
                }
            }
        }

        Ok(stmts)
    }

    /// Detect the indentation level of the upcoming block.
    fn detect_block_indent(&mut self) -> usize {
        // Skip newlines to find the first indent/non-whitespace token
        while matches!(self.peek_kind(), TokenKind::Newline) {
            self.advance();
        }
        match self.peek_kind() {
            TokenKind::Indent(n) => *n,
            _ => 0,
        }
    }

    /// Get the indentation level at the current position.
    fn current_line_indent(&self) -> usize {
        match self.peek_kind() {
            TokenKind::Indent(n) => *n,
            // If we see a non-indent token, the line has 0 indentation
            _ => 0,
        }
    }

    /// Skip blank lines (newlines possibly followed by more newlines).
    fn skip_blank_lines(&mut self) {
        while matches!(self.peek_kind(), TokenKind::Newline) {
            // Check if the next non-newline token is an indent or content
            self.advance();
        }
    }

    // ── Type expression parsing ──────────────────────────────

    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        let base = self.parse_type_expr_atom()?;

        // Check for function type: `T => :U` (single param) or after atom
        // If we see `=>` after the base type, this is a function type signature
        if self.check(&TokenKind::FatArrow) {
            self.advance(); // consume `=>`
            // The return type follows after `:`
            if self.check(&TokenKind::Colon) {
                self.advance(); // consume `:`
            }
            let ret = self.parse_type_expr()?;
            let params = match base {
                TypeExpr::Named(ref n) if n == "_" => vec![], // `_ => :T` = no-arg function
                _ => vec![base],
            };
            return Ok(TypeExpr::Function(params, Box::new(ret)));
        }

        Ok(base)
    }

    /// Parse a single type expression atom (without function arrow).
    fn parse_type_expr_atom(&mut self) -> Result<TypeExpr, ParseError> {
        // `@(...)` buchi pack type
        if self.check(&TokenKind::At) {
            let save = self.pos;
            self.advance();
            if self.check(&TokenKind::LParen) {
                self.advance();
                let mut fields = Vec::new();
                while !self.check(&TokenKind::RParen) && !self.is_at_end() {
                    self.skip_newlines();
                    if self.check(&TokenKind::RParen) {
                        break;
                    }
                    let field_span = self.current_span();
                    let field_name = self.expect_ident()?;
                    self.expect(&TokenKind::Colon)?;
                    let field_type = self.parse_type_expr()?;
                    fields.push(FieldDef {
                        name: field_name,
                        type_annotation: Some(field_type),
                        default_value: None,
                        is_method: false,
                        method_def: None,
                        doc_comments: vec![],
                        span: field_span,
                    });
                    self.match_token(&TokenKind::Comma);
                    self.skip_newlines();
                }
                self.expect(&TokenKind::RParen)?;
                return Ok(TypeExpr::BuchiPack(fields));
            } else if self.check(&TokenKind::LBracket) {
                // `@[T]` list type
                self.advance();
                let inner = self.parse_type_expr()?;
                self.expect(&TokenKind::RBracket)?;
                return Ok(TypeExpr::List(Box::new(inner)));
            }
            self.pos = save;
        }

        // Placeholder `_` as type (used in `_ => :T` for no-arg function type,
        // or as type inference placeholder in `Result[T, _]`)
        if self.check(&TokenKind::Placeholder) {
            self.advance();
            return Ok(TypeExpr::Named("_".to_string()));
        }

        // Named type or generic type
        let name = self.expect_ident()?;

        // Check for generic: `Name[T, E]`
        if self.check(&TokenKind::LBracket) {
            self.advance();
            let mut type_args = Vec::new();
            while !self.check(&TokenKind::RBracket) && !self.is_at_end() {
                type_args.push(self.parse_type_expr()?);
                self.match_token(&TokenKind::Comma);
            }
            self.expect(&TokenKind::RBracket)?;
            return Ok(TypeExpr::Generic(name, type_args));
        }

        Ok(TypeExpr::Named(name))
    }

    // ── Buchi pack field parsing ─────────────────────────────

    fn parse_buchi_pack_fields(&mut self) -> Result<Vec<FieldDef>, ParseError> {
        self.expect(&TokenKind::At)?;
        self.expect(&TokenKind::LParen)?;
        self.skip_newlines();

        let mut fields = Vec::new();
        while !self.check(&TokenKind::RParen) && !self.is_at_end() {
            self.skip_newlines();
            if self.check(&TokenKind::RParen) {
                break;
            }

            // Collect doc comments for this field
            let field_docs = self.collect_doc_comments();
            self.skip_newlines();
            if self.check(&TokenKind::RParen) {
                break;
            }

            let field_span = self.current_span();

            // Special case: `unmold _ = body => :T` — custom unmold definition
            // The `_` placeholder represents the filling value (implicit parameter).
            // Parsed as a method-like field with name "unmold" and zero explicit params.
            if matches!(self.peek_kind(), TokenKind::Ident(s) if s == "unmold")
                && self.peek_at(1).kind == TokenKind::Placeholder
            {
                let _unmold_name = self.expect_ident()?; // consume "unmold"
                self.advance(); // consume `_`

                // Parse optional `=` then body, like a method definition
                self.expect(&TokenKind::Eq)?;
                while matches!(self.peek_kind(), TokenKind::Newline) {
                    self.advance();
                }
                let body = self.parse_block()?;
                self.skip_newlines();

                let return_type = if self.check(&TokenKind::FatArrow) {
                    self.advance();
                    if self.match_token(&TokenKind::Colon) {
                        Some(self.parse_type_expr()?)
                    } else {
                        None
                    }
                } else {
                    None
                };

                fields.push(FieldDef {
                    name: "unmold".to_string(),
                    type_annotation: None,
                    default_value: None,
                    is_method: true,
                    method_def: Some(FuncDef {
                        name: "unmold".to_string(),
                        type_params: Vec::new(),
                        params: Vec::new(), // no explicit params; filling is accessed by name
                        body,
                        return_type,
                        doc_comments: field_docs.clone(),
                        span: field_span.clone(),
                    }),
                    doc_comments: field_docs,
                    span: field_span,
                });

                self.match_token(&TokenKind::Comma);
                self.skip_newlines();
                continue;
            }

            let field_name = self.expect_ident()?;

            // Check if this is a type annotation field or a method
            if self.check(&TokenKind::Colon) {
                self.advance();
                let field_type = self.parse_type_expr()?;

                // Check for default value
                let default = if self.check(&TokenKind::LtEq) {
                    self.advance();
                    Some(self.parse_expression()?)
                } else {
                    None
                };

                fields.push(FieldDef {
                    name: field_name,
                    type_annotation: Some(field_type),
                    default_value: default,
                    is_method: false,
                    method_def: None,
                    doc_comments: field_docs,
                    span: field_span,
                });
            } else if self.check(&TokenKind::LtEq) {
                // `field <= value`
                self.advance();
                let value = self.parse_expression()?;
                fields.push(FieldDef {
                    name: field_name,
                    type_annotation: None,
                    default_value: Some(value),
                    is_method: false,
                    method_def: None,
                    doc_comments: field_docs,
                    span: field_span,
                });
            } else if self.check(&TokenKind::Eq) || self.check_ident() {
                // Method definition: name [params] = body [=> :ReturnType]
                let mut params = Vec::new();
                while self.check_ident() {
                    let param_span = self.current_span();
                    let param_name = self.expect_ident()?;
                    let type_ann = if self.match_token(&TokenKind::Colon) {
                        Some(self.parse_type_expr()?)
                    } else {
                        None
                    };
                    params.push(Param {
                        name: param_name,
                        type_annotation: type_ann,
                        default_value: None,
                        span: param_span,
                    });
                }

                self.expect(&TokenKind::Eq)?;
                while matches!(self.peek_kind(), TokenKind::Newline) {
                    self.advance();
                }

                let body = self.parse_block()?;
                self.skip_newlines();

                let return_type = if self.check(&TokenKind::FatArrow) {
                    self.advance();
                    if self.match_token(&TokenKind::Colon) {
                        Some(self.parse_type_expr()?)
                    } else {
                        None
                    }
                } else {
                    None
                };

                fields.push(FieldDef {
                    name: field_name.clone(),
                    type_annotation: None,
                    default_value: None,
                    is_method: true,
                    method_def: Some(FuncDef {
                        name: field_name,
                        type_params: Vec::new(),
                        params,
                        body,
                        return_type,
                        doc_comments: field_docs.clone(),
                        span: field_span.clone(),
                    }),
                    doc_comments: field_docs,
                    span: field_span,
                });
            } else {
                // Just a field name with no type, used for shorthand
                fields.push(FieldDef {
                    name: field_name,
                    type_annotation: None,
                    default_value: None,
                    is_method: false,
                    method_def: None,
                    doc_comments: field_docs,
                    span: field_span,
                });
            }

            self.match_token(&TokenKind::Comma);
            self.skip_newlines();
        }

        self.expect(&TokenKind::RParen)?;
        Ok(fields)
    }

    // ── Pipeline / Assignment helpers ────────────────────────

    /// After parsing an initial expression, check for `=> ...` pipeline chains.
    /// Returns a Statement:
    /// - If the chain ends with `=> ident` (no call/access), it becomes an Assignment.
    /// - Otherwise the entire chain is wrapped as `Expr::Pipeline`.
    /// - If `]=>` follows instead, wraps as UnmoldForward.
    /// - If no `=>` or `]=>` follows, returns `Statement::Expr(expr)`.
    fn finish_expr_as_statement(
        &mut self,
        expr: Expr,
        start_span: Span,
    ) -> Result<Statement, ParseError> {
        if self.check(&TokenKind::FatArrow) {
            // Pipeline: expr => step => step => ...
            let mut steps: Vec<Expr> = vec![expr];
            while self.check(&TokenKind::FatArrow) {
                let save_before_arrow = self.pos;
                self.advance(); // consume `=>`

                // Check for return type annotation `=> :Type` — not a pipeline step
                if self.check(&TokenKind::Colon) {
                    // This is a return type annotation, restore position before `=>`
                    self.pos = save_before_arrow;
                    break;
                }

                // Check for newline/EOF — pipeline ends
                if self.is_at_end()
                    || matches!(self.peek_kind(), TokenKind::Newline | TokenKind::Indent(_))
                {
                    self.pos = save_before_arrow;
                    break;
                }

                let step = self.parse_expression()?;
                steps.push(step);
            }

            // Single-direction constraint: => used, so <= must not follow
            if self.check(&TokenKind::LtEq) {
                return Err(ParseError {
                    message: "E0301: 単一方向制約違反 — 一つの文内で => と <= を混在させることはできません".to_string(),
                    span: self.current_span(),
                });
            }

            if steps.len() == 1 {
                // No actual pipeline steps parsed (e.g., we hit `=> :Type`)
                // Re-check for ]=>
                if self.check(&TokenKind::UnmoldForward) {
                    let span = self.current_span();
                    self.advance();
                    let target = self.expect_ident()?;
                    return Ok(Statement::UnmoldForward(UnmoldForwardStmt {
                        source: steps.into_iter().next().unwrap(),
                        target,
                        span,
                    }));
                }
                return Ok(Statement::Expr(steps.into_iter().next().unwrap()));
            }

            // Check if the last step is a simple identifier — that's an assignment target
            if let Some(Expr::Ident(name, _)) = steps.last() {
                let target = name.clone();
                let pipeline_steps: Vec<Expr> = steps[..steps.len() - 1].to_vec();
                let value = if pipeline_steps.len() == 1 {
                    pipeline_steps.into_iter().next().unwrap()
                } else {
                    Expr::Pipeline(pipeline_steps, start_span.clone())
                };
                return Ok(Statement::Assignment(Assignment {
                    target,
                    type_annotation: None,
                    value,
                    span: start_span,
                }));
            }

            // Not ending with identifier — pure pipeline expression
            Ok(Statement::Expr(Expr::Pipeline(steps, start_span)))
        } else if self.check(&TokenKind::UnmoldForward) {
            let span = self.current_span();
            self.advance(); // consume `]=>`
            let target = self.expect_ident()?;
            Ok(Statement::UnmoldForward(UnmoldForwardStmt {
                source: expr,
                target,
                span,
            }))
        } else {
            Ok(Statement::Expr(expr))
        }
    }

    // ── Expression Parsing (Pratt Parser) ────────────────────
}

#[path = "parser_expr.rs"]
mod parser_expr;

/// Convenience function: parse source code into a Program.
pub fn parse(source: &str) -> (Program, Vec<ParseError>) {
    let (tokens, lex_errors) = crate::lexer::tokenize(source);
    if !lex_errors.is_empty() {
        let parse_errors: Vec<ParseError> = lex_errors
            .into_iter()
            .map(|e| ParseError {
                message: e.message,
                span: e.span,
            })
            .collect();
        return (Program { statements: vec![] }, parse_errors);
    }
    Parser::new(tokens).parse()
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
