// RC1 Phase 4 -- addon-backed package usage example.
//
// This example demonstrates how Taida user code calls a function
// implemented in a Rust addon (cdylib) without ever seeing a raw
// pointer or FFI detail.
//
// The addon-backed package layout (`.dev/RC1_DESIGN.md` Phase 4 Lock):
//
//   .taida/deps/taida-lang/addon-rs-sample/
//     native/
//       addon.toml              <-- ABI v1, function table, library stem
//       libtaida_addon_sample.so <-- the cdylib
//
// Native interpreter only: this example fails on JS / WASM / Cranelift
// native backends with a deterministic compile-time error
// (`addon-backed package '...' is not supported on backend '...'`).
//
// To run this example end-to-end:
//
//   1. cargo build -p taida-addon-sample
//   2. Set up the .taida/deps/ layout above (the integration test
//      `tests/addon_package_integration.rs` does this automatically).
//   3. taida examples/addon_echo.td
//
// Expected output:
//
//   hello from taida

>>> taida-lang/addon-rs-sample => @(echo)

result <= echo("hello from taida")
stdout(result)
