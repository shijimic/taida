// C25B-028: `jsonEncode(Result[42]())` — Result success branch. Must
// include `__predicate: {}`, `__value: 42` and `throw` as the empty Error
// (`{"message":"","type":""}`), with no `has_value`, since Result uses
// predicate+throw rather than hasValue.
r <= Result[42]()
stdout(jsonEncode(r))
