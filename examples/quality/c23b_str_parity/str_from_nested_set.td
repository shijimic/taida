// C23B-003 reopen 2: Set-in-HashMap must recurse through the full-form
// helper on all 4 backends. Before the reopen-2 fix, the inner Set
// collapsed to the `.toString()` short form `Set({1, 2})` on
// native/wasm (and `String(v)` short-form on JS), breaking parity with
// the interpreter's `BuchiPack(__items, __type)` recursion.
inner <= setOf(@[1, 2])
out <= hashMap().set("k", inner)
stdout(Str[out]())
