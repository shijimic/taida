// JS build example: taida build --target js examples/transpile_j2.td -o out.mjs
Pilot = @(name: Str, age: Int)
asuka <= Pilot(name <= "Asuka", age <= 14)
debug(asuka.name)
debug(asuka.age)

double <= _ x = x * 2
debug(double(5))

nums <= @[1, 2, 3]
debug(nums)

// C20-1 (ROOT-5): parenthesise multi-line rhs guard.
result <= (
  | asuka.age > 25 |> "adult"
  | _ |> "young"
)
debug(result)
