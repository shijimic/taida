// C25B-028: `jsonEncode(Gorillax[42]())` — success branch must render
// with all three visible fields (`__error`, `__value`, `hasValue`) and
// `__error` as an empty object (Value::Unit → `{}`).
g <= Gorillax[42]()
stdout(jsonEncode(g))
