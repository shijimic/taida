// C27B-021 (2026-04-25): Float Mod by zero -> Lax empty.
a <= 5.0
b <= 0.0
c <= Mod[a, b]()
stdout(c.hasValue.toString())
