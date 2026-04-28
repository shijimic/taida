// E30 Phase 5 / E30B-003 fixture (TypeDef variant):
// declare-only function field with opaque return type → [E1410]
//
// `Opaque` は registered しない unknown alias であり、defaultFn 生成不能。
// definition-site で `[E1410]` reject される (taida check 段階で発火)。

Pilot = @(
  name: Str,
  build: Str => :Opaque
)
