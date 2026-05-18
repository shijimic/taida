// B-8e helper: export recursive function
sumTo n: Int acc: Int =
  | n == 0 |> acc
  | _ |> sumTo(n - 1, acc + n)
=> :Int

<<< @(sumTo)
