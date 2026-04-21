// C21-1 / seed-06: Float 関数跨ぎ parity test 基盤 — minimal case
//
// 関数から返した Float を stdout に直接渡すと、interpreter 以外の backend で
// 値表現が崩れる問題 (seed-01/03/05) の regression guard。
//
// 期待: 4 backend (Interpreter / JS / Native / WASM-wasi) 全てで `12.0` を出力
// 現状 (Phase 1 land 時点の snapshot, Phase 2-5 で順次解消):
//   - Interpreter: 12.0 (リファレンス, 正解)
//   - JS:         12   (Number 型で .0 が落ちる; seed-04 近縁 — Phase 5)
//   - Native:     Verifier errors で compile 失敗 (新ブロッカー C21B-xxx)
//   - WASM-wasi:  4622945017495814144 (double の i64 bit pattern; seed-03 — Phase 2/4)
triple x: Float =
  x * 3.0
=> :Float

stdout(triple(4.0))
