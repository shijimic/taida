// B-8c helper: export function with pattern match
classify n: Int =
  | n > 100 |> "big"
  | n > 10 |> "medium"
  | _ |> "small"
=> :Str

<<< @(classify)
