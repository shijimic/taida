//! C24 Phase 5 (RC-SLOW-2 / C24B-006): Generated fixture lists.
//!
//! This module re-exports the `pub const` fixture lists produced by
//! `build.rs` so integration tests can declare one `#[test]` per fixture.
//!
//! The lists are regenerated on every `cargo build` when the enumerated
//! directories (`examples/`, `examples/quality/`) change.

// Lists are generated into $OUT_DIR/*.rs and included here.

include!(concat!(env!("OUT_DIR"), "/examples_all_td_fixtures.rs"));
include!(concat!(env!("OUT_DIR"), "/examples_compile_td_fixtures.rs"));
include!(concat!(
    env!("OUT_DIR"),
    "/examples_numbered_td_fixtures.rs"
));
include!(concat!(
    env!("OUT_DIR"),
    "/quality_cross_module_fixtures.rs"
));
