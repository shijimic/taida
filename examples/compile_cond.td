x <= 5
// C20-1 (ROOT-5): parenthesise multi-line rhs guard.
result <= (
  | x > 3 |> 100
  | _ |> 0
)
debug(result)
