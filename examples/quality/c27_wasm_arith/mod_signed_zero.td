// C27B-021 wD Round 1 review (Critical 2 numeric coverage):
// Mod[-0.0, 1.0]() preserves signed zero per IEEE 754 fmod sign rule.
// Reference: -0.0
Mod[-0.0, 1.0]() ]=> r
debug(r)
