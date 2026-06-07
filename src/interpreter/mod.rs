mod abi_eval;
#[cfg(feature = "native")]
mod addon_eval;
mod control_flow;
pub mod env;
pub mod eval;
pub(crate) mod json;
mod methods;
mod module_eval;
mod mold_eval;
mod net;
mod os_eval;
mod prelude;
// D29B-016 / Phase 10-B: gap-buffer rope path for `Value::Str` mutation hot
// paths (Lock-K verdict V-1/V-2/V-3, transparent promotion at 1024-byte
// concat threshold).
pub(crate) mod runtime;
// C12 Phase 6 (FB-5): Regex value helpers shared between prelude
// constructor, Str method overloads, and checker-level type inference.
pub(crate) mod regex_eval;
#[cfg(test)]
mod tests_eval;
#[cfg(test)]
mod tests_extended;
mod unmold;
pub mod value;
/// / common foundation:
/// hashable view over `Value` for HashSet / HashMap fast paths.
pub(crate) mod value_key;

pub use env::*;
pub use eval::*;
pub use value::*;
