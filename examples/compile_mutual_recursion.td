// Taida Lang Example: Mutual Recursion with Tail Call Optimization
// Two or more functions calling each other in tail position are optimized
// to avoid stack overflow, just like self-recursive tail calls.

// Classic isEven/isOdd mutual recursion (returns 1/0 for cross-backend parity)
isEven n: Int =
  | n == 0 |> 1
  | _ |> isOdd(n - 1)
=> :Int

isOdd n: Int =
  | n == 0 |> 0
  | _ |> isEven(n - 1)
=> :Int

stdout(isEven(0))
stdout(isEven(1))
stdout(isEven(4))
stdout(isOdd(3))
stdout(isOdd(4))

// Deep mutual recursion (would stack overflow without TCO)
stdout(isEven(100000))
