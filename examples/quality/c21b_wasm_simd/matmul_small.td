// C21-3 / C21-6: WASM SIMD smoke — small Float hot loop that LLVM's wasm
// auto-vectorizer is expected to lower to `v128.*` / `f64x2.*` instructions
// once `-msimd128` is passed to clang (Phase 3).
//
// The assertion is two-fold:
//   * 4-backend parity on the scalar result (`sumSquaresAt` of 8 elements
//     = 1 + 4 + 9 + 16 + 25 + 36 + 49 + 64 = `204.0`)
//   * `tests/c21_wasm_simd.rs` separately disassembles the generated
//     `wasm-wasi` binary with `wasm-tools print` and asserts that at least
//     one SIMD-ish opcode (`v128.*` or `f32.*` or `f64x2.*`) is present
//     — that check is what confirms Phase 3 actually opened the path.
//
// `wasm-min` is intentionally left out of both checks: that profile keeps
// the no-SIMD compatibility door open, so SIMD is *not* expected there.

sumSquaresAt a: @[Float] i: Int n: Int acc: Float =
  | i == n |> acc
  | _ |>
      a.get(i) ]=> av
      sumSquaresAt(a, i + 1, n, acc + av * av)
=> :Float

a <= @[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0]
stdout(sumSquaresAt(a, 0, 8, 0.0))
