// C25B-033: `Concat` is a prelude list-mold helper. A user FuncDef of the
// same Taida-level name must not clash with the runtime declaration on the
// JS backend. Interpreter / native / wasm accept the user name verbatim.
Concat a b =
  `${a}${b}`
=> :Str

stdout(Concat("a", "b"))
