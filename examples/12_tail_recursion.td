// Taida Lang Example: Tail Recursion Optimization
// Tail calls are automatically detected and optimized (no stack overflow)

// Factorial with accumulator pattern (tail recursive)
factorialTail n: Int acc: Int =
  | n < 1 |> acc
  | _ |> factorialTail(n - 1, acc * n)
=> :Int

stdout(factorialTail(10, 1))

// Fibonacci with accumulator pattern (tail recursive)
fibTail n: Int a: Int b: Int =
  | n == 0 |> a
  | n == 1 |> b
  | _ |> fibTail(n - 1, b, a + b)
=> :Int

stdout(fibTail(20, 0, 1))

// Sum with tail recursion
sumTail n: Int acc: Int =
  | n == 0 |> acc
  | _ |> sumTail(n - 1, acc + n)
=> :Int

stdout(sumTail(100, 0))

// Deep recursion that would stack overflow without TCO
countdown n: Int =
  | n == 0 |> 0
  | _ |> countdown(n - 1)
=> :Int

stdout(countdown(100000))
