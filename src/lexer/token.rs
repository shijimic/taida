/// Source location tracking for error reporting and graph model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Char offset (Unicode scalar value index) of the start position.
    pub start: usize,
    /// Char offset (Unicode scalar value index) of the end position (exclusive).
    pub end: usize,
    /// 1-based line number.
    pub line: usize,
    /// 1-based column number.
    pub column: usize,
}

impl Span {
    pub fn new(start: usize, end: usize, line: usize, column: usize) -> Self {
        Self {
            start,
            end,
            line,
            column,
        }
    }
}

/// All token types in Taida Lang.
///
/// Taida has exactly 10 custom operators plus arithmetic/comparison/logical operators,
/// special literals, and structural tokens.
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

    // ── The 10 Taida Operators ───────────────────────────────
    /// `=` Definition
    Eq,
    /// `=>` Right buchi (pipe forward / assignment right)
    FatArrow,
    /// `<=` Left buchi (assignment left / pipe backward)
    LtEq,
    /// `]=>` Unmold forward
    UnmoldForward,
    /// `<=[` Unmold backward
    UnmoldBackward,
    /// `|==` Error ceiling
    ErrorCeiling,
    /// `|` Condition guard
    Pipe,
    /// `|>` Condition extract
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
    /// Single-line comment `// ...`
    LineComment(String),
    /// Multi-line comment `/* ... */`
    BlockComment(String),
    /// Documentation comment `///@ ...`
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
