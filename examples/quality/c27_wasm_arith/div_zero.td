// C27B-021 (2026-04-25): Float Div by zero -> Lax empty with FLOAT tag.
// `Div[1.0, 0.0]()` matches native `taida_div_mold_f`'s C26B-011
// semantics: empty Lax (hasValue == false), default 0.0, FLOAT-tagged.
a <= 1.0
b <= 0.0
c <= Div[a, b]()
stdout(c.hasValue.toString())
