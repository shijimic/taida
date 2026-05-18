// === Taida Lang: Prelude -- Lax (Optional replacement) ===
// Optional is abolished in v0.8.0. Lax[T] is the replacement.
// Lax[T] provides the same safety with default value guarantees.

name <= Lax["Alice"]()
empty <= Div[0, 0]()  // has_value = false

// hasValue / is_empty
stdout("name.hasValue() = " + name.hasValue().toString())
stdout("empty.isEmpty() = " + empty.isEmpty().toString())

// getOrDefault
stdout("name.getOrDefault('unknown') = " + name.getOrDefault("unknown"))
stdout("empty.getOrDefault('unknown') = " + empty.getOrDefault("unknown"))

// map
double x: Int = x * 2 => :Int
result <= Lax[21]().map(double)
stdout("Lax[21]().map(double) = " + result.getOrDefault(0).toString())
stdout("Div[0,0]().map(double) = " + Div[0, 0]().map(double).getOrDefault(0).toString())

// flatMap
safeDivide x: Int =
  | x > 0 |> Lax[Div[100, x]().unmold()]()
  | _ |> Div[0, 0]()
=> :Lax[Int]

result2 <= Lax[5]().flatMap(safeDivide)
stdout("Lax[5]().flatMap(safeDivide) = " + result2.getOrDefault(0).toString())

result3 <= Div[0, 0]().flatMap(safeDivide)
stdout("Div[0,0]().flatMap(safeDivide) = " + result3.getOrDefault(0).toString())

// toString
stdout("Lax[42]().toString() = " + Lax[42]().toString())
stdout("Div[0,0]().toString() = " + Div[0, 0]().toString())

// typeof
stdout("typeof(Lax[1]()) = " + typeof(Lax[1]()))
stdout("typeof(Div[0,0]()) = " + typeof(Div[0, 0]()))
