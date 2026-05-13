// D-2c: docs/reference/standard_methods.md code examples

// String methods
stdout("hello".length().toString())
stdout("hello".contains("ell").toString())
stdout("hello".startsWith("he").toString())
stdout("hello".endsWith("lo").toString())
// NOTE: String has no is_empty method (QF finding: doc says it does)
// stdout("".isEmpty().toString())
stdout("hello".length().toString())
stdout("hello".toString())

// List methods
nums <= @[1, 2, 3, 4, 5]
stdout(nums.length().toString())
stdout(nums.isEmpty().toString())
stdout(nums.contains(3).toString())
stdout(nums.contains(99).toString())
stdout(nums.indexOf(3).toString())
stdout(nums.indexOf(99).toString())
nums.get(2) ]=> got
stdout(got.toString())
nums.first() ]=> fst
stdout(fst.toString())
nums.last() ]=> lst
stdout(lst.toString())
nums.max() ]=> mx
stdout(mx.toString())
nums.min() ]=> mn
stdout(mn.toString())
stdout(nums.any(_ x = x > 4).toString())
stdout(nums.all(_ x = x > 0).toString())
stdout(nums.none(_ x = x > 10).toString())

// BuchiPack field access (toString not supported on buchi packs - QF finding)
pilot <= @(name <= "Asuka", age <= 14)
stdout(pilot.name)

// Lax methods
lax <= Div[10, 3]()
stdout(lax.has_value.toString())
stdout(lax.isEmpty().toString())
stdout(lax.getOrDefault(99).toString())
lax ]=> unmolded
stdout(unmolded.toString())

// toString on primitives
n <= 42
stdout(n.toString())
f <= 3.14
stdout(f.toString())
b <= true
stdout(b.toString())
