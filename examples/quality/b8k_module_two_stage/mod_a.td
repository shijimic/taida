// B-8k mod_a: middle module, imports from mod_b
>>> ./mod_b.td => @(square)

sumSquares items =
  Map[items, _ x = square(x)]() ]=> squared
  Sum[squared]() ]=> total
  total
=> :Int

<<< @(sumSquares)
