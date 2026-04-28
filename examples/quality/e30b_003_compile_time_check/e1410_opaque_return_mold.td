// E30 Phase 5 / E30B-003 fixture (Mold variant):
// declare-only function field with opaque return type → [E1410]

Mold[T] => Foo[T] = @(
  name: Str,
  transform: T => :Opaque
)
