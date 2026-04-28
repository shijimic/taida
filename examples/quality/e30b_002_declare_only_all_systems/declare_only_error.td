// E30B-002 / E30 Phase 4: declare-only function field on an Error variant.
//
// `recovery: Unit => :Unit` is a declare-only function field representing
// a recovery hook. Phase 4 (E30B-002) accepts the field at definition
// and excludes it from the required positional/named binding set, so
// `NotFound(msg <= "missing")` instantiates without supplying `recovery`.
// Phase 6 (E30B-004) will replace the `Value::Unit` placeholder with an
// auto-generated `defaultFn`; this fixture does not call
// `err.recovery()` so it remains parity-safe across backends.
Error => NotFound = @(
  msg: Str,
  recovery: Unit => :Unit
)
err <= NotFound(msg <= "missing")
stdout(err.msg)
