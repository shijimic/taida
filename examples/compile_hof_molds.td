// compile_hof_molds.td -- Native HOF mold types test
// Tests: Map, Filter, Fold, Foldr, Reduce, Take, TakeWhile, Drop, DropWhile

// helper
double x: Int = x * 2 => :Int
isEven x: Int =
  Mod[x, 2]() >=> r
  r == 0
=> :Bool
isSmall x: Int = x < 5 => :Bool

nums <= @[1, 2, 3, 4, 5]

// -- Map[list, fn]()
Map[nums, double]() >=> doubled
stdout(Join[doubled, " "]())

// -- Filter[list, fn]()
Filter[nums, isEven]() >=> evens
stdout(Join[evens, " "]())

// -- Fold[list, init, fn]() -- left fold (sum)
Fold[nums, 0, _ acc: Int x: Int = acc + x]() >=> sum
stdout(sum.toString())

// -- Foldr[list, init, fn]() -- right fold (sum from right)
Foldr[nums, 0, _ acc: Int x: Int = acc + x]() >=> rsum
stdout(rsum.toString())

// -- Reduce (alias of Fold)
Reduce[nums, 10, _ acc: Int x: Int = acc + x]() >=> reduced
stdout(reduced.toString())

// -- Take[list, n]()
Take[nums, 3]() >=> first3
stdout(Join[first3, " "]())

// -- TakeWhile[list, fn]()
TakeWhile[nums, isSmall]() >=> under5
stdout(Join[under5, " "]())

// -- Drop[list, n]()
Drop[nums, 2]() >=> rest
stdout(Join[rest, " "]())

// -- DropWhile[list, fn]()
DropWhile[nums, isSmall]() >=> after5
stdout(Join[after5, " "]())
