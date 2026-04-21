// D-1a: docs/guide/00_overview.md code examples

// Hello World
greet name: Str =
  "Hello, " + name + "!"
=> :Str

message <= greet("World")
stdout(message)

// Variables
x <= 42
name2 <= "Asuka"
active <= true

// Type definition
Pilot = @(
  name: Str
  age: Int
  active: Bool
)

asuka <= Pilot(name <= "Asuka", age <= 14, active <= true)

// Function definition
add x: Int y: Int =
  x + y
=> :Int

result <= add(3, 5)
stdout(result.toString())

// Condition
// C20-5: multi-line rhs multi-arm guard is rejected with [E0303]; wrap in parens.
score <= 85
grade <= (
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | score >= 70 |> "C"
  | _ |> "D"
)
stdout(grade)

// Pipeline (using step-by-step, not pipeline syntax)
data <= @[1, 2, 3, 4, 5]
Filter[data, _ x2 = x2 > 0]() ]=> filtered
Map[filtered, _ x2 = x2 * 2]() ]=> pipeResult
stdout(jsonEncode(pipeResult))

// Default values
NervStaff = @(name: Str, callSign: Str, age: Int)
ritchan <= NervStaff(name <= "Ritsuko")
stdout(ritchan.callSign)
stdout(ritchan.age.toString())
