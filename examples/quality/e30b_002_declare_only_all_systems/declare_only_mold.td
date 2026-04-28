// E30B-002 / E30 Phase 4: declare-only function field on a Mold variant.
//
// `transform: T => :T` is a declare-only function field. Phase 4 (E30B-002)
// excludes it from the required-positional `[]` set, so `Foo[1, "x"]()`
// can omit `transform` from the binding list. Phase 6 (E30B-004) will
// replace the runtime `Value::Unit` placeholder with an automatically-
// generated `defaultFn`; this fixture deliberately does not call
// `f.transform(...)` so it remains parity-safe across backends both
// before and after Phase 6.
Mold[T] => Foo[T] = @(
  name: Str,
  transform: T => :T
)
f <= Foo[1, "x"]()
stdout(f.name)
