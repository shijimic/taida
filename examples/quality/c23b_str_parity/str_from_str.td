// C23B-001 guard: `Str["abc"]()` wrap-in-Lax preserves the original string
// and surrounds it with WASM_TAG_STR fields rendered as `"abc"` / `""`.
stdout(Str["abc"]())
stdout(Str[""]())
