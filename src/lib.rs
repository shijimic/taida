/// RC1 -- Native addon foundation host bindings.
///
/// `addon::backend_policy` is always available so non-Native backends
/// can produce the deterministic unsupported-backend diagnostic.
/// `addon::loader` is gated on `feature = "native"` because it depends
/// on `libloading` (dlopen / LoadLibrary).
pub mod addon;
#[cfg(feature = "community")]
pub mod auth;
#[cfg(feature = "native")]
pub mod codegen;
#[cfg(feature = "community")]
pub mod community;
/// SHA-256 digest (hand-written, no external crate).
pub mod crypto;
pub mod doc;
pub mod graph;
pub mod interpreter;
pub mod js;
pub mod lexer;
#[cfg(feature = "lsp")]
pub mod lsp;
pub mod module_graph;
pub mod net_surface;
pub mod parser;
pub mod pkg;
pub mod types;
#[cfg(feature = "community")]
pub mod upgrade;
pub mod util;
pub mod version;
