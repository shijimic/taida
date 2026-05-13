// D-1i: docs/guide/08_error_handling.md code examples

// Lax safe operations
Div[10, 3]() ]=> div_ok
stdout(div_ok.toString())
Mod[10, 3]() ]=> mod_ok
stdout(mod_ok.toString())
Div[10, 0]() ]=> div_zero
stdout(div_zero.toString())
Mod[10, 0]() ]=> mod_zero
stdout(mod_zero.toString())
stdout(Div[10, 3]().has_value.toString())
stdout(Div[10, 0]().has_value.toString())

// List safe access
items <= @[10, 20, 30]
items.get(1) ]=> val1
stdout(val1.toString())
items.first() ]=> val2
stdout(val2.toString())
items.last() ]=> val3
stdout(val3.toString())
items.get(100) ]=> val4
stdout(val4.toString())
empty: @[Int] <= @[]
empty.first() ]=> val5
stdout(val5.toString())

// Type conversion
Int["123"]() ]=> conv1
stdout(conv1.toString())
Int["abc"]() ]=> conv2
stdout(conv2.toString())
stdout(Int["abc"]().has_value.toString())

// hasValue branching
user_input <= "42"
Int[user_input]() => parsed
| parsed.has_value |>
  stdout("Parsed: " + parsed.unmold().toString())
| _ |>
  stdout("Invalid input")

// Custom error with throw and |==
Error => ValidationError = @(field: Str, code: Int)

validate text: Str =
  |== error: Error =
    | error.type == "ValidationError" |> "Invalid: " + error.message
    | _ |> "Unknown error: " + error.message
  => :Str
  | text == "" |> ValidationError(type <= "ValidationError", message <= "Empty input", field <= "text", code <= 400).throw()
  | _ |> "Valid: " + text
=> :Str

stdout(validate("hello"))
stdout(validate(""))

// Error propagation
Error => NegativeValueError = @(value: Int)

innerFunction value: Int =
  | value < 0 |> NegativeValueError(type <= "NegativeValueError", message <= "Must be positive", value <= value).throw()
  | _ |> value * 2
=> :Int

outerFunction x: Int =
  |== error: Error =
    | error.type == "NegativeValueError" |> 0
    | _ |> -1
  => :Int
  result <= innerFunction(x)
  result + 10
=> :Int

stdout(outerFunction(-5).toString())
stdout(outerFunction(5).toString())
