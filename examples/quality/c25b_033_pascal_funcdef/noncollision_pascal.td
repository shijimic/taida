// C25B-033 guardrail: PascalCase user FuncDef that does NOT collide with
// any prelude reserved identifier must be emitted verbatim on JS (no
// mangling) so Taida's "user may name functions freely" surface invariant
// (PHILOSOPHY.md I. / docs/reference/naming_conventions.md) is preserved
// for the common case. Also exercises the 4-backend parity.
MyCustomFunc a b =
  a + b
=> :Int

stdout(MyCustomFunc(2, 3))
