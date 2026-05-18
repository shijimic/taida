// W-5: Basic closure test — function returning a function
makeAdder x: Int =
  _ y: Int = x + y
=> :Int => :Int

add5 <= makeAdder(5)
add10 <= makeAdder(10)

debug(add5(3))
debug(add5(7))
debug(add10(3))
debug(add10(20))
