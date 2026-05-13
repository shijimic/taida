// RC-1a: Type System Edge Cases
// Verifies type system rules: defaults, structural subtyping, mold semantics

// === 1. Default values for all types ===

// 1a: Primitive defaults via type instantiation
// NOTE: KNOWN ISSUE (RC-1 Finding F-2) — TypeDef() with zero args fails.
// PilotA() is parsed as a function call, not TypeDef instantiation.
// Workaround: supply at least one field.
PilotA = @(name: Str, age: Int, pi: Float, active: Bool)
p1 <= PilotA(name <= "")
stdout(p1.name)
stdout(p1.age.toString())
stdout(p1.pi.toString())
stdout(p1.active.toString())

// 1b: Partial field initialization
p2 <= PilotA(name <= "Rei")
stdout(p2.name)
stdout(p2.age.toString())

// === 2. Structural subtyping ===

// 2a: Subtype accepted where supertype expected
Base = @(name: Str, age: Int)
Extended = @(name: Str, age: Int, role: Str, active: Bool)

greetBase b: Base =
  "Hello, " + b.name + " age " + b.age.toString()
=> :Str

ext <= Extended(name <= "Ritsuko", age <= 30, role <= "scientist", active <= true)
stdout(greetBase(ext))

// === 3. Type conversion molds ===

// 3a: Int conversion edge cases
Int["0"]() ]=> zero
stdout(zero.toString())
Int["-5"]() ]=> neg
stdout(neg.toString())
Int[""]() ]=> emptyInt
stdout(emptyInt.toString())
stdout(Int[""]().has_value.toString())

// 3b: Float conversion edge cases
Float["0.0"]() ]=> zeroF
stdout(zeroF.toString())
Float["-3.14"]() ]=> negF
stdout(negF.toString())

// 3c: Bool conversion
Bool[1]() ]=> t
stdout(t.toString())
Bool[0]() ]=> f
stdout(f.toString())
Bool["true"]() ]=> st
stdout(st.toString())
Bool["false"]() ]=> sf
stdout(sf.toString())

// 3d: Str conversion
Str[0]() ]=> zeroS
stdout(zeroS)
Str[3.14]() ]=> piS
stdout(piS)
Str[true]() ]=> trueS
stdout(trueS)
Str[false]() ]=> falseS
stdout(falseS)

// === 4. List operations correctness ===

// 4a: Empty list operations
empty: @[Int] <= @[]
stdout(empty.length().toString())
stdout(empty.isEmpty().toString())
empty.first() ]=> ef
stdout(ef.toString())
empty.last() ]=> el
stdout(el.toString())
empty.max() ]=> em
stdout(em.toString())
stdout(empty.first().has_value.toString())

// 4b: List methods
nums <= @[3, 1, 4, 1, 5, 9, 2, 6]
stdout(nums.length().toString())
stdout(nums.contains(4).toString())
stdout(nums.contains(99).toString())
stdout(nums.indexOf(4).toString())
stdout(nums.indexOf(99).toString())

// 4c: List access with get
nums.get(0) ]=> first
stdout(first.toString())
nums.get(7) ]=> last
stdout(last.toString())
nums.get(100) ]=> oob
stdout(oob.toString())
stdout(nums.get(100).has_value.toString())

// 4d: Max/Min
nums.max() ]=> mx
stdout(mx.toString())
nums.min() ]=> mn
stdout(mn.toString())

// === 5. String operations ===

// 5a: String state methods
stdout("hello".length().toString())
stdout("hello".contains("ell").toString())
stdout("hello".startsWith("he").toString())
stdout("hello".endsWith("lo").toString())
// Note: isEmpty() is NOT a Str method (only List/Lax/Gorillax have it)
// Use length() == 0 instead
stdout(("".length() == 0).toString())
stdout(("hello".length() == 0).toString())

// 5b: String molds
Upper["hello"]() => upper
stdout(upper)
Lower["HELLO"]() => lower
stdout(lower)
Trim["  hello  "]() => trimmed
stdout(trimmed)
Split["a,b,c", ","]() => parts
stdout(parts)
Replace["hello world", "world", "taida"]() => replaced
stdout(replaced)
CharAt["hello", 0]() => ch
stdout(ch)
Repeat["ha", 3]() => repeated
stdout(repeated)
Reverse["hello"]() => reversed
stdout(reversed)

// === 6. Numeric molds ===
ToFixed[3.14159, 2]() => fixed
stdout(fixed)
Abs[-5]() => abs_val
stdout(abs_val.toString())
Floor[3.7]() => floored
stdout(floored.toString())
Ceil[3.2]() => ceiled
stdout(ceiled.toString())
Round[3.5]() => rounded
stdout(rounded.toString())
Clamp[15, 0, 10]() => clamped
stdout(clamped.toString())

// === 7. List molds ===
Sort[@[3, 1, 4, 1, 5]]() ]=> sorted
stdout(sorted)
Sort[@[3, 1, 2]](reverse <= true) ]=> desc
stdout(desc)
Reverse[@[1, 2, 3]]() ]=> rev
stdout(rev)
Unique[@[1, 2, 2, 3, 3, 1]]() ]=> uniq
stdout(uniq)
Flatten[@[@[1, 2], @[3, 4]]]() ]=> flat
stdout(flat)
Concat[@[1, 2], @[3, 4]]() ]=> concat
stdout(concat)
Append[@[1, 2], 3]() ]=> appended
stdout(appended)
Prepend[@[2, 3], 1]() ]=> prepended
stdout(prepended)
Join[@["a", "b", "c"], ","]() ]=> joined
stdout(joined)
Sum[@[1, 2, 3, 4, 5]]() ]=> sum
stdout(sum.toString())

// === 8. HOF molds ===
Map[@[1, 2, 3], _ x = x * 2]() ]=> mapped
stdout(mapped)
Filter[@[1, 2, 3, 4, 5], _ x = x > 3]() ]=> filtered
stdout(filtered)
Fold[@[1, 2, 3, 4, 5], 0, _ acc x = acc + x]() ]=> folded
stdout(folded.toString())
Find[@[1, 2, 3, 4, 5], _ x = x > 3]() ]=> found
stdout(found.toString())
stdout(Find[@[1, 2, 3], _ x = x > 10]().has_value.toString())
FindIndex[@[10, 20, 30, 40], _ x = x > 25]() => found_idx
stdout(found_idx.toString())
Count[@[1, 2, 3, 4, 5], _ x = x > 2]() => counted
stdout(counted.toString())
Take[@[1, 2, 3, 4, 5], 3]() ]=> taken
stdout(taken)
Drop[@[1, 2, 3, 4, 5], 3]() ]=> dropped
stdout(dropped)

// === 9. Lax monad operations ===
lax1 <= Div[10, 3]()
lax1.map(_ x = x * 2) ]=> doubled
stdout(doubled.toString())
stdout(lax1.getOrDefault(99).toString())

lax2 <= Div[10, 0]()
lax2.map(_ x = x * 2) ]=> dbl2
stdout(dbl2.toString())
stdout(lax2.getOrDefault(99).toString())

// === 10. Error types ===
Error => CustomError = @(code: Int)

handleErr x: Int =
  |== error: Error =
    error.message
  => :Str
  | x < 0 |> CustomError(type <= "CustomError", message <= "negative", code <= x).throw()
  | _ |> "ok"
=> :Str

stdout(handleErr(5))
stdout(handleErr(-1))

// === 11. Result mold ===
ok <= Result[42, _ = true]()
ok ]=> okVal
stdout(okVal.toString())

// Result failure must be tested inside a function to avoid top-level termination
testResultFail =
  |== error: Error =
    -1
  => :Int
  err_res <= Result[0, _ = false](throw <= Error(type <= "Error", message <= "fail"))
  err_res ]=> errVal
  errVal
=> :Int
stdout(testResultFail().toString())

// === 12. toString method ===
stdout(42.toString())
stdout(3.14.toString())
stdout(true.toString())
stdout("hello".toString())

stdout("=== ALL TYPE SYSTEM TESTS PASSED ===")
