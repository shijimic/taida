// C12-3 / FB-8: Tail-only mutual recursion passes the new
// `mutual-recursion` compile-time check and produces identical output on
// all three backends. Non-tail mutual recursion is rejected with
// [E1614] — see the compile-fail parity test
// `test_c12_3_non_tail_mutual_recursion_reject_parity` and
// docs/reference/tail_recursion.md.

isEven n =
  | n == 0 |> 1
  | _ |> isOdd(n - 1)

isOdd n =
  | n == 0 |> 0
  | _ |> isEven(n - 1)

stdout(isEven(0))
stdout(isOdd(0))
stdout(isEven(4))
stdout(isOdd(5))
