// E30B-001 / Phase 7 sub-track A: post-migration class-like (unified syntax).
//
// `taida upgrade --e30` drops the `Mold[T] => ` prefix. The post-migration
// header `Box[T] = @(...)` is a plain class-like definition (zero-arity
// sugar / generic class-like, Lock-B Sub-B1). Instantiation switches from
// the Mold contract `Box[1, "apple"]()` to the unified class-like contract
// `Box(label <= "apple")`.

Box[T] = @(
  label: Str,
  transform: T => :T
)
b <= Box(label <= "apple")
stdout(b.label)
