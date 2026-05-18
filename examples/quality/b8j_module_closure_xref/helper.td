// B-8j helper: closure cross-reference (F-56 regression)
// export function A's closure references non-export function B

formatNum n: Int =
  "[" + n.toString() + "]"
=> :Str

wrapAll items: @[Int] =
  Map[items, _ x: Int = formatNum(x)]() >=> result
  Join[result, ","]() >=> joined
  joined
=> :Str

<<< @(wrapAll)
