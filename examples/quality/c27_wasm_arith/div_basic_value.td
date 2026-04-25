// C27B-021 wD Round 1 review (Critical 2 numeric coverage):
// Bit-pattern parity for `Div[1.0, 3.0]()` — exercises the truncation
// behavior of `taida_float_to_str` across backends.
// Reference: 0.3333333333333333
Div[1.0, 3.0]() ]=> r
debug(r)
