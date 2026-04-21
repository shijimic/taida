// D-1h: docs/guide/07_control_flow.md code examples

// Basic condition
// C20-5: multi-line rhs multi-arm guard is rejected with [E0303]; wrap in parens.
score <= 85
grade <= (
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | score >= 70 |> "C"
  | score >= 60 |> "D"
  | _ |> "F"
)
stdout(grade)

// 1-line condition
x <= 5
sign <= | x >= 0 |> "positive" | _ |> "negative"
stdout(sign)

// Pattern match on value
status <= "success"
message <= (
  | status == "success" |> "Operation completed"
  | status == "error" |> "Operation failed"
  | status == "pending" |> "Operation in progress"
  | _ |> "Unknown status"
)
stdout(message)

// BuchiPack field matching
response <= @(code <= 200, data <= "OK")
result <= (
  | response.code == 200 |> "Success: " + response.data
  | response.code == 404 |> "Not found"
  | response.code >= 500 |> "Server error"
  | _ |> "Unknown response"
)
stdout(result)

// Nested conditions
staff <= @(age <= 29, role <= "commander", active <= true)
accessLevel <= (
  | staff.role == "commander" |>
    | staff.active |> "full"
    | _ |> "readonly"
  | staff.role == "operator" |>
    | staff.age >= 18 |> "standard"
    | _ |> "limited"
  | _ |> "none"
)
stdout(accessLevel)

// Tail recursion: sum
sumToTail n: Int acc: Int =
  | n < 1 |> acc
  | _ |> sumToTail(n - 1, acc + n)
=> :Int

sumTo n: Int =
  sumToTail(n, 0)
=> :Int

result2 <= sumTo(100)
stdout(result2.toString())

// Factorial
factorialTail n: Int acc: Int =
  | n < 2 |> acc
  | _ |> factorialTail(n - 1, acc * n)
=> :Int

factorial n: Int =
  factorialTail(n, 1)
=> :Int

stdout(factorial(10).toString())

// Fibonacci
fibTail n: Int a: Int b: Int =
  | n == 0 |> a
  | n == 1 |> b
  | _ |> fibTail(n - 1, b, a + b)
=> :Int

fibonacci n: Int =
  fibTail(n, 0, 1)
=> :Int

stdout(fibonacci(10).toString())

// Expression-based condition
active <= true
message2 <= "Status: " + (| active |> "ON" | _ |> "OFF")
stdout(message2)
