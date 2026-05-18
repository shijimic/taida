// Taida Lang Example: List Operation Mold Types

isEven x: Int =
  Mod[x, 2]().unmold() == 0
=> :Bool

numbers <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

// Map: double each number
Map[numbers, _ x: Int = x * 2]() >=> doubled
stdout(doubled)

// Filter: even numbers only
Filter[numbers, isEven]() >=> evens
stdout(evens)

// Fold: sum all numbers
Fold[numbers, 0, _ acc: Int x: Int = acc + x]() >=> sum
stdout(sum)

// Chain: filter evens, double them, sum
Filter[numbers, isEven]() >=> step1
Map[step1, _ x: Int = x * 2]() >=> step2
Fold[step2, 0, _ acc: Int x: Int = acc + x]() >=> total
stdout(total)

// Take first 3
Take[numbers, 3]() >=> first3
stdout(first3)

// Drop first 5
Drop[numbers, 5]() >=> rest
stdout(rest)

// TakeWhile < 5
TakeWhile[numbers, _ x: Int = x < 5]() >=> under5
stdout(under5)
