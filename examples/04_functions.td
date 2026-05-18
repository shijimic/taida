// === Taida Lang: Functions ===

// Basic function
add x: Int y: Int =
  x + y
=> :Int

result <= add(3, 5)
stdout(`3 + 5 = ${result}`)

// Multi-line function
greet name: Str greeting: Str =
  message <= `${greeting}, ${name}!`
  message
=> :Str

stdout(greet("Taida", "Hello"))

// Tail-recursive factorial
factorialLoop n: Int acc: Int =
  | n < 2 |> acc
  | _ |> factorialLoop(n - 1, acc * n)
=> :Int

factorial n: Int =
  factorialLoop(n, 1)
=> :Int

stdout(`5! = ${factorial(5)}`)
stdout(`10! = ${factorial(10)}`)

// Tail-recursive Fibonacci
fibLoop n: Int a: Int b: Int =
  | n == 0 |> a
  | n == 1 |> b
  | _ |> fibLoop(n - 1, b, a + b)
=> :Int

fib n: Int =
  fibLoop(n, 0, 1)
=> :Int

stdout(`fib(10) = ${fib(10)}`)
