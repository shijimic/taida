// D28B-009 (2026-04-26, wD Round 1) — ±Infinity arithmetic, pinned
// across all 4 backends. Interpreter is the reference.
//
// Root cause: pre-fix wasi `taida_mod_mold_f` returned Lax-empty for
// any non-finite divisor, so `Mod[1.5, +Inf]` rendered "0.0" while
// native rendered "1.5".  After fix wasi mirrors libc fmod (= native).

// (1) +Infinity from overflow division.
Div[1e308, 1e-308] >=> big
big_value: Float <= big
debug(big_value)

// (2) -Infinity from negative overflow.
Div[-1e308, 1e-308] >=> negbig
negbig_value: Float <= negbig
debug(negbig_value)

// (3) Mod[finite, +Inf] = finite (the dividend itself, sign preserved).
Mod[1.5, big_value] >=> m_pos
debug(m_pos)

// (4) Mod[finite, -Inf] = finite (still the dividend).
Mod[2.5, negbig_value] >=> m_neg
debug(m_neg)

// (5) Inf + finite = Inf.
plus_inf <= big_value + 1.0
debug(plus_inf)

// (6) -Inf - finite = -Inf.
minus_inf <= negbig_value - 1.0
debug(minus_inf)
