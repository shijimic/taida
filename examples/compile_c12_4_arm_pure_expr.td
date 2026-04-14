// C12-4 / FB-17: `| |>` arm body pure-expression discipline.
//
// A condition arm body is a sequence of let-bindings followed by a
// single final result expression. This file exercises the legal
// patterns across all three backends so that the wasm-* parity grids
// pick it up automatically. Illegal patterns (discarded pipelines,
// bare side-effect calls, definitions inside arm bodies) are covered
// by `test_c12_4_*_reject_parity` in tests/parity.rs.
//
// See docs/guide/07_control_flow.md for the full rule.

// Single-expression arm — the simplest form.
classifyScore score =
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | score >= 70 |> "C"
  | _ |> "F"
=> :Str

stdout(classifyScore(95))
stdout(classifyScore(85))
stdout(classifyScore(60))

// Let-binding followed by final expression (legal `<=` let inside arm).
computeBlock flag n =
  | flag |>
    doubled <= n * 2
    doubled + 1
  | _ |>
    incremented <= n + 1
    incremented * 2
=> :Int

stdout(computeBlock(true, 5).toString())
stdout(computeBlock(false, 5).toString())

// Nested condition branch as an arm body — a single expression (the
// inner `| |>`) remains legal. No let-binding needed.
deepClassify x =
  | x > 100 |>
    | x > 500 |> "big"
    | _ |> "medium"
  | _ |> "small"
=> :Str

stdout(deepClassify(1000))
stdout(deepClassify(200))
stdout(deepClassify(50))

// Expected output:
// A
// B
// F
// 11
// 12
// big
// medium
// small
