use super::token::{Span, Token, TokenKind};

/// Lexer error with location information.
#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Lexer error at line {}, column {}: {}",
            self.span.line, self.span.column, self.message
        )
    }
}

impl std::error::Error for LexError {}

/// The Taida Lang lexer.
///
/// Converts source text into a stream of tokens. Handles:
/// - All 10 Taida operators: `=`, `=>`, `<=`, `]=>`, `<=[`, `|==`, `|`, `|>`, `>>>`, `<<<`
/// - Arithmetic: `+`, `-`, `*`, `/`, `%`
/// - Comparison: `==`, `!=`, `<`, `>`, `>=`
/// - Logical: `&&`, `||`, `!`
/// - Literals: integers, floats, strings (single/double/template), booleans, gorilla `><`
/// - Indentation tracking (2-space based)
/// - Comments: `//`, `/* */`, `///@`
pub struct Lexer {
    source: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    /// Whether we are at the start of a line (for indentation tracking).
    at_line_start: bool,
    tokens: Vec<Token>,
    errors: Vec<LexError>,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Self {
            source: source.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
            at_line_start: true,
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// Tokenize the entire source and return tokens + errors.
    pub fn tokenize(mut self) -> (Vec<Token>, Vec<LexError>) {
        while !self.is_at_end() {
            self.scan_token();
        }
        // Emit final EOF
        let span = Span::new(self.pos, self.pos, self.line, self.column);
        self.tokens.push(Token::new(TokenKind::Eof, span));
        (self.tokens, self.errors)
    }

    // ── Helpers ──────────────────────────────────────────────

    fn is_at_end(&self) -> bool {
        self.pos >= self.source.len()
    }

    fn peek(&self) -> char {
        if self.is_at_end() {
            '\0'
        } else {
            self.source[self.pos]
        }
    }

    fn peek_at(&self, offset: usize) -> char {
        let idx = self.pos + offset;
        if idx >= self.source.len() {
            '\0'
        } else {
            self.source[idx]
        }
    }

    fn advance(&mut self) -> char {
        let ch = self.source[self.pos];
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        ch
    }

    fn emit(&mut self, kind: TokenKind, start: usize, start_line: usize, start_col: usize) {
        let span = Span::new(start, self.pos, start_line, start_col);
        self.tokens.push(Token::new(kind, span));
    }

    fn error(&mut self, message: &str, start: usize, start_line: usize, start_col: usize) {
        self.errors.push(LexError {
            message: message.to_string(),
            span: Span::new(start, self.pos, start_line, start_col),
        });
    }

    // ── Main scan ────────────────────────────────────────────

    fn scan_token(&mut self) {
        // Handle indentation at line start
        if self.at_line_start {
            self.at_line_start = false;
            if self.peek() == ' ' {
                self.scan_indent();
                return;
            } else if self.peek() == '\t' {
                let start = self.pos;
                let line = self.line;
                let col = self.column;
                self.advance();
                self.error(
                    "Tab characters are not allowed. Use 2 spaces for indentation.",
                    start,
                    line,
                    col,
                );
                return;
            } else if self.peek() != '\n' && self.peek() != '\r' && !self.is_at_end() {
                // Emit zero indent at line start if no spaces
                self.emit(TokenKind::Indent(0), self.pos, self.line, self.column);
            }
        }

        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        let ch = self.advance();

        match ch {
            // Newlines
            '\n' => {
                self.emit(TokenKind::Newline, start, start_line, start_col);
                self.at_line_start = true;
            }
            '\r' => {
                // Handle \r\n
                if self.peek() == '\n' {
                    self.advance();
                }
                self.emit(TokenKind::Newline, start, start_line, start_col);
                self.at_line_start = true;
            }

            // Whitespace (non-newline, non-indent)
            ' ' => {
                // Skip spaces that aren't at line start
            }

            // Backslash (line continuation)
            '\\' => {
                self.emit(TokenKind::Backslash, start, start_line, start_col);
            }

            // Comments and division
            '/' => {
                if self.peek() == '/' {
                    self.advance(); // consume second /
                    self.scan_line_comment(start, start_line, start_col);
                } else if self.peek() == '*' {
                    self.advance(); // consume *
                    self.scan_block_comment(start, start_line, start_col);
                } else {
                    self.emit(TokenKind::Slash, start, start_line, start_col);
                }
            }

            // Operators starting with |
            '|' => {
                if self.peek() == '=' && self.peek_at(1) == '=' {
                    self.advance(); // =
                    self.advance(); // =
                    self.emit(TokenKind::ErrorCeiling, start, start_line, start_col);
                } else if self.peek() == '>' {
                    self.advance();
                    self.emit(TokenKind::PipeGt, start, start_line, start_col);
                } else if self.peek() == '|' {
                    self.advance();
                    self.emit(TokenKind::Or, start, start_line, start_col);
                } else {
                    self.emit(TokenKind::Pipe, start, start_line, start_col);
                }
            }

            // Operators starting with =
            '=' => {
                if self.peek() == '>' {
                    self.advance();
                    self.emit(TokenKind::FatArrow, start, start_line, start_col);
                } else if self.peek() == '=' {
                    self.advance();
                    self.emit(TokenKind::EqEq, start, start_line, start_col);
                } else {
                    self.emit(TokenKind::Eq, start, start_line, start_col);
                }
            }

            // Operators starting with <
            '<' => {
                if self.peek() == '=' && self.peek_at(1) == '[' {
                    self.advance(); // =
                    self.advance(); // [
                    self.emit(TokenKind::UnmoldBackward, start, start_line, start_col);
                } else if self.peek() == '=' {
                    self.advance();
                    self.emit(TokenKind::LtEq, start, start_line, start_col);
                } else if self.peek() == '<' && self.peek_at(1) == '<' {
                    self.advance(); // <
                    self.advance(); // <
                    self.emit(TokenKind::Export, start, start_line, start_col);
                } else {
                    self.emit(TokenKind::Lt, start, start_line, start_col);
                }
            }

            // Operators starting with >
            '>' => {
                if self.peek() == '>' && self.peek_at(1) == '>' {
                    self.advance(); // >
                    self.advance(); // >
                    self.emit(TokenKind::Import, start, start_line, start_col);
                } else if self.peek() == '=' {
                    self.advance();
                    self.emit(TokenKind::GtEq, start, start_line, start_col);
                } else if self.peek() == '<' {
                    // Gorilla literal ><
                    self.advance();
                    self.emit(TokenKind::Gorilla, start, start_line, start_col);
                } else {
                    self.emit(TokenKind::Gt, start, start_line, start_col);
                }
            }

            // Operators starting with ]
            ']' => {
                if self.peek() == '=' && self.peek_at(1) == '>' {
                    self.advance(); // =
                    self.advance(); // >
                    self.emit(TokenKind::UnmoldForward, start, start_line, start_col);
                } else {
                    self.emit(TokenKind::RBracket, start, start_line, start_col);
                }
            }

            // Operators starting with !
            '!' => {
                if self.peek() == '=' {
                    self.advance();
                    self.emit(TokenKind::BangEq, start, start_line, start_col);
                } else {
                    self.emit(TokenKind::Bang, start, start_line, start_col);
                }
            }

            // Operators starting with &
            '&' => {
                if self.peek() == '&' {
                    self.advance();
                    self.emit(TokenKind::And, start, start_line, start_col);
                } else {
                    self.error(
                        "Unexpected character '&'. Did you mean '&&'?",
                        start,
                        start_line,
                        start_col,
                    );
                }
            }

            // Simple single-character tokens
            '+' => self.emit(TokenKind::Plus, start, start_line, start_col),
            '-' => self.emit(TokenKind::Minus, start, start_line, start_col),
            '*' => self.emit(TokenKind::Star, start, start_line, start_col),
            '%' => self.emit(TokenKind::Percent, start, start_line, start_col),
            '(' => self.emit(TokenKind::LParen, start, start_line, start_col),
            ')' => self.emit(TokenKind::RParen, start, start_line, start_col),
            '[' => self.emit(TokenKind::LBracket, start, start_line, start_col),
            '@' => self.emit(TokenKind::At, start, start_line, start_col),
            ',' => self.emit(TokenKind::Comma, start, start_line, start_col),
            '.' => self.emit(TokenKind::Dot, start, start_line, start_col),
            ':' => self.emit(TokenKind::Colon, start, start_line, start_col),

            // String literals
            '"' => self.scan_string('"', start, start_line, start_col),
            '\'' => self.scan_string('\'', start, start_line, start_col),
            '`' => self.scan_template_string(start, start_line, start_col),

            // Numbers
            c if c.is_ascii_digit() => {
                self.scan_number(start, start_line, start_col);
            }

            // Identifiers and keywords
            c if c.is_alphabetic() || c == '_' => {
                self.scan_identifier(start, start_line, start_col);
            }

            _ => {
                self.error(
                    &format!("Unexpected character '{}'", ch),
                    start,
                    start_line,
                    start_col,
                );
            }
        }
    }

    // ── Indentation ──────────────────────────────────────────

    fn scan_indent(&mut self) {
        let start = self.pos;
        let start_line = self.line;
        let start_col = self.column;
        let mut count = 0;

        while self.peek() == ' ' {
            self.advance();
            count += 1;
        }

        // Check for tab characters after spaces
        if self.peek() == '\t' {
            self.error(
                "Tab characters are not allowed. Use 2 spaces for indentation.",
                self.pos,
                self.line,
                self.column,
            );
            self.advance();
            return;
        }

        // Don't emit indent for blank lines
        if self.peek() == '\n' || self.peek() == '\r' || self.is_at_end() {
            return;
        }

        // Don't emit indent for comment-only lines (still need to emit for content)
        self.emit(TokenKind::Indent(count), start, start_line, start_col);
    }

    // ── Numbers ──────────────────────────────────────────────

    fn scan_number(&mut self, start: usize, start_line: usize, start_col: usize) {
        // Prefixed integer literal: 0x / 0o / 0b
        // SAFETY: `start` is the position of the digit that triggered this call,
        // so it is always a valid index into `self.source`.
        let first = self.source[start];
        if first == '0' {
            let prefix = self.peek();
            let (base, is_prefixed) = match prefix {
                'x' | 'X' => (16, true),
                'o' | 'O' => (8, true),
                'b' | 'B' => (2, true),
                _ => (10, false),
            };
            if is_prefixed {
                self.advance(); // consume base prefix char
                let digits_start = self.pos;
                while self.peek().is_ascii_alphanumeric() || self.peek() == '_' {
                    self.advance();
                }

                let digits: String = self.source[digits_start..self.pos].iter().collect();
                if !Self::is_valid_grouped_digits(&digits, base) {
                    self.error(
                        &format!("Invalid base-{} integer literal", base),
                        start,
                        start_line,
                        start_col,
                    );
                    return;
                }

                let normalized = digits.replace('_', "");
                match i64::from_str_radix(&normalized, base) {
                    Ok(val) => self.emit(TokenKind::IntLiteral(val), start, start_line, start_col),
                    Err(_) => self.error(
                        &format!("Invalid integer literal: {}", normalized),
                        start,
                        start_line,
                        start_col,
                    ),
                }
                return;
            }
        }

        // Decimal integer / float / scientific notation (supports '_' separators)
        while self.peek().is_ascii_digit() || self.peek() == '_' {
            self.advance();
        }
        let int_part: String = self.source[start..self.pos].iter().collect();
        if !Self::is_valid_grouped_digits(&int_part, 10) {
            self.error(
                "Invalid integer literal (separator placement)",
                start,
                start_line,
                start_col,
            );
            return;
        }

        let mut is_float = false;
        let mut had_exponent = false;

        // Fractional part
        if self.peek() == '.' && (self.peek_at(1).is_ascii_digit() || self.peek_at(1) == '_') {
            is_float = true;
            self.advance(); // consume '.'
            let frac_start = self.pos;
            while self.peek().is_ascii_digit() || self.peek() == '_' {
                self.advance();
            }
            let frac_part: String = self.source[frac_start..self.pos].iter().collect();
            if !Self::is_valid_grouped_digits(&frac_part, 10) {
                self.error(
                    "Invalid float literal (fraction separator placement)",
                    start,
                    start_line,
                    start_col,
                );
                return;
            }
        }

        // Exponent part
        if self.peek() == 'e' || self.peek() == 'E' {
            is_float = true;
            had_exponent = true;
            self.advance(); // consume e/E
            if self.peek() == '+' || self.peek() == '-' {
                self.advance();
            }
            let exp_start = self.pos;
            while self.peek().is_ascii_digit() || self.peek() == '_' {
                self.advance();
            }
            let exp_part: String = self.source[exp_start..self.pos].iter().collect();
            if !Self::is_valid_grouped_digits(&exp_part, 10) {
                self.error(
                    "Invalid float literal (exponent digits)",
                    start,
                    start_line,
                    start_col,
                );
                return;
            }
        }

        let text: String = self.source[start..self.pos].iter().collect();
        let normalized = text.replace('_', "");
        if is_float || had_exponent {
            match normalized.parse::<f64>() {
                Ok(val) => self.emit(TokenKind::FloatLiteral(val), start, start_line, start_col),
                Err(_) => self.error(
                    &format!("Invalid float literal: {}", text),
                    start,
                    start_line,
                    start_col,
                ),
            }
        } else {
            match normalized.parse::<i64>() {
                Ok(val) => self.emit(TokenKind::IntLiteral(val), start, start_line, start_col),
                Err(_) => self.error(
                    &format!("Invalid integer literal: {}", text),
                    start,
                    start_line,
                    start_col,
                ),
            }
        }
    }

    fn is_valid_grouped_digits(text: &str, base: u32) -> bool {
        if text.is_empty() {
            return false;
        }
        let chars: Vec<char> = text.chars().collect();
        if chars.first() == Some(&'_') || chars.last() == Some(&'_') {
            return false;
        }
        let mut prev_is_underscore = false;
        let mut saw_digit = false;
        for ch in chars {
            if ch == '_' {
                if prev_is_underscore {
                    return false;
                }
                prev_is_underscore = true;
                continue;
            }
            let valid = match base {
                2 => matches!(ch, '0' | '1'),
                8 => ('0'..='7').contains(&ch),
                10 => ch.is_ascii_digit(),
                16 => ch.is_ascii_hexdigit(),
                _ => false,
            };
            if !valid {
                return false;
            }
            saw_digit = true;
            prev_is_underscore = false;
        }
        saw_digit
    }

    // ── Strings ──────────────────────────────────────────────

    fn scan_string(&mut self, quote: char, start: usize, start_line: usize, start_col: usize) {
        let mut value = String::new();

        while !self.is_at_end() && self.peek() != quote {
            if self.peek() == '\n' {
                self.error(
                    "Unterminated string literal (newline before closing quote)",
                    start,
                    start_line,
                    start_col,
                );
                return;
            }
            if self.peek() == '\\' {
                self.advance(); // consume backslash
                if let Some(decoded) =
                    self.scan_escape_sequence(quote, false, false, start, start_line, start_col)
                {
                    value.push_str(&decoded);
                } else {
                    return;
                }
            } else {
                value.push(self.advance());
            }
        }

        if self.is_at_end() {
            self.error("Unterminated string literal", start, start_line, start_col);
            return;
        }

        self.advance(); // consume closing quote
        self.emit(
            TokenKind::StringLiteral(value),
            start,
            start_line,
            start_col,
        );
    }

    fn scan_template_string(&mut self, start: usize, start_line: usize, start_col: usize) {
        let mut value = String::new();

        while !self.is_at_end() && self.peek() != '`' {
            if self.peek() == '\\' {
                self.advance();
                if let Some(decoded) =
                    self.scan_escape_sequence('`', true, true, start, start_line, start_col)
                {
                    value.push_str(&decoded);
                } else {
                    return;
                }
            } else {
                value.push(self.advance());
            }
        }

        if self.is_at_end() {
            self.error("Unterminated template string", start, start_line, start_col);
            return;
        }

        self.advance(); // consume closing backtick
        self.emit(
            TokenKind::TemplateLiteral(value),
            start,
            start_line,
            start_col,
        );
    }

    fn scan_escape_sequence(
        &mut self,
        terminator: char,
        allow_newline: bool,
        in_template: bool,
        start: usize,
        start_line: usize,
        start_col: usize,
    ) -> Option<String> {
        if self.is_at_end() {
            self.error(
                &format!(
                    "Unterminated escape sequence{}",
                    Self::escape_context(in_template)
                ),
                start,
                start_line,
                start_col,
            );
            return None;
        }

        let escaped = self.advance();
        match escaped {
            'n' => Some("\n".to_string()),
            't' => Some("\t".to_string()),
            'r' => Some("\r".to_string()),
            '\\' => Some("\\".to_string()),
            '\'' => Some("'".to_string()),
            '"' => Some("\"".to_string()),
            '`' if in_template => Some("`".to_string()),
            '$' if in_template => Some("$".to_string()),
            '0' => {
                if self.peek().is_ascii_digit() {
                    self.error(
                        &format!(
                            "Unsupported octal escape sequence{}: use \\0, \\xNN, or \\u{{HEX+}}",
                            Self::escape_context(in_template)
                        ),
                        start,
                        start_line,
                        start_col,
                    );
                    Some("\\0".to_string())
                } else {
                    Some("\0".to_string())
                }
            }
            'x' => Some(self.scan_hex_escape(in_template, start, start_line, start_col)),
            'u' => Some(self.scan_unicode_escape(
                terminator,
                allow_newline,
                in_template,
                start,
                start_line,
                start_col,
            )),
            _ => {
                self.error(
                    &format!(
                        "Invalid escape sequence{}: \\{}",
                        Self::escape_context(in_template),
                        escaped
                    ),
                    start,
                    start_line,
                    start_col,
                );
                Some(format!("\\{}", escaped))
            }
        }
    }

    fn scan_hex_escape(
        &mut self,
        in_template: bool,
        start: usize,
        start_line: usize,
        start_col: usize,
    ) -> String {
        let hi = self.peek();
        let lo = self.peek_at(1);
        if !(hi.is_ascii_hexdigit() && lo.is_ascii_hexdigit()) {
            self.error(
                &format!(
                    "Invalid hex escape sequence{}: expected two hex digits after \\x",
                    Self::escape_context(in_template)
                ),
                start,
                start_line,
                start_col,
            );
            return "\\x".to_string();
        }

        let hi = self.advance();
        let lo = self.advance();
        let value = u8::from_str_radix(&format!("{}{}", hi, lo), 16)
            .expect("validated hex escape should parse");
        (value as char).to_string()
    }

    fn scan_unicode_escape(
        &mut self,
        terminator: char,
        allow_newline: bool,
        in_template: bool,
        start: usize,
        start_line: usize,
        start_col: usize,
    ) -> String {
        if self.peek() != '{' {
            self.error(
                &format!(
                    "Unsupported Unicode escape sequence{}: use \\u{{HEX+}}, not \\uXXXX",
                    Self::escape_context(in_template)
                ),
                start,
                start_line,
                start_col,
            );
            return "\\u".to_string();
        }

        self.advance(); // consume `{`
        let mut raw = String::from("\\u{");
        let mut hex = String::new();

        while !self.is_at_end() && self.peek() != '}' {
            if self.peek() == terminator || (!allow_newline && self.peek() == '\n') {
                self.error(
                    &format!(
                        "Unterminated Unicode escape sequence{}",
                        Self::escape_context(in_template)
                    ),
                    start,
                    start_line,
                    start_col,
                );
                return raw;
            }

            let ch = self.advance();
            raw.push(ch);
            if !ch.is_ascii_hexdigit() {
                while !self.is_at_end() && self.peek() != '}' {
                    if self.peek() == terminator || (!allow_newline && self.peek() == '\n') {
                        self.error(
                            &format!(
                                "Invalid Unicode escape sequence{}: non-hex digit in \\u{{...}}",
                                Self::escape_context(in_template)
                            ),
                            start,
                            start_line,
                            start_col,
                        );
                        return raw;
                    }
                    let extra = self.advance();
                    raw.push(extra);
                }
                if self.peek() == '}' {
                    self.advance();
                    raw.push('}');
                }
                self.error(
                    &format!(
                        "Invalid Unicode escape sequence{}: non-hex digit in \\u{{...}}",
                        Self::escape_context(in_template)
                    ),
                    start,
                    start_line,
                    start_col,
                );
                return raw;
            }
            hex.push(ch);
        }

        if self.is_at_end() {
            self.error(
                &format!(
                    "Unterminated Unicode escape sequence{}",
                    Self::escape_context(in_template)
                ),
                start,
                start_line,
                start_col,
            );
            return raw;
        }

        self.advance(); // consume `}`
        raw.push('}');

        if hex.is_empty() {
            self.error(
                &format!(
                    "Invalid Unicode escape sequence{}: empty \\u{{}} is not allowed",
                    Self::escape_context(in_template)
                ),
                start,
                start_line,
                start_col,
            );
            return raw;
        }

        let code_point =
            u32::from_str_radix(&hex, 16).expect("validated unicode escape should parse");
        if code_point > 0x10FFFF {
            self.error(
                &format!(
                    "Invalid Unicode escape sequence{}: code point must be <= U+10FFFF",
                    Self::escape_context(in_template)
                ),
                start,
                start_line,
                start_col,
            );
            return raw;
        }
        if (0xD800..=0xDFFF).contains(&code_point) {
            self.error(
                &format!(
                    "Invalid Unicode escape sequence{}: surrogate code points are not allowed",
                    Self::escape_context(in_template)
                ),
                start,
                start_line,
                start_col,
            );
            return raw;
        }

        char::from_u32(code_point).map_or_else(
            || {
                self.error(
                    &format!(
                        "Invalid Unicode escape sequence{}: invalid scalar value",
                        Self::escape_context(in_template)
                    ),
                    start,
                    start_line,
                    start_col,
                );
                raw
            },
            |ch| ch.to_string(),
        )
    }

    fn escape_context(in_template: bool) -> &'static str {
        if in_template {
            " in template string"
        } else {
            ""
        }
    }

    // ── Identifiers & Keywords ───────────────────────────────

    fn scan_identifier(&mut self, start: usize, start_line: usize, start_col: usize) {
        while !self.is_at_end() && (self.peek().is_alphanumeric() || self.peek() == '_') {
            self.advance();
        }

        let text: String = self.source[start..self.pos].iter().collect();

        // Check for boolean literals and placeholder
        match text.as_str() {
            "true" => self.emit(TokenKind::BoolLiteral(true), start, start_line, start_col),
            "false" => self.emit(TokenKind::BoolLiteral(false), start, start_line, start_col),
            "_" => self.emit(TokenKind::Placeholder, start, start_line, start_col),
            _ => self.emit(TokenKind::Ident(text), start, start_line, start_col),
        }
    }

    // ── Comments ─────────────────────────────────────────────

    fn scan_line_comment(&mut self, start: usize, start_line: usize, start_col: usize) {
        // Check for doc comment `///@`
        if self.peek() == '/' && self.peek_at(1) == '@' {
            self.advance(); // consume third /
            self.advance(); // consume @
            let mut content = String::new();
            // Skip leading space if present
            if self.peek() == ' ' {
                self.advance();
            }
            while !self.is_at_end() && self.peek() != '\n' {
                content.push(self.advance());
            }
            self.emit(TokenKind::DocComment(content), start, start_line, start_col);
        } else {
            let mut content = String::new();
            // Skip leading space if present
            if self.peek() == ' ' {
                self.advance();
            }
            while !self.is_at_end() && self.peek() != '\n' {
                content.push(self.advance());
            }
            self.emit(
                TokenKind::LineComment(content),
                start,
                start_line,
                start_col,
            );
        }
    }

    fn scan_block_comment(&mut self, start: usize, start_line: usize, start_col: usize) {
        let mut content = String::new();
        let mut depth = 1; // Support nested block comments

        while !self.is_at_end() && depth > 0 {
            if self.peek() == '/' && self.peek_at(1) == '*' {
                content.push(self.advance());
                content.push(self.advance());
                depth += 1;
            } else if self.peek() == '*' && self.peek_at(1) == '/' {
                depth -= 1;
                if depth > 0 {
                    content.push(self.advance());
                    content.push(self.advance());
                } else {
                    self.advance(); // consume *
                    self.advance(); // consume /
                }
            } else {
                content.push(self.advance());
            }
        }

        if depth > 0 {
            self.error("Unterminated block comment", start, start_line, start_col);
            return;
        }

        self.emit(
            TokenKind::BlockComment(content),
            start,
            start_line,
            start_col,
        );
    }
}

/// Convenience function to tokenize source code.
pub fn tokenize(source: &str) -> (Vec<Token>, Vec<LexError>) {
    Lexer::new(source).tokenize()
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::TokenKind::*;

    /// Helper: tokenize and return only non-comment, non-newline, non-indent, non-EOF token kinds.
    fn tok_kinds(source: &str) -> Vec<TokenKind> {
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        tokens
            .into_iter()
            .map(|t| t.kind)
            .filter(|k| {
                !matches!(
                    k,
                    Newline | Eof | Indent(_) | LineComment(_) | BlockComment(_) | DocComment(_)
                )
            })
            .collect()
    }

    // ── Operator tests ───────────────────────────────────────

    #[test]
    fn test_definition_operator() {
        assert_eq!(
            tok_kinds("x = 42"),
            vec![Ident("x".into()), Eq, IntLiteral(42)]
        );
    }

    #[test]
    fn test_fat_arrow() {
        assert_eq!(
            tok_kinds("x => y"),
            vec![Ident("x".into()), FatArrow, Ident("y".into())]
        );
    }

    #[test]
    fn test_left_buchi() {
        assert_eq!(
            tok_kinds("x <= 42"),
            vec![Ident("x".into()), LtEq, IntLiteral(42)]
        );
    }

    #[test]
    fn test_unmold_forward() {
        assert_eq!(
            tok_kinds("opt ]=> value"),
            vec![Ident("opt".into()), UnmoldForward, Ident("value".into())]
        );
    }

    #[test]
    fn test_unmold_backward() {
        assert_eq!(
            tok_kinds("value <=[ opt"),
            vec![Ident("value".into()), UnmoldBackward, Ident("opt".into())]
        );
    }

    #[test]
    fn test_error_ceiling() {
        assert_eq!(
            tok_kinds("|== error"),
            vec![ErrorCeiling, Ident("error".into())]
        );
    }

    #[test]
    fn test_pipe_and_pipe_gt() {
        assert_eq!(
            tok_kinds("| x > 0 |> y"),
            vec![
                Pipe,
                Ident("x".into()),
                Gt,
                IntLiteral(0),
                PipeGt,
                Ident("y".into())
            ]
        );
    }

    #[test]
    fn test_import() {
        assert_eq!(tok_kinds(">>> std"), vec![Import, Ident("std".into())]);
    }

    #[test]
    fn test_export() {
        assert_eq!(tok_kinds("<<< x"), vec![Export, Ident("x".into())]);
    }

    // ── Arithmetic ───────────────────────────────────────────

    #[test]
    fn test_arithmetic() {
        assert_eq!(
            tok_kinds("1 + 2 - 3 * 4 / 5 % 6"),
            vec![
                IntLiteral(1),
                Plus,
                IntLiteral(2),
                Minus,
                IntLiteral(3),
                Star,
                IntLiteral(4),
                Slash,
                IntLiteral(5),
                Percent,
                IntLiteral(6)
            ]
        );
    }

    // ── Comparison ───────────────────────────────────────────

    #[test]
    fn test_comparison() {
        assert_eq!(
            tok_kinds("a == b != c < d > e >= f"),
            vec![
                Ident("a".into()),
                EqEq,
                Ident("b".into()),
                BangEq,
                Ident("c".into()),
                Lt,
                Ident("d".into()),
                Gt,
                Ident("e".into()),
                GtEq,
                Ident("f".into())
            ]
        );
    }

    // ── Logical ──────────────────────────────────────────────

    #[test]
    fn test_logical() {
        assert_eq!(
            tok_kinds("a && b || !c"),
            vec![
                Ident("a".into()),
                And,
                Ident("b".into()),
                Or,
                Bang,
                Ident("c".into())
            ]
        );
    }

    // ── Literals ─────────────────────────────────────────────

    #[test]
    fn test_integer_literal() {
        assert_eq!(tok_kinds("42"), vec![IntLiteral(42)]);
    }

    #[test]
    fn test_float_literal() {
        assert_eq!(tok_kinds("3.14"), vec![FloatLiteral(314.0 / 100.0)]);
    }

    #[test]
    fn test_prefixed_integer_literals() {
        assert_eq!(tok_kinds("0xFF"), vec![IntLiteral(255)]);
        assert_eq!(tok_kinds("0o77"), vec![IntLiteral(63)]);
        assert_eq!(tok_kinds("0b1010"), vec![IntLiteral(10)]);
    }

    #[test]
    fn test_scientific_notation_literals() {
        assert_eq!(tok_kinds("1e9"), vec![FloatLiteral(1e9)]);
        assert_eq!(tok_kinds("1.5e-3"), vec![FloatLiteral(1.5e-3)]);
        assert_eq!(tok_kinds("2E+8"), vec![FloatLiteral(2e8)]);
    }

    #[test]
    fn test_numeric_separator_literals() {
        assert_eq!(tok_kinds("1_000_000"), vec![IntLiteral(1_000_000)]);
        assert_eq!(tok_kinds("0xFF_FF"), vec![IntLiteral(0xFF_FF)]);
        assert_eq!(tok_kinds("1.234_567"), vec![FloatLiteral(1.234_567)]);
        assert_eq!(tok_kinds("1e1_0"), vec![FloatLiteral(1e10)]);
    }

    #[test]
    fn test_invalid_prefixed_integer_literal_reports_error() {
        let (_, errors) = tokenize("0b102");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Invalid base-2 integer literal")),
            "Expected invalid binary literal error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_invalid_numeric_separator_reports_error() {
        let (_, errors) = tokenize("1__0");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Invalid integer literal")),
            "Expected invalid separator error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_invalid_exponent_reports_error() {
        let (_, errors) = tokenize("1e_2");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Invalid float literal")),
            "Expected invalid exponent error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_string_double_quote() {
        assert_eq!(tok_kinds(r#""hello""#), vec![StringLiteral("hello".into())]);
    }

    #[test]
    fn test_string_single_quote() {
        assert_eq!(tok_kinds("'world'"), vec![StringLiteral("world".into())]);
    }

    #[test]
    fn test_string_escape_sequences() {
        assert_eq!(
            tok_kinds(r#""line1\nline2\t\"end""#),
            vec![StringLiteral("line1\nline2\t\"end".into())]
        );
    }

    #[test]
    fn test_string_rc3_escape_sequences() {
        assert_eq!(
            tok_kinds(r#""\0\x41\u{1F600}""#),
            vec![StringLiteral("\0A😀".into())]
        );
    }

    #[test]
    fn test_template_string() {
        assert_eq!(
            tok_kinds(r#"`Hello ${name}`"#),
            vec![TemplateLiteral("Hello ${name}".into())]
        );
    }

    #[test]
    fn test_template_string_rc3_escape_sequences() {
        assert_eq!(
            tok_kinds(r#"`\0\x41\u{1F600}`"#),
            vec![TemplateLiteral("\0A😀".into())]
        );
    }

    #[test]
    fn test_bool_literals() {
        assert_eq!(
            tok_kinds("true false"),
            vec![BoolLiteral(true), BoolLiteral(false)]
        );
    }

    #[test]
    fn test_gorilla_literal() {
        assert_eq!(tok_kinds("><"), vec![Gorilla]);
    }

    // ── Identifiers ──────────────────────────────────────────

    #[test]
    fn test_identifier() {
        assert_eq!(tok_kinds("user_name"), vec![Ident("user_name".into())]);
    }

    #[test]
    fn test_placeholder() {
        assert_eq!(tok_kinds("_"), vec![Placeholder]);
    }

    #[test]
    fn test_type_name() {
        assert_eq!(tok_kinds("Person"), vec![Ident("Person".into())]);
    }

    // ── Delimiters ───────────────────────────────────────────

    #[test]
    fn test_delimiters() {
        assert_eq!(
            tok_kinds("@( ) [ ] , . :"),
            vec![At, LParen, RParen, LBracket, RBracket, Comma, Dot, Colon]
        );
    }

    // ── Comments ─────────────────────────────────────────────

    #[test]
    fn test_line_comment() {
        let (tokens, errors) = tokenize("// this is a comment\nx <= 42");
        assert!(errors.is_empty());
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert!(kinds.contains(&&LineComment("this is a comment".into())));
    }

    #[test]
    fn test_block_comment() {
        let (tokens, errors) = tokenize("/* block */ x");
        assert!(errors.is_empty());
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert!(kinds.contains(&&BlockComment(" block ".into())));
    }

    #[test]
    fn test_nested_block_comment() {
        assert_eq!(
            tok_kinds("/* outer /* inner */ outer-end */ x <= 1"),
            vec![Ident("x".into()), LtEq, IntLiteral(1)],
            "Nested block comment should be skipped cleanly"
        );
    }

    #[test]
    fn test_unterminated_nested_block_comment_error() {
        let (_, errors) = tokenize("/* outer /* inner */");
        assert!(!errors.is_empty(), "Expected unterminated comment error");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unterminated block comment")),
            "Expected unterminated block comment error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_doc_comment() {
        let (tokens, errors) = tokenize("///@ Purpose: test");
        assert!(errors.is_empty());
        let kinds: Vec<_> = tokens.iter().map(|t| &t.kind).collect();
        assert!(kinds.contains(&&DocComment("Purpose: test".into())));
    }

    // ── Indentation ──────────────────────────────────────────

    #[test]
    fn test_indentation() {
        let source = "add x y =\n  x + y\n=> :Int";
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty());
        // Check that indent tokens are present
        let indent_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, Indent(_)))
            .collect();
        assert!(indent_tokens.len() >= 2);
        // First line should have indent 0
        assert_eq!(indent_tokens[0].kind, Indent(0));
        // Second line should have indent 2
        assert_eq!(indent_tokens[1].kind, Indent(2));
    }

    #[test]
    fn test_tab_error() {
        let (_, errors) = tokenize("\tx <= 42");
        assert!(!errors.is_empty());
        assert!(errors[0].message.contains("Tab"));
    }

    #[test]
    fn test_tab_after_spaces_at_line_start_error() {
        let (_, errors) = tokenize("  \tx <= 42");
        assert!(!errors.is_empty(), "Expected tab indentation error");
        assert!(
            errors[0].message.contains("Tab"),
            "Expected tab-related error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_tab_in_nested_block_error() {
        let source = "add x y =\n  x + y\n\tz <= 1\n=> :Int";
        let (_, errors) = tokenize(source);
        assert!(!errors.is_empty(), "Expected tab indentation error");
        assert!(
            errors.iter().any(|e| e.message.contains("Tab")),
            "Expected tab-related error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_vertical_tab_control_char_error() {
        let (_, errors) = tokenize("x <= 1\u{000b}y <= 2");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character")),
            "Expected unexpected-character error for vertical tab, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_form_feed_control_char_error() {
        let (_, errors) = tokenize("x <= 1\u{000c}y <= 2");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character")),
            "Expected unexpected-character error for form-feed, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_null_byte_control_char_error() {
        let (_, errors) = tokenize("x <= 1\0y <= 2");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character")),
            "Expected unexpected-character error for NUL byte, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_non_breaking_space_error() {
        let (_, errors) = tokenize("x <= 1\u{00a0}y <= 2");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character")),
            "Expected unexpected-character error for non-breaking space, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_zero_width_space_error() {
        let (_, errors) = tokenize("x <= 1\u{200b}y <= 2");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character")),
            "Expected unexpected-character error for zero-width space, got: {:?}",
            errors
        );
    }

    // ── Complex expressions ──────────────────────────────────

    #[test]
    fn test_buchi_pack() {
        assert_eq!(
            tok_kinds("@(name <= \"Alice\", age <= 30)"),
            vec![
                At,
                LParen,
                Ident("name".into()),
                LtEq,
                StringLiteral("Alice".into()),
                Comma,
                Ident("age".into()),
                LtEq,
                IntLiteral(30),
                RParen
            ]
        );
    }

    #[test]
    fn test_list_literal() {
        assert_eq!(
            tok_kinds("@[1, 2, 3]"),
            vec![
                At,
                LBracket,
                IntLiteral(1),
                Comma,
                IntLiteral(2),
                Comma,
                IntLiteral(3),
                RBracket
            ]
        );
    }

    #[test]
    fn test_mold_definition() {
        // Mold[T] => Optional[T] = @(hasValue: Bool)
        assert_eq!(
            tok_kinds("Mold[T] => Optional[T] = @(hasValue: Bool)"),
            vec![
                Ident("Mold".into()),
                LBracket,
                Ident("T".into()),
                RBracket,
                FatArrow,
                Ident("Optional".into()),
                LBracket,
                Ident("T".into()),
                RBracket,
                Eq,
                At,
                LParen,
                Ident("hasValue".into()),
                Colon,
                Ident("Bool".into()),
                RParen
            ]
        );
    }

    #[test]
    fn test_function_definition() {
        // add x: Int y: Int =\n  x + y\n=> :Int
        let kinds = tok_kinds("add x: Int y: Int =");
        assert_eq!(
            kinds,
            vec![
                Ident("add".into()),
                Ident("x".into()),
                Colon,
                Ident("Int".into()),
                Ident("y".into()),
                Colon,
                Ident("Int".into()),
                Eq
            ]
        );
    }

    #[test]
    fn test_return_type() {
        // => :Int
        assert_eq!(
            tok_kinds("=> :Int"),
            vec![FatArrow, Colon, Ident("Int".into())]
        );
    }

    #[test]
    fn test_pipeline() {
        // 5 => add(3, _) => result
        assert_eq!(
            tok_kinds("5 => add(3, _) => result"),
            vec![
                IntLiteral(5),
                FatArrow,
                Ident("add".into()),
                LParen,
                IntLiteral(3),
                Comma,
                Placeholder,
                RParen,
                FatArrow,
                Ident("result".into())
            ]
        );
    }

    #[test]
    fn test_map_operation() {
        // Map[numbers, _ x = x * 2]() ]=> doubled
        assert_eq!(
            tok_kinds("Map[numbers, _ x = x * 2]() ]=> doubled"),
            vec![
                Ident("Map".into()),
                LBracket,
                Ident("numbers".into()),
                Comma,
                Placeholder,
                Ident("x".into()),
                Eq,
                Ident("x".into()),
                Star,
                IntLiteral(2),
                RBracket,
                LParen,
                RParen,
                UnmoldForward,
                Ident("doubled".into())
            ]
        );
    }

    #[test]
    fn test_error_handling_pattern() {
        // |== error: Error =
        assert_eq!(
            tok_kinds("|== error: Error ="),
            vec![
                ErrorCeiling,
                Ident("error".into()),
                Colon,
                Ident("Error".into()),
                Eq
            ]
        );
    }

    #[test]
    fn test_condition_branch() {
        // | score >= 90 |> "A"
        assert_eq!(
            tok_kinds("| score >= 90 |> \"A\""),
            vec![
                Pipe,
                Ident("score".into()),
                GtEq,
                IntLiteral(90),
                PipeGt,
                StringLiteral("A".into())
            ]
        );
    }

    #[test]
    fn test_import_statement() {
        // >>> ./utils.td => @(helper)
        assert_eq!(
            tok_kinds(">>> ./utils.td => @(helper)"),
            vec![
                Import,
                Dot,
                Slash,
                Ident("utils".into()),
                Dot,
                Ident("td".into()),
                FatArrow,
                At,
                LParen,
                Ident("helper".into()),
                RParen
            ]
        );
    }

    #[test]
    fn test_export_statement() {
        // <<< @(add, subtract)
        assert_eq!(
            tok_kinds("<<< @(add, subtract)"),
            vec![
                Export,
                At,
                LParen,
                Ident("add".into()),
                Comma,
                Ident("subtract".into()),
                RParen
            ]
        );
    }

    #[test]
    fn test_method_call() {
        // "hello".toUpperCase()
        assert_eq!(
            tok_kinds("\"hello\".toUpperCase()"),
            vec![
                StringLiteral("hello".into()),
                Dot,
                Ident("toUpperCase".into()),
                LParen,
                RParen
            ]
        );
    }

    #[test]
    fn test_type_definition() {
        // Person = @(\n  name: Str\n  age: Int\n)
        let kinds = tok_kinds("Person = @(name: Str, age: Int)");
        assert_eq!(
            kinds,
            vec![
                Ident("Person".into()),
                Eq,
                At,
                LParen,
                Ident("name".into()),
                Colon,
                Ident("Str".into()),
                Comma,
                Ident("age".into()),
                Colon,
                Ident("Int".into()),
                RParen
            ]
        );
    }

    #[test]
    fn test_error_throw_pattern() {
        // ValidationError(type <= "ValidationError", message <= "Invalid").throw()
        let kinds = tok_kinds(
            "ValidationError(type <= \"ValidationError\", message <= \"Invalid\").throw()",
        );
        assert_eq!(
            kinds,
            vec![
                Ident("ValidationError".into()),
                LParen,
                Ident("type".into()),
                LtEq,
                StringLiteral("ValidationError".into()),
                Comma,
                Ident("message".into()),
                LtEq,
                StringLiteral("Invalid".into()),
                RParen,
                Dot,
                Ident("throw".into()),
                LParen,
                RParen
            ]
        );
    }

    #[test]
    fn test_negative_number_as_minus_then_int() {
        // Negative numbers: the lexer produces Minus + IntLiteral
        // (unary minus is handled at the parser level)
        assert_eq!(tok_kinds("-10"), vec![Minus, IntLiteral(10)]);
    }

    #[test]
    fn test_inheritance_syntax() {
        // Person => Employee = @(department: Str)
        assert_eq!(
            tok_kinds("Person => Employee = @(department: Str)"),
            vec![
                Ident("Person".into()),
                FatArrow,
                Ident("Employee".into()),
                Eq,
                At,
                LParen,
                Ident("department".into()),
                Colon,
                Ident("Str".into()),
                RParen
            ]
        );
    }

    #[test]
    fn test_error_inheritance_syntax() {
        // Error => ValidationError = @(field: Str)
        assert_eq!(
            tok_kinds("Error => ValidationError = @(field: Str)"),
            vec![
                Ident("Error".into()),
                FatArrow,
                Ident("ValidationError".into()),
                Eq,
                At,
                LParen,
                Ident("field".into()),
                Colon,
                Ident("Str".into()),
                RParen
            ]
        );
    }

    #[test]
    fn test_line_continuation() {
        assert_eq!(tok_kinds("x \\"), vec![Ident("x".into()), Backslash]);
    }

    #[test]
    fn test_async_unmold_await() {
        // response ]=> data
        assert_eq!(
            tok_kinds("response ]=> data"),
            vec![
                Ident("response".into()),
                UnmoldForward,
                Ident("data".into())
            ]
        );
    }

    #[test]
    fn test_default_case_placeholder() {
        // | _ |> "default"
        assert_eq!(
            tok_kinds("| _ |> \"default\""),
            vec![Pipe, Placeholder, PipeGt, StringLiteral("default".into())]
        );
    }

    #[test]
    fn test_multiline_program() {
        let source = r#"Person = @(
  name: Str
  age: Int
)

alice <= Person(name <= "Alice", age <= 30)
"#;
        let (tokens, errors) = tokenize(source);
        assert!(errors.is_empty(), "Unexpected errors: {:?}", errors);
        // Just verify it tokenizes without errors
        assert!(tokens.len() > 10);
    }

    // ── N-10 / N-11: Escape sequence error reporting ────────

    #[test]
    fn test_invalid_escape_in_string_reports_error() {
        let (tokens, errors) = tokenize(r#""hello \q world""#);
        // Error is reported for the invalid escape
        assert!(
            !errors.is_empty(),
            "Expected error for invalid escape \\q in string"
        );
        assert!(errors[0].message.contains("Invalid escape sequence"));
        // Token is still emitted for error recovery (literal char without backslash)
        let string_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, StringLiteral(_)))
            .collect();
        assert_eq!(string_tokens.len(), 1);
        if let StringLiteral(ref val) = string_tokens[0].kind {
            assert!(
                val.contains("\\q"),
                "Recovery should keep the literal escape sequence"
            );
        }
    }

    #[test]
    fn test_invalid_hex_escape_in_string_reports_error() {
        let (tokens, errors) = tokenize(r#""bad \x4 end""#);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Invalid hex escape sequence")),
            "Expected invalid hex escape error, got: {:?}",
            errors
        );
        let string_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, StringLiteral(_)))
            .collect();
        assert_eq!(string_tokens.len(), 1);
        if let StringLiteral(ref val) = string_tokens[0].kind {
            assert!(val.contains("\\x4"));
        }
    }

    #[test]
    fn test_invalid_unicode_escape_in_string_reports_error() {
        let (_tokens, errors) = tokenize(r#""bad \u{} end""#);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("empty \\u{} is not allowed")),
            "Expected empty unicode escape error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_unicode_escape_out_of_range_reports_error() {
        let (_tokens, errors) = tokenize(r#""bad \u{110000} end""#);
        assert!(
            errors.iter().any(|e| e.message.contains("<= U+10FFFF")),
            "Expected out-of-range unicode escape error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_unicode_escape_surrogate_reports_error() {
        let (_tokens, errors) = tokenize(r#""bad \u{D800} end""#);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("surrogate code points")),
            "Expected surrogate unicode escape error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_unsupported_octal_escape_reports_error() {
        let (tokens, errors) = tokenize(r#""bad \033 end""#);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unsupported octal escape sequence")),
            "Expected unsupported octal escape error, got: {:?}",
            errors
        );
        let string_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, StringLiteral(_)))
            .collect();
        assert_eq!(string_tokens.len(), 1);
        if let StringLiteral(ref val) = string_tokens[0].kind {
            assert!(val.contains("\\033"));
        }
    }

    #[test]
    fn test_unsupported_fixed_width_unicode_escape_reports_error() {
        let (tokens, errors) = tokenize(r#""bad \u0041 end""#);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("use \\u{HEX+}, not \\uXXXX")),
            "Expected unsupported fixed-width unicode escape error, got: {:?}",
            errors
        );
        let string_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, StringLiteral(_)))
            .collect();
        assert_eq!(string_tokens.len(), 1);
        if let StringLiteral(ref val) = string_tokens[0].kind {
            assert!(val.contains("\\u0041"));
        }
    }

    // ── BT-1: 10-operator rule negative tests ──────────────────
    // PHILOSOPHY.md: "演算子は10種のみ"
    // These tests verify that invalid operators are properly rejected.

    #[test]
    fn test_bt1_caret_rejected() {
        let (_, errors) = tokenize("x <= 1 ^ 2");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character '^'")),
            "Caret '^' should be rejected as unexpected character, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_bt1_single_ampersand_rejected() {
        let (_, errors) = tokenize("x <= 1 & 2");
        assert!(!errors.is_empty(), "Single '&' should produce an error");
        assert!(
            errors[0].message.contains("&"),
            "Error should mention '&', got: {}",
            errors[0].message
        );
    }

    #[test]
    fn test_bt1_tilde_rejected() {
        let (_, errors) = tokenize("x <= ~1");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character '~'")),
            "Tilde '~' should be rejected as unexpected character, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_bt1_question_mark_rejected() {
        let (_, errors) = tokenize("x <= y ? 1");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character '?'")),
            "Question mark '?' should be rejected as unexpected character, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_bt1_hash_rejected() {
        let (_, errors) = tokenize("x <= #tag");
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("Unexpected character '#'")),
            "Hash '#' should be rejected as unexpected character, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_bt1_slash_tokenized_as_slash() {
        // `/` is tokenized at lexer level (for comments: //, /* */)
        // but rejected at parser level as division operator.
        // At lexer level, standalone `/` should produce a Slash token (no error).
        let kinds = tok_kinds("x / y");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), Slash, Ident("y".into())],
            "Standalone '/' should tokenize as Slash (rejected at parser level)"
        );
    }

    #[test]
    fn test_bt1_percent_tokenized_as_percent() {
        // `%` is tokenized at lexer level but rejected at parser level as modulo.
        let kinds = tok_kinds("x % y");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), Percent, Ident("y".into())],
            "Standalone '%' should tokenize as Percent (rejected at parser level)"
        );
    }

    // ── BT-1b: Operator partial match / boundary tests ───────────
    // Verify that operator-like sequences don't produce wrong tokens.

    #[test]
    fn test_bt1_fat_arrow_eq_eq_boundary() {
        // `=>==` should tokenize as `=>` `==`, not as some merged operator
        let kinds = tok_kinds("x =>== y");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), FatArrow, EqEq, Ident("y".into())],
            "'=>==' should split into '=>' + '=='"
        );
    }

    #[test]
    fn test_bt1_lt_eq_eq_eq_gt_boundary() {
        // `<===>`  should tokenize as `<=` `==` `>`
        let kinds = tok_kinds("x <===> y");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), LtEq, EqEq, Gt, Ident("y".into())],
            "'<===>' should split into '<=' + '==' + '>'"
        );
    }

    #[test]
    fn test_bt1_pipe_eq_gt_boundary() {
        // `|=>` should tokenize as `|` `=>`, not as some combined operator
        let kinds = tok_kinds("x |=> y");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), Pipe, FatArrow, Ident("y".into())],
            "'|=>' should split into '|' + '=>'"
        );
    }

    #[test]
    fn test_bt1_double_gt_not_import() {
        // `>>` (two greater-than) should NOT be tokenized as Import (`>>>`)
        let kinds = tok_kinds("x >> y");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), Gt, Gt, Ident("y".into())],
            "'>>' should be two Gt tokens, not Import"
        );
    }

    #[test]
    fn test_bt1_double_lt_not_export() {
        // `<<` should NOT be tokenized as Export (`<<<`)
        let kinds = tok_kinds("x << y");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), Lt, Lt, Ident("y".into())],
            "'<<' should be two Lt tokens, not Export"
        );
    }

    // ── BT-2: null/undefined rejection tests ───────────────────
    // PHILOSOPHY.md I: "null/undefinedの完全排除 — 全ての型にデフォルト値を保証"
    // These words must not be keywords or special tokens — they should be
    // plain identifiers that will be rejected by the type checker as undefined.

    #[test]
    fn test_bt2_null_is_plain_identifier() {
        let kinds = tok_kinds("x <= null");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), LtEq, Ident("null".into())],
            "'null' should be a plain identifier, not a keyword/special token"
        );
    }

    #[test]
    fn test_bt2_undefined_is_plain_identifier() {
        let kinds = tok_kinds("x <= undefined");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), LtEq, Ident("undefined".into())],
            "'undefined' should be a plain identifier, not a keyword/special token"
        );
    }

    #[test]
    fn test_bt2_none_is_plain_identifier() {
        let kinds = tok_kinds("x <= none");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), LtEq, Ident("none".into())],
            "'none' should be a plain identifier, not a keyword/special token"
        );
    }

    #[test]
    fn test_bt2_nil_is_plain_identifier() {
        let kinds = tok_kinds("x <= nil");
        assert_eq!(
            kinds,
            vec![Ident("x".into()), LtEq, Ident("nil".into())],
            "'nil' should be a plain identifier, not a keyword/special token"
        );
    }

    #[test]
    fn test_invalid_escape_in_template_reports_error() {
        let (tokens, errors) = tokenize(r#"`template \q text`"#);
        assert!(
            !errors.is_empty(),
            "Expected error for invalid escape \\q in template string"
        );
        assert!(
            errors[0]
                .message
                .contains("Invalid escape sequence in template string")
        );
        // Token is still emitted for error recovery
        let tmpl_tokens: Vec<_> = tokens
            .iter()
            .filter(|t| matches!(t.kind, TemplateLiteral(_)))
            .collect();
        assert_eq!(tmpl_tokens.len(), 1);
        if let TemplateLiteral(ref val) = tmpl_tokens[0].kind {
            assert!(
                val.contains("\\q"),
                "Template recovery keeps backslash + char"
            );
        }
    }

    #[test]
    fn test_invalid_hex_escape_in_template_reports_error() {
        let (_tokens, errors) = tokenize(r#"`bad \xG1 end`"#);
        assert!(
            errors.iter().any(|e| e
                .message
                .contains("Invalid hex escape sequence in template string")),
            "Expected invalid hex escape in template error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_invalid_unicode_escape_in_template_reports_error() {
        let (_tokens, errors) = tokenize(r#"`bad \u{} end`"#);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("empty \\u{} is not allowed")),
            "Expected invalid unicode escape in template error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_unsupported_octal_escape_in_template_reports_error() {
        let (_tokens, errors) = tokenize(r#"`bad \033 end`"#);
        assert!(
            errors.iter().any(|e| e
                .message
                .contains("Unsupported octal escape sequence in template string")),
            "Expected unsupported octal template escape error, got: {:?}",
            errors
        );
    }

    #[test]
    fn test_unsupported_fixed_width_unicode_escape_in_template_reports_error() {
        let (_tokens, errors) = tokenize(r#"`bad \u0041 end`"#);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("use \\u{HEX+}, not \\uXXXX")),
            "Expected unsupported fixed-width unicode template error, got: {:?}",
            errors
        );
    }

    // ── BT-16: Unicode boundary tests ──

    #[test]
    fn test_bidi_override_in_identifier() {
        // U+202E (RIGHT-TO-LEFT OVERRIDE) embedded in identifier should not silently pass
        let source = "x\u{202E}y <= 1";
        let (tokens, errors) = tokenize(source);
        // BiDi control chars are not alphabetic/alphanumeric, so should split
        // the identifier or cause an error token
        let idents: Vec<&Token> = tokens
            .iter()
            .filter(|t| matches!(t.kind, Ident(_)))
            .collect();
        // The identifier should NOT be "x\u{202E}y" as a single token
        let has_bidi_ident = idents.iter().any(|t| {
            if let Ident(ref name) = t.kind {
                name.contains('\u{202E}')
            } else {
                false
            }
        });
        assert!(
            !has_bidi_ident || !errors.is_empty(),
            "BiDi control character U+202E should not silently appear in identifier. Tokens: {:?}, Errors: {:?}",
            idents,
            errors
        );
    }

    #[test]
    fn test_bidi_lri_in_string_literal() {
        // U+2066 (LEFT-TO-RIGHT ISOLATE) in string should be preserved
        let source = "s <= \"hello\u{2066}world\"";
        let (tokens, _errors) = tokenize(source);
        let str_tokens: Vec<&Token> = tokens
            .iter()
            .filter(|t| matches!(t.kind, StringLiteral(_)))
            .collect();
        assert_eq!(str_tokens.len(), 1, "Should have one string literal");
        if let StringLiteral(ref val) = str_tokens[0].kind {
            assert!(
                val.contains('\u{2066}'),
                "BiDi character inside string literal should be preserved"
            );
        }
    }

    #[test]
    fn test_combining_character_in_identifier() {
        // e + U+0301 (COMBINING ACUTE ACCENT) = e followed by combining mark
        // Combining marks are not alphabetic, so behavior depends on is_alphanumeric()
        let source = "e\u{0301} <= 1";
        let (tokens, _errors) = tokenize(source);
        let idents: Vec<String> = tokens
            .iter()
            .filter_map(|t| {
                if let Ident(ref name) = t.kind {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect();
        // The combining mark U+0301 should either be included in the identifier
        // (if is_alphanumeric returns true for it) or cause a split.
        // In Rust, char::is_alphanumeric returns false for U+0301 (Mn category),
        // but char::is_alphabetic also returns false, so it should NOT start an ident.
        // However, U+0301 follows 'e' which starts the ident, and is_alphanumeric is
        // used for continuation chars. U+0301 is Mn (Nonspacing_Mark), is_alphanumeric() = false.
        // So the identifier should be just "e", and U+0301 becomes an error/unknown token.
        assert!(
            idents.contains(&"e".to_string()),
            "Identifier 'e' should be recognized, idents found: {:?}",
            idents
        );
    }

    #[test]
    fn test_japanese_identifier() {
        // Japanese characters are alphabetic and should work as identifiers
        let source = "\u{540D}\u{524D} <= 42";
        let kinds = tok_kinds(source);
        assert_eq!(
            kinds,
            vec![Ident("\u{540D}\u{524D}".into()), LtEq, IntLiteral(42)],
            "Japanese characters should be valid identifiers"
        );
    }

    #[test]
    fn test_emoji_in_identifier_rejected() {
        // Emojis (e.g. U+1F600) are not alphabetic in Rust's char::is_alphabetic
        // They should not form valid identifiers
        let source = "\u{1F600} <= 1";
        let (_tokens, errors) = tokenize(source);
        // Emoji is not alphabetic, not a known operator, so should produce an error
        assert!(
            !errors.is_empty(),
            "Emoji should not be accepted as identifier start"
        );
    }
}
