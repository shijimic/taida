// C23B-003 reopen guard: `Str[hashMap()...]()` must return the interpreter's
// synthetic full-form pack `@(__entries <= @[@(key <= "a", value <= 1)],
// __type <= "HashMap")` on all 4 backends. The JS runtime previously
// rendered method source bodies via the plain-pack fallback; native/wasm
// previously rendered the short-form `HashMap({"a": 1})` via the
// `.toString()`-style helper.
m <= hashMap().set("a", 1)
stdout(Str[m]())
