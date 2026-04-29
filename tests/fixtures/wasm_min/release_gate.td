// W-2 regression test: --release gate should work with wasm-min.
// This file itself is valid wasm-min code, used to verify that
// `taida build wasm-min --release` succeeds or fails
// depending on the presence of release_gate markers.
stdout("release test")
