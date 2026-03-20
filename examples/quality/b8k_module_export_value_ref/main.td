// Quality Test: B-8k -- export value referenced by sibling function
// Issue 1: export function that references a sibling top-level variable

>>> ./helper.td => @(add, base)

stdout(base.toString())
stdout(add(4).toString())
stdout(add(10).toString())

// Expected: 3
// Expected: 7
// Expected: 13
