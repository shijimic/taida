// D-1f: docs/guide/05_molding.md code examples

// String molds
stdout(Upper["hello"]())
stdout(Lower["HELLO"]())
stdout(Trim["  hello  "]())
Split["a,b,c", ","]() ]=> split_result
stdout(split_result)
stdout(Replace["hello world", "o", "0"]())
stdout(Replace["hello world", "o", "0"](all <= true))
stdout(Slice["hello"](start <= 1, end <= 3))
stdout(CharAt["hello", 0]())
stdout(Repeat["ha", 3]())
stdout(Reverse["hello"]())
stdout(Pad["42", 5](side <= "start", char <= "0"))

// Number molds
stdout(ToFixed[3.14159, 2]())
Abs[-5]() ]=> abs_val
stdout(abs_val.toString())
Floor[3.7]() ]=> floor_val
stdout(floor_val.toString())
Ceil[3.2]() ]=> ceil_val
stdout(ceil_val.toString())
Round[3.5]() ]=> round_val
stdout(round_val.toString())
Truncate[-3.7]() ]=> trunc_val
stdout(trunc_val.toString())
Clamp[15, 0, 10]() ]=> clamp_val
stdout(clamp_val.toString())

// List molds
Sort[@[3, 1, 2]]() ]=> sorted
stdout(sorted)
Sort[@[3, 1, 2]](reverse <= true) ]=> sorted_rev
stdout(sorted_rev)
Unique[@[1, 2, 2, 3, 3]]() ]=> uniq
stdout(uniq)

// HOF molds
numbers <= @[1, 2, 3, 4, 5]
Filter[numbers, _ x = x > 3]() ]=> filtered
stdout(filtered)
Map[numbers, _ x = x * 2]() ]=> mapped
stdout(mapped)
Fold[numbers, 0, _ acc x = acc + x]() ]=> total
stdout(total.toString())

// Div / Mod
Div[10, 3]() ]=> quotient
stdout(quotient.toString())
Div[10, 0]() ]=> quotient_zero
stdout(quotient_zero.toString())
stdout(Div[10, 0]().has_value.toString())
Mod[10, 3]() ]=> remainder
stdout(remainder.toString())
Mod[10, 0]() ]=> remainder_zero
stdout(remainder_zero.toString())
stdout(Mod[10, 0]().has_value.toString())

// Type conversion molds
Int["123"]() ]=> conv_num
stdout(conv_num.toString())
Int["abc"]() ]=> conv_fail
stdout(conv_fail.toString())
Float["3.14"]() ]=> conv_float
stdout(conv_float.toString())
Str[42]() ]=> conv_str
stdout(conv_str)
Bool[1]() ]=> conv_true
stdout(conv_true.toString())
Bool[0]() ]=> conv_false
stdout(conv_false.toString())

// Lax methods
lax <= Div[10, 3]()
stdout(lax.has_value.toString())
stdout(lax.isEmpty().toString())
stdout(lax.getOrDefault(99).toString())
lax.map(_ x = x * 2) ]=> doubled
stdout(doubled.toString())
stdout(lax.toString())

empty_lax <= Div[10, 0]()
stdout(empty_lax.has_value.toString())
stdout(empty_lax.isEmpty().toString())
stdout(empty_lax.getOrDefault(99).toString())
empty_lax.map(_ x = x * 2) ]=> doubled2
stdout(doubled2.toString())
stdout(empty_lax.toString())

// Unmolding methods
lax2 <= Div[10, 3]()
lax2 ]=> val1
stdout(val1.toString())
val2 <=[ lax2
stdout(val2.toString())
val3 <= lax2.unmold()
stdout(val3.toString())

// Pipeline with molds
"  Hello, World!  " => Trim[_]() => Upper[_]() => pipe_result
stdout(pipe_result)
