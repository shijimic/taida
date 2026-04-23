#[cfg(feature = "native")]
mod addon_eval;
mod control_flow;
pub mod env;
pub mod eval;
pub(crate) mod json;
mod methods;
mod module_eval;
mod mold_eval;
mod net_eval;
mod net_h2;
#[allow(dead_code)] // Phase 3: protocol layer ready, QUIC transport gated
mod net_h3;
#[allow(dead_code)] // Phase 1 defines interfaces consumed in Phase 2+
mod net_transport;
mod os_eval;
mod prelude;
// C12 Phase 6 (FB-5): Regex value helpers shared between prelude
// constructor, Str method overloads, and checker-level type inference.
pub(crate) mod regex_eval;
#[cfg(test)]
mod tests_eval;
#[cfg(test)]
mod tests_extended;
mod unmold;
pub mod value;
/// C25B-021 / C25B-022 / C25B-023 common foundation (Phase 5-D):
/// hashable view over `Value` for HashSet / HashMap fast paths.
pub(crate) mod value_key;

pub use env::*;
pub use eval::*;
pub use value::*;
