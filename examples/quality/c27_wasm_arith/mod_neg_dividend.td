// C27B-021 wD Round 1 review (Critical 2 numeric coverage):
// Mod[-7.0, 3.0]() preserves sign of dividend (IEEE 754 fmod).
// Reference: -1.0
Mod[-7.0, 3.0]() ]=> r
debug(r)
