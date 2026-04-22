// C23B-009 (2026-04-22): entries() after a remove must walk the insertion-
// order side-index (C23B-008) while still emitting `@(key, value)` pairs.
// Native / wasm iterate `order_array[0..next_ord]` with `slot == -1` null-
// outs for removed entries; JS filters its `__entries` array; interpreter
// uses `Vec<(k,v)>` directly. All four backends must agree byte-for-byte.
m <= hashMap().set("a", 1).set("b", 2).set("c", 3)
m2 <= m.remove("b")
stdout(Str[m2.entries()]())
