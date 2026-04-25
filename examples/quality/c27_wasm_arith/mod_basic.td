// C27B-021 (2026-04-25): Float Mod basic.
a <= 7.0
b <= 3.0
c <= Mod[a, b]()
stdout(c.hasValue.toString())
