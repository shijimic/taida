// C23B-003 reopen guard: `Str[Gorillax[v]()]()` must return the
// interpreter's full-form `@(hasValue <= true, __value <= v, __error <=
// @(), __type <= "Gorillax")`. Pre-fix, JS rendered method source bodies;
// native's `taida_pack_to_display_string_full` skipped every field because
// `__error` wasn't in the name registry (printed `@()`). WASM still uses
// a different hash (`isOk` vs `hasValue`) for Gorillax construction — that
// is a pre-existing divergence tracked outside C23 scope, so this fixture
// is exercised by interpreter / JS / native only (`c23_str_parity::wasm_wasi_matches_interpreter`
// deliberately excludes it via `WASM_SKIP_FIXTURES`).
g <= Gorillax[42]()
stdout(Str[g]())
