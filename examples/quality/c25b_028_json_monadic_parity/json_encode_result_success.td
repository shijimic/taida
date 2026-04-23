// C25B-028: `jsonEncode(Result[42]())` — Result success branch. Must
// include `__predicate: {}`, `throw: {}`, `__value: 42` (no `hasValue`,
// since Result uses predicate+throw rather than hasValue).
r <= Result[42]()
stdout(jsonEncode(r))
