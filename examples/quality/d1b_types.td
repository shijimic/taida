// D-1b: docs/guide/01_types.md code examples

// Primitives
count <= 42
negative <= -10
pi <= 3.14159
rate <= 0.5
name <= "Rei"
greeting <= 'Hello'
template <= `Hello, ${name}!`
active <= true
deleted <= false

stdout(count.toString())
stdout(negative.toString())
stdout(pi.toString())
stdout(name)
stdout(greeting)
stdout(template)
stdout(active.toString())

// String molds
Upper["hello"]() ]=> upper_result
stdout(upper_result)
Trim["  hello  "]() ]=> trim_result
stdout(trim_result)
Split["a,b,c", ","]() ]=> split_result
stdout(split_result)
Replace["hello world", "world", "taida"]() ]=> replace_result
stdout(replace_result)

// String methods
stdout("hello".length().toString())
stdout("hello".contains("ell").toString())
stdout("hello".startsWith("he").toString())

// List
numbers <= @[1, 2, 3, 4, 5]
names <= @["Asuka", "Rei"]
stdout(numbers.length().toString())
stdout(names)

// List molds
Sort[@[3, 1, 2]]() ]=> sorted
stdout(sorted)
Filter[@[1, 2, 3, 4], _ x = x > 2]() ]=> filtered
stdout(filtered)
Map[@[1, 2, 3], _ x = x * 2]() ]=> mapped
stdout(mapped)
Concat[@[1, 2], @[3, 4]]() ]=> concatenated
stdout(concatenated)
Join[@["a", "b", "c"], ","]() ]=> joined
stdout(joined)

// List methods
stdout(@[1, 2, 3].length().toString())
stdout(@[1, 2, 3].isEmpty().toString())
stdout(@[1, 2, 3].contains(2).toString())
@[1, 2, 3].get(0) ]=> val
stdout(val.toString())

// BuchiPack
pilot <= @(name <= "Shinji", age <= 14)
Pilot = @(
  name: Str
  age: Int
  active: Bool
)
asuka <= Pilot(name <= "Asuka", age <= 14, active <= true)
stdout(pilot.name)
stdout(asuka.name)
stdout(asuka.active.toString())

// Lax
Div[10, 3]() ]=> div_result
stdout(div_result.toString())
Div[10, 0]() ]=> div_zero
stdout(div_zero.toString())

items <= @[10, 20, 30]
items.get(1) ]=> item_val
stdout(item_val.toString())
items.get(100) ]=> item_oob
stdout(item_oob.toString())

// Type conversion molds
Int["123"]() ]=> num
stdout(num.toString())
Int["abc"]() ]=> num_fail
stdout(num_fail.toString())
Float["3.14"]() ]=> fval
stdout(fval.toString())
Str[42]() ]=> text
stdout(text)

// toString
n42 <= 42
stdout(n42.toString())
f314 <= 3.14
stdout(f314.toString())
btrue <= true
stdout(btrue.toString())

// JSON schema matching
Pilot2 = @(name: Str, age: Int, sync_rate: Int)
raw <= '{"name": "Asuka", "age": 14, "sync_rate": 78}'
JSON[raw, Pilot2]() ]=> json_pilot
stdout(json_pilot.name)
stdout(json_pilot.age.toString())

// JSON missing field -> default
raw2 <= '{"name": "Rei"}'
JSON[raw2, Pilot2]() ]=> json_pilot2
stdout(json_pilot2.name)
stdout(json_pilot2.age.toString())
stdout(json_pilot2.sync_rate.toString())

// JSON null -> default
raw3 <= '{"name": null, "age": null}'
JSON[raw3, Pilot2]() ]=> json_pilot3
stdout(json_pilot3.name)
stdout(json_pilot3.age.toString())

// jsonEncode
pilot_out <= Pilot2(name <= "Asuka", age <= 14, sync_rate <= 78)
stdout(jsonEncode(pilot_out))

// Type inference
num2 <= 42
text2 <= "hello"
flag2 <= true
list2 <= @[1, 2, 3]
sum2 <= 1 + 2
concat2 <= "a" + "b"
pilot3 <= @(name <= "Rei", age <= 14)
upper2 <= Upper["hello"]()
divided <= Div[10, 3]()
stdout(num2.toString())
stdout(text2)
stdout(sum2.toString())
stdout(concat2)

// Default values
Pilot3 = @(name: Str, call_sign: Str, age: Int)
rei <= Pilot3(name <= "Rei")
stdout(rei.call_sign)
stdout(rei.age.toString())
