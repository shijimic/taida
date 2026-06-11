// B-8i helper: export function with pipeline
processNums nums: @[Int] =
  Filter[nums, _ x: Int = Mod[x, 2]().unmold() == 0]() >=> evens
  Map[evens, _ x: Int = x * 10]() >=> scaled
  Sum[scaled]() >=> total
  total
=> :Int

<<< @(processNums)
