// Pi approximation using Leibniz formula with fixed-point integer arithmetic.
// scale = 10^18. Result: pi * 10^9 (truncated).

scale <= 1000000000000000000

piStep sum k sign i n =
  | i == n |> sum
  | _ |>
    Div[scale, k]() ]=> term
    | sign == 1 |> piStep(sum + term, k + 2, 0 - 1, i + 1, n)
    | _ |> piStep(sum - term, k + 2, 1, i + 1, n)
=> :Int

result <= piStep(0, 1, 1, 0, 1000000)

Div[result * 4, 1000000000]() ]=> pi_billion

debug(pi_billion)
