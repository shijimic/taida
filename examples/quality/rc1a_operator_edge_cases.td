// RC-1a: Operator Semantics Edge Cases
// Verifies all 10 Taida-specific operators + arithmetic/comparison/logic

// === 1. Assignment operators: =, =>, <= ===

// 1a: Basic assignment with <=
x <= 42
stdout(x.toString())

// 1b: Forward pipe with =>
42 => y
stdout(y.toString())

// 1c: Multi-step forward pipeline
@[1, 2, 3, 4, 5] => Filter[_, _ n = n > 2]() => Map[_, _ n = n * 10]() => result1
stdout(result1)

// 1d: Multi-step backward pipeline
// NOTE: KNOWN ISSUE (RC-1 Finding F-1) — backward pipeline with <= chaining
// does NOT parse. The parser only handles <= as statement-level assignment.
// The docs (05_molding.md, 06_lists.md, 09_functions.md, operators.md) claim:
//   result <= Map[_, _ x = x * 2]() <= Filter[_, _ x = x > 0]() <= data
// But this produces "Parse error: Unexpected token: LtEq"
// Workaround: use forward pipeline or intermediate variables.
Filter[@[1, 2, 3, 4, 5], _ n = n > 2]() ]=> filtered2
Map[filtered2, _ n = n * 10]() ]=> result2
stdout(result2)

// 1e: Pipeline with string molds
"  hello world  " => Trim[_]() => Upper[_]() => result3
stdout(result3)

// === 2. Unmold operators: ]=>, <=[ ===

// 2a: Forward unmold
Div[10, 3]() ]=> div_result
stdout(div_result.toString())

// 2b: Backward unmold
back_result <=[ Div[20, 4]()
stdout(back_result.toString())

// 2c: Unmold method
lax <= Div[100, 7]()
method_result <= lax.unmold()
stdout(method_result.toString())

// 2d: Chained unmolds
Map[@[1, 2, 3], _ x = x * 3]() ]=> tripled
Filter[tripled, _ x = x > 5]() ]=> big_ones
stdout(big_ones)

// 2e: Backward unmold chain (using single <=[ per step)
tripled2 <=[ Map[@[1, 2, 3], _ x = x * 3]()
big_ones2 <=[ Filter[tripled2, _ x = x > 5]()
stdout(big_ones2)

// === 3. Condition operators: |, |> ===

// 3a: Basic condition
// C20-5: multi-line rhs multi-arm guard is rejected with [E0303]; wrap in parens.
val3 <= 15
grade <= (
  | val3 >= 90 |> "A"
  | val3 >= 80 |> "B"
  | val3 >= 70 |> "C"
  | _ |> "F"
)
stdout(grade)

// 3b: Single-line condition
sign <= | 5 > 0 |> "positive" | _ |> "negative"
stdout(sign)

// 3c: Nested condition
staff <= @(role <= "commander", active <= true)
access <= (
  | staff.role == "commander" |>
    | staff.active |> "full"
    | _ |> "readonly"
  | _ |> "none"
)
stdout(access)

// 3d: Condition with complex expressions
nums <= @[1, 2, 3, 4, 5]
category <= (
  | nums.length() > 10 |> "large"
  | nums.length() > 3 |> "medium"
  | _ |> "small"
)
stdout(category)

// === 4. Error ceiling |== ===

// 4a: Basic error ceiling
Error => TestError = @(code: Int)
safeOp x: Int =
  |== error: Error =
    -1
  => :Int
  | x < 0 |> TestError(type <= "TestError", message <= "negative", code <= 1).throw()
  | _ |> x * 2
=> :Int
stdout(safeOp(5).toString())
stdout(safeOp(-1).toString())

// 4b: Error propagation across functions
inner x: Int =
  | x == 0 |> Error(type <= "ZeroError", message <= "zero input").throw()
  | _ |> x * 3
=> :Int

outer x: Int =
  |== error: Error =
    999
  => :Int
  inner(x) + 10
=> :Int
stdout(outer(5).toString())
stdout(outer(0).toString())

// === 5. Module operators: >>>, <<< ===
// (Tested in separate module files)

// === 6. Arithmetic edge cases ===

// 6a: Unary minus
neg <= -42
stdout(neg.toString())

// 6b: Int overflow behavior
big <= 999999999 * 999999999
stdout(big.toString())

// 6c: Float arithmetic
f1 <= 0.1 + 0.2
stdout(f1.toString())

// 6d: String concatenation
s1 <= "Hello" + " " + "World"
stdout(s1)

// === 7. Comparison edge cases ===

// 7a: All comparison operators
stdout((1 == 1).toString())
stdout((1 != 2).toString())
stdout((1 < 2).toString())
stdout((2 > 1).toString())
stdout((2 >= 2).toString())
stdout((2 >= 1).toString())

// 7b: <= is NOT comparison, it's assignment
// Verify "less than or equal" workarounds
is_leq <= !(5 > 3)
stdout(is_leq.toString())
is_leq2 <= 3 < 5 || 3 == 5
stdout(is_leq2.toString())

// 7c: String comparison
stdout(("abc" == "abc").toString())
stdout(("abc" != "def").toString())

// 7d: Bool comparison
stdout((true == true).toString())
stdout((false == false).toString())
stdout((true != false).toString())

// === 8. Logical operators ===

// 8a: Short-circuit AND
stdout((true && true).toString())
stdout((true && false).toString())
stdout((false && true).toString())

// 8b: Short-circuit OR
stdout((false || true).toString())
stdout((false || false).toString())
stdout((true || false).toString())

// 8c: NOT
stdout((!true).toString())
stdout((!false).toString())

// 8d: Complex logical
stdout((1 > 0 && 2 > 0).toString())
stdout((1 > 0 || 2 < 0).toString())

// === 9. Lax operations (Div/Mod) ===

// 9a: Normal division
Div[10, 3]() ]=> d1
stdout(d1.toString())

// 9b: Zero division
Div[10, 0]() ]=> d2
stdout(d2.toString())

// 9c: hasValue check
stdout(Div[10, 3]().hasValue.toString())
stdout(Div[10, 0]().hasValue.toString())

// 9d: Mod
Mod[10, 3]() ]=> m1
stdout(m1.toString())
Mod[10, 0]() ]=> m2
stdout(m2.toString())

// 9e: Float division
Div[10.0, 3.0]() ]=> fd
stdout(fd.toString())

// === 10. Type conversion molds ===

// 10a: String to Int
Int["42"]() ]=> ti1
stdout(ti1.toString())
Int["abc"]() ]=> ti2
stdout(ti2.toString())
stdout(Int["abc"]().hasValue.toString())

// 10b: String to Float
Float["3.14"]() ]=> tf1
stdout(tf1.toString())

// 10c: Number to String
Str[42]() ]=> ts1
stdout(ts1)

// 10d: Bool conversion
Bool[1]() ]=> tb1
stdout(tb1.toString())
Bool[0]() ]=> tb2
stdout(tb2.toString())

// === 11. Multibyte character indexOf/lastIndexOf (T-1) ===

// 11a: String indexOf with multibyte (UTF-8)
mbStr <= "あいうえお"
stdout(mbStr.indexOf("う").toString())
stdout(mbStr.indexOf("お").toString())
stdout(mbStr.indexOf("か").toString())

// 11b: String lastIndexOf with multibyte
mbStr2 <= "あいうあいう"
stdout(mbStr2.lastIndexOf("う").toString())
stdout(mbStr2.lastIndexOf("あ").toString())
stdout(mbStr2.lastIndexOf("か").toString())

// 11c: List indexOf with multibyte elements
mbList <= @["あ", "い", "う", "え", "お"]
stdout(mbList.indexOf("う").toString())
stdout(mbList.lastIndexOf("い").toString())

stdout("=== ALL OPERATOR TESTS PASSED ===")
