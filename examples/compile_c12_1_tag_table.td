// compile_c12_1_tag_table.td — C12-1 (FB-27) regression fixture
//
// Exercises the mold-return-tag table (`src/types/mold_returns.rs`) by
// printing Str / Int / Float / Bool / List-returning molds directly to
// stdout. Before C12-1, `expr_type_tag()` hardcoded Pack (tag=4) for
// every MoldInst, which surfaced as wrong tag propagation when a
// MoldInst was passed as an argument to a user function. The compile-
// time tag is now authoritative.
//
// 3 backend (interp / JS / native) + 3 wasm profile (min / wasi / full)
// must produce the same output.

// ── Str-returning molds ─────────────────────────────────
stdout(Upper["hi"]())
stdout(Lower["HELLO"]())
stdout(Trim["  pad  "]())
stdout(Repeat["ab", 3]())
stdout(Join[@[1, 2, 3], ","]())

// ── Int-returning molds ─────────────────────────────────
stdout(Floor[3.7]())
stdout(Ceil[3.2]())
stdout(Round[4.5]())
stdout(Truncate[3.9]())

// ── Bool-returning molds (direct stdout; FB-3 path) ─────
stdout(TypeIs[42, :Int]())
stdout(TypeIs["abc", :Int]())

// ── List-returning molds ────────────────────────────────
// NB: Chars / Sort / etc. list molds still trigger wasm-wasi rejects
// (taida_str_chars is core-only) — tested directly in tests/parity.rs
// rather than here so this fixture stays wasm-* compatible.

// ── Pack round-trip through a user function ─────────────
wrap s =
    @(label <= "up", value <= s)
p <= wrap(Upper["hello"]())
stdout(p.value)
stdout(p.label)
