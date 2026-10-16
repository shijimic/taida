// Addon-backed package usage example.
//
// This example demonstrates how Taida user code calls a function
// implemented in a Rust addon (cdylib) without ever seeing a raw
// pointer or FFI detail.
//
// The addon-backed package layout:
//
//   .taida/deps/taida-lang/addon-rs-sample/
//     native/
//       addon.toml              <-- function table, library stem
//       libtaida_addon_sample.so <-- the cdylib
//
// The interpreter and the native backend run it. The JS and WASM
// backends reject addon-backed packages at build time with a
// deterministic error.
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

result: Str <= echo("hello from taida")
stdout(result)
