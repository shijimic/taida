// C25B-028: `jsonEncode(Lax[42]())` — Int Lax success. `__default` must
// render as `0` (Int zero), not `{}` (Unit empty), and `hasValue` as
// `true` (Bool), not `1` (Int).
l <= Lax[42]()
stdout(jsonEncode(l))
