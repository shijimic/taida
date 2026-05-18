/// Source location tracking for error reporting and graph model.
#[derive(Debug, Clone, Eq)]
pub struct Span {
    /// Char offset (Unicode scalar value index) of the start position.
    pub start: usize,
    /// Char offset (Unicode scalar value index) of the end position (exclusive).
    pub end: usize,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column number.
    pub column: usize,
    /// Parser-assigned expression node identity. `0` means this span
    /// is not attached to an expression node or has not been assigned yet.
    pub node_id: usize,
}

impl PartialEq for Span {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && self.end == other.end
            && self.line == other.line
            && self.column == other.column
    }
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, column: usize) -> Self {
        Self {
            start,
            end,
            line,
            column,
            node_id: 0,
        }
    }

    pub fn with_node_id(mut self, node_id: usize) -> Self {
        self.node_id = node_id;
        self
    }
}

/// All token types in Taida Lang.
///
/// Taida exposes exactly 10 semantic operators (per PHILOSOPHY.md /
/// `docs/reference/operators.md`): `=`, `=>`, `<=`, `>=>`, `<=<`,
/// `|==`, `(|... |>)` (condition delimiter pair), `>>>`, `<<<`,
/// and `:` (type marker). At the token level the condition pair is
/// emitted as two adjacent tokens (`Pipe` + `PipeGt`); they are
/// counted as one semantic operator.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── Literals ──────────────────────────────────────────────
    /// Integer literal: `42`, `-10`
    IntLiteral(i64),
    /// Float literal: `3.14`, `0.5`
    FloatLiteral(f64),
    /// String literal (single or double quoted): `"hello"`, `'world'`
    StringLiteral(String),
    /// Template string (backtick): `` `Hello ${name}` ``
    /// Stored as raw content between backticks (interpolation resolved later).
    TemplateLiteral(String),
    /// Boolean literal: `true`, `false`
    BoolLiteral(bool),
    /// Gorilla literal: `><`
    Gorilla,

    // ── Identifiers ──────────────────────────────────────────
    /// Identifier: variable names, function names, type names
    Ident(String),
    /// Placeholder: `_`
    Placeholder,

    // ── The 10 Taida Operators (tokens; `Pipe` + `PipeGt` together
    //    form the single semantic `(| ... |>)` delimiter pair) ──
    /// `=` Definition
    Eq,
    /// `=>` Right buchi (pipe forward / assignment right)
    FatArrow,
    /// `<=` Left buchi (assignment left / pipe backward)
    LtEq,
    /// `>=>` Unmold forward (renamed from legacy `]=>`).
    UnmoldForward,
    /// `<=<` Unmold backward (renamed from legacy `<=[`).
    UnmoldBackward,
    /// `|==` Error ceiling
    ErrorCeiling,
    /// `|` Condition guard (start of the `(|... |>)` delimiter pair).
    Pipe,
    /// `|>` Condition extract (end of the `(|... |>)` delimiter pair).
    PipeGt,
    /// `>>>` Import
    Import,
    /// `<<<` Export
    Export,

    // ── Arithmetic Operators ─────────────────────────────────
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%`
    Percent,

    // ── Comparison Operators ─────────────────────────────────
    /// `==`
    EqEq,
    /// `!=`
    BangEq,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `>=`
    GtEq,

    // ── Logical Operators ────────────────────────────────────
    /// `&&`
    And,
    /// `||`
    Or,
    /// `!`
    Bang,

    // ── Delimiters ───────────────────────────────────────────
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `@`
    At,
    /// `,`
    Comma,
    /// `.`
    Dot,
    /// `:`
    Colon,
    /// `\` (line continuation)
    Backslash,

    // ── Whitespace / Structure ────────────────────────────────
    /// Newline (significant for indentation-sensitive parsing)
    Newline,
    /// Indentation change (spaces at start of line). Value = number of spaces.
    Indent(usize),

    // ── Comments ─────────────────────────────────────────────
    /// Single-line comment `//...`
    LineComment(String),
    /// Multi-line comment `/*... */`
    BlockComment(String),
    /// Documentation comment `///@...`
    DocComment(String),

    // ── Special ──────────────────────────────────────────────
    /// End of file
    Eof,
}

/// A token with its kind and source location.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }
}
