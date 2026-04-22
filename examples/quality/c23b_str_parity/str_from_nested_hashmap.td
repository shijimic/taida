// C23B-003 reopen 2: nested HashMap-in-HashMap must recurse through the
// full-form helper on all 4 backends. Before the reopen-2 fix, the
// inner HashMap collapsed to the `.toString()` short form
// `HashMap({"a": 1})` on native/wasm (and via `String(v)` on JS),
// breaking parity with the interpreter's `BuchiPack(__entries, __type)`
// recursion.
inner <= hashMap().set("a", 1)
out <= hashMap().set("k", inner)
stdout(Str[out]())
