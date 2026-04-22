// C23B-003 reopen guard: `Str[setOf(@[...])]()` must return the
// interpreter's synthetic full-form pack `@(__items <= @[...], __type <=
// "Set")` on all 4 backends. Pre-fix, JS rendered method source bodies,
// and native/wasm rendered the short-form `Set({...})`.
s <= setOf(@[1, 2, 3])
stdout(Str[s]())
