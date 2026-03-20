// Test: export function depending on sibling value
// Case 1: only function imported (sibling value not imported)
>>> ./helper_val.td => @(add)
stdout(add(4).toString())
