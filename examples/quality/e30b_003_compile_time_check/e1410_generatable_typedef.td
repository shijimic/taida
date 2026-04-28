// E30 Phase 5 / E30B-003 fixture (regression guard / non-firing case):
// declare-only function field with generatable return type (Str = primitive)
// → 0 errors. defaultFn (Phase 6 land 済) が default 値 "" を返す。

Pilot = @(
  name: Str,
  greet: Str => :Str
)
p <= Pilot(name <= "Rei")
stdout(p.name)
