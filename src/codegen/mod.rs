pub mod driver;
pub mod edge_glue;
pub mod emit;
pub mod emit_wasm_c;
pub mod ir;
pub mod lifetime;
pub mod lower;
pub mod native_runtime;
pub mod rc_opt;
pub mod runtime;
pub mod runtime_core_wasm;
#[cfg(test)]
mod runtime_mirror;
pub(crate) mod tag_prop;
