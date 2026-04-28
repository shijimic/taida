// E30B-001 / Phase 7 sub-track A: legacy `Mold[T] => Foo[T]` syntax fixture.
//
// `taida upgrade --e30 examples/quality/e30b_001_unified_class_like/legacy_mold_pre.td`
// drops the `Mold[T] => ` prefix. NB: the instantiation contract differs
// between Mold (`Box[1, "apple"]()`) and plain class-like
// (`Box(label <= "apple")`). This fixture exercises only the legacy form;
// the migrated counterpart (`migrated_class_like_post.td`) uses the new
// instantiation contract on the same data shape.

Mold[T] => Box[T] = @(
  label: Str,
  transform: T => :T
)
b <= Box[1, "apple"]()
stdout(b.label)
