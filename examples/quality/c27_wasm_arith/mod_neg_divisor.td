// C27B-021 wD Round 1 review (Critical 2 numeric coverage):
// Mod[7.0, -3.0]() — sign rule says result follows dividend, not
// divisor. fmod(7, -3) == 1.0 (positive).
// Reference: 1.0
Mod[7.0, -3.0]() ]=> r
debug(r)
