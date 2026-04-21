// C21-1 / seed-06: 内積計算 — 関数跨ぎ + @[Float] 経由の Float 演算 parity guard
//
// 期待: 4 backend (Interpreter / JS / Native / WASM-wasi) 全てで `11.0` を出力
//       (1.0*3.0 + 2.0*4.0 = 11.0)
// これは seed-01 (Wasm Float boxed / hot loop) の最小再現ケースでもある。
dotProductAt a: @[Float] b: @[Float] i: Int n: Int acc: Float =
  | i == n |> acc
  | _ |>
      a.get(i) ]=> av
      b.get(i) ]=> bv
      dotProductAt(a, b, i + 1, n, acc + av * bv)
=> :Float

a <= @[1.0, 2.0]
b <= @[3.0, 4.0]
stdout(dotProductAt(a, b, 0, 2, 0.0))
