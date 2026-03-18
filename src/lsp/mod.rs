pub mod completion;
pub mod diagnostics;
pub(super) mod format;
pub mod hover;
/// LSP server for Taida Lang.
///
/// Provides language intelligence features:
/// - Diagnostics: parse errors + type errors (type mismatch, empty list annotation)
/// - Hover: variable types, function signatures, type/mold definitions, doc_comments
/// - Completion: variables, functions, types, molds, prelude functions, operators, dot-completion
pub mod server;
pub(super) mod utf16;
