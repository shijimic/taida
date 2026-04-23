// C25B-028: `jsonEncode(Lax[0]())` — Int Lax with zero payload. Both
// `__value` and `__default` must render as `0`, not `{}`.
l <= Lax[0]()
stdout(jsonEncode(l))
