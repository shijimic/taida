// E30 Phase 5 / E30B-003 fixture (Error variant):
// declare-only function field with opaque return type → [E1410]

Error => MyErr = @(
  msg: Str,
  recovery: Unit => :Opaque
)
