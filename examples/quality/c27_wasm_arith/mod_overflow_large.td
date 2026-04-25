// C27B-021 wD Round 1 review (Critical 1 regression guard):
// Mod[1.0e20, 7.0]() requires correct fmod across all 4 backends.
// Pre-fix wasm produced `0` (silent wrong) because the single-pass
// `(int64_t)(a/b) * b` lost precision.
// Reference (interp / native / js): 2.0
Mod[1.0e20, 7.0]() ]=> r
debug(r)
