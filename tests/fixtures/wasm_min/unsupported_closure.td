// Historical negative fixture: closures are now supported in wasm-min
makeAdder x: Int =
  _ y: Int = x + y
=> :Int => :Int

add5 <= makeAdder(5)
debug(add5(3))
