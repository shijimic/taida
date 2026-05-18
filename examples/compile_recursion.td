// Tail-recursive factorial (optimized)
factorial n: Int acc: Int =
  | n == 1 |> acc
  | _ |> factorial(n - 1, acc * n)
=> :Int
debug(factorial(10, 1))

// Tail-recursive sum (TCO optimized — runs with 100000 depth)
sumTo n: Int acc: Int =
  | n == 0 |> acc
  | _ |> sumTo(n - 1, acc + n)
=> :Int
debug(sumTo(100000, 0))
