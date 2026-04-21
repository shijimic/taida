// D-2a: docs/reference/operators.md code examples

// = definition
Pilot = @(name: Str, age: Int)

// <= assignment
name <= "Asuka"
age <= 14
pilot <= Pilot(name <= "Asuka", age <= 14)

// => pipe forward
Str[42]() ]=> str_val
stdout(str_val)

// ]=> unmold
Div[10, 3]() ]=> div_val
stdout(div_val.toString())

// <=[ backward unmold
back_val <=[ Div[20, 4]()
stdout(back_val.toString())

// | |> condition
// C20-5: multi-line rhs multi-arm guard is rejected with [E0303]; wrap in parens.
score <= 85
grade <= (
  | score >= 90 |> "A"
  | score >= 80 |> "B"
  | _ |> "C"
)
stdout(grade)

// |== error ceiling
Error => TestError = @(code: Int)
safeOp x: Int =
  |== error: Error =
    0
  => :Int
  | x < 0 |> TestError(type <= "TestError", message <= "neg", code <= 1).throw()
  | _ |> x * 2
=> :Int
stdout(safeOp(5).toString())
stdout(safeOp(-1).toString())

// Arithmetic
stdout((3 + 5).toString())
stdout((10 - 3).toString())
stdout((4 * 5).toString())

// Comparison
stdout((3 == 3).toString())
stdout((3 != 4).toString())
stdout((3 < 5).toString())
stdout((5 > 3).toString())
// NOTE: <= is assignment, not comparison. Use !(x > y) for "less than or equal"
stdout((!(3 > 3)).toString())
stdout((3 >= 3).toString())

// Logical
stdout((true && true).toString())
stdout((true || false).toString())
stdout((!false).toString())

// String concat
stdout("Hello" + " " + "World")
