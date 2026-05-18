// === BuchiPack field function call ===
// obj.field(args) where field is a lambda stored in BuchiPack

// 0-arity field call
actions <= @(
  greet <= _ = "hi"
)
stdout(actions.greet())

// 1-arg field call: return arg directly (string)
echo <= @(
  say <= _ msg: Str = msg
)
stdout(echo.say("hello from field"))

// 1-arg field call: numeric operation
doubler <= @(
  run <= _ x: Int = x * 2
)
stdout(doubler.run(21).toString())

// 2-arg field call: numeric addition
calc <= @(
  plus <= _ a: Int b: Int = a + b
)
stdout(calc.plus(3, 7).toString())

// 1-arg closure field call (captures outer variable)
tag <= "PREFIX"
box <= @(
  wrap <= _ x: Str = tag + x
)
stdout(box.wrap(":item"))
