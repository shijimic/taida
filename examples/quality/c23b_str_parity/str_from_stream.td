// C23B-003 reopen guard: `Str[Stream[...]()]()` must return
// `"Stream[completed: N items]"` (interpreter `Value::Stream` carries its
// own `to_display_string` shape, not a BuchiPack full form). Pre-fix, JS
// fell through to the plain-pack branch and rendered the Stream's method
// source bodies as pack fields. Native / WASM don't support Stream
// lowering (`unsupported mold type: Stream`), so this fixture is exercised
// by interpreter + JS only via `STREAM_ONLY_FIXTURES`.
s <= Stream[@[1, 2, 3]]()
stdout(Str[s]())
