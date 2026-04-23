// C25B-033: user FuncDef named `Join` collides with the prelude list-mold
// `Join(list, sep)` on the JS backend. Pre-fix, node emitted
// `SyntaxError: Identifier 'Join' has already been declared`; interpreter,
// native, and wasm-wasi returned the correct `hi-world`.
//
// Post-fix: JS mangles the user FuncDef to `_td_user_Join` so the prelude
// helper stays intact and user surface semantics are preserved. Output must
// match the interpreter reference across all 4 backends.
sep <= "-"
Join a b =
  `${a}${sep}${b}`
=> :Str

stdout(Join("hi", "world"))
