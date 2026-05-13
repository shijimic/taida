// D-1c: docs/guide/02_strict_typing.md code examples

// Explicit conversion
num <= 42
str <= num.toString()
stdout(str)
Int["42"]() ]=> back
stdout(back.toString())

// Div/Mod safe operations
Div[10, 3]() ]=> div_result
stdout(div_result.toString())
Div[10, 0]() ]=> div_zero
stdout(div_zero.toString())

// hasValue check
lax <= Div[10, 0]()
stdout(lax.has_value.toString())
lax2 <= Div[10, 3]()
stdout(lax2.has_value.toString())

// Mod
Mod[10, 3]() ]=> mod_result
stdout(mod_result.toString())
Mod[10, 0]() ]=> mod_zero
stdout(mod_zero.toString())
stdout(Mod[10, 0]().has_value.toString())

// Range-safe access
items <= @[10, 20, 30]
items.get(1) ]=> val
stdout(val.toString())
items.get(100) ]=> val2
stdout(val2.toString())

// first/last/max/min
@[1, 2, 3].first() ]=> first_val
stdout(first_val.toString())
empty: @[Int] <= @[]
empty.first() ]=> empty_first
stdout(empty_first.toString())
@[1, 2, 3].last() ]=> last_val
stdout(last_val.toString())
empty.last() ]=> empty_last
stdout(empty_last.toString())
@[1, 3, 2].max() ]=> max_val
stdout(max_val.toString())
empty.max() ]=> empty_max
stdout(empty_max.toString())

// hasValue for empty list
lax3 <= empty.first()
stdout(lax3.has_value.toString())

// JSON as airlock
Pilot = @(name: Str, age: Int, sync_rate: Int)
raw <= '{"name": "Shinji", "age": 14, "sync_rate": 78}'
JSON[raw, Pilot]() ]=> pilot
stdout(pilot.name)
stdout(pilot.age.toString())

// JSON null -> default
raw2 <= '{"name": null, "age": null}'
JSON[raw2, Pilot]() ]=> pilot2
stdout(pilot2.name)
stdout(pilot2.age.toString())

// Safe operations
Div[10, 0]() ]=> safe_div
stdout(safe_div.toString())
@[1, 2].get(100) ]=> safe_get
stdout(safe_get.toString())
Int["abc"]() ]=> safe_conv
stdout(safe_conv.toString())
JSON['{}', Pilot]() ]=> safe_json
stdout(safe_json.name)
stdout(safe_json.age.toString())
