// B-8k mod_a: middle module, imports from mod_b
>>> ./mod_b.td => @(square)

squareLocal x: Int = square(x) => :Int

sumSquares items: @[Int] =
  Map[items, squareLocal]() >=> squared
  Sum[squared]() >=> total
  total
=> :Int

<<< @(sumSquares)
