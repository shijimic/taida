// D-2b: docs/reference/class_like_types.md code examples (formerly mold_types.md, renamed in E30 Phase 1 Sub-step 1.3)

// String molds
stdout(Upper["hello"]())
stdout(Lower["WORLD"]())
stdout(Trim["  test  "]())
Split["a-b-c", "-"]() ]=> split_r
stdout(split_r)
stdout(Replace["abcabc", "b", "X"]())
stdout(Replace["abcabc", "b", "X"](all <= true))
stdout(CharAt["hello", 1]())
stdout(Repeat["ab", 3]())
stdout(Reverse["abc"]())

// Number molds
stdout(ToFixed[3.14159, 2]())
Abs[-7]() ]=> abs_r
stdout(abs_r.toString())
Floor[3.9]() ]=> floor_r
stdout(floor_r.toString())
Ceil[3.1]() ]=> ceil_r
stdout(ceil_r.toString())
Round[3.5]() ]=> round_r
stdout(round_r.toString())
Clamp[15, 0, 10]() ]=> clamp_r
stdout(clamp_r.toString())

// List molds
Sort[@[5, 3, 1, 4, 2]]() ]=> sorted
stdout(sorted)
Unique[@[1, 1, 2, 2, 3]]() ]=> unique
stdout(unique)
Flatten[@[@[1, 2], @[3, 4]]]() ]=> flat
stdout(flat)
Concat[@[1, 2], @[3]]() ]=> concat_r
stdout(concat_r)
Append[@[1, 2], 3]() ]=> append_r
stdout(append_r)
Prepend[@[1, 2], 0]() ]=> prepend_r
stdout(prepend_r)
Join[@["x", "y", "z"], "-"]() ]=> join_r
stdout(join_r)
Sum[@[1, 2, 3, 4]]() ]=> sum_r
stdout(sum_r.toString())
Take[@[1, 2, 3, 4, 5], 3]() ]=> take_r
stdout(take_r)
Drop[@[1, 2, 3, 4, 5], 2]() ]=> drop_r
stdout(drop_r)

// HOF molds
Map[@[1, 2, 3], _ x = x * 3]() ]=> map_r
stdout(map_r)
Filter[@[1, 2, 3, 4, 5], _ x = x > 2]() ]=> filter_r
stdout(filter_r)
Fold[@[1, 2, 3], 0, _ acc x = acc + x]() ]=> fold_r
stdout(fold_r.toString())

// Div/Mod
Div[100, 7]() ]=> div_r
stdout(div_r.toString())
Mod[100, 7]() ]=> mod_r
stdout(mod_r.toString())

// Type conversion
Int["99"]() ]=> int_r
stdout(int_r.toString())
Float["2.5"]() ]=> float_r
stdout(float_r.toString())
Str[123]() ]=> str_r
stdout(str_r)
Bool[1]() ]=> bool_r
stdout(bool_r.toString())

// Lax
lax_ok <= Div[10, 2]()
stdout(lax_ok.hasValue.toString())
lax_ok ]=> lax_val
stdout(lax_val.toString())
lax_fail <= Div[10, 0]()
stdout(lax_fail.hasValue.toString())
stdout(lax_fail.getOrDefault(42).toString())
