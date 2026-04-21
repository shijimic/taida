// === Taida Lang: Condition Branches ===

score <= 85

// C20-1 (ROOT-5): multi-line `| |>` guards on the right-hand side of
// `<=` now require the parenthesised escape hatch. The parens make
// the branch's boundary unambiguous so subsequent top-level
// statements cannot be silently absorbed into the last arm.
grade <= (
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | score >= 70 |> "C"
  | score >= 60 |> "D"
  | _ |> "F"
)

stdout(`Score: ${score}, Grade: ${grade}`)

// FizzBuzz
fizzbuzz n =
  | Mod[n, 15]().unmold() == 0 |> "FizzBuzz"
  | Mod[n, 3]().unmold() == 0 |> "Fizz"
  | Mod[n, 5]().unmold() == 0 |> "Buzz"
  | _ |> n.toString()
=> :Str

i <= 1
stdout(fizzbuzz(1))
stdout(fizzbuzz(2))
stdout(fizzbuzz(3))
stdout(fizzbuzz(4))
stdout(fizzbuzz(5))
stdout(fizzbuzz(6))
stdout(fizzbuzz(9))
stdout(fizzbuzz(10))
stdout(fizzbuzz(15))
stdout(fizzbuzz(20))
stdout(fizzbuzz(30))
