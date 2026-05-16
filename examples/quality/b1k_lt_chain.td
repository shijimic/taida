// `<=` chain — backward pipeline assignment
//
// `result <= f(_) <= g(_) <= data` は
// `data => g(_) => f(_) => result` と等価で、
// 4 backend で同一出力を返す。
double x = x * 2 => :Int
plusOne x = x + 1 => :Int

// untyped chain
basic <= plusOne(_) <= double(_) <= 4
stdout(basic.toString())
// Expected: 9

// typed chain
typed: Int <= plusOne(_) <= double(_) <= 10
stdout(typed.toString())
// Expected: 21

// single binding still works (no chain)
single <= plusOne(7)
stdout(single.toString())
// Expected: 8
