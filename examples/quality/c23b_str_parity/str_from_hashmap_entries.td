// C23B-009 (2026-04-22): HashMap.entries() must emit `@(key, value)` pairs
// across all four backends. Previously:
//   - JS: `@(first <= …, second <= …)` (legacy zip-style convention)
//   - Native / WASM: `@()` (field names not registered)
// Interpreter (`src/interpreter/methods.rs:761-783`) and
// `docs/reference/standard_library.md:238` both document `@[@(key, value)]`.
m <= hashMap()
m1 <= m.set("a", 1)
m2 <= m1.set("b", 2)
stdout(Str[m2.entries()]())
