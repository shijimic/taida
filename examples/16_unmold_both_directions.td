// === Taida Lang: >=> vs <=< ===
// Both unmold operators work identically, just in opposite directions

numbers <= @[1, 2, 3, 4, 5]

// >=> (forward): source >=> target
Map[numbers, _ x: Int = x * 3]() >=> tripled
stdout(`>=> tripled: ${tripled}`)

// <=< (backward): target <=< source
doubled <=< Map[numbers, _ x: Int = x * 2]()
stdout(`<=< doubled: ${doubled}`)

// Lax unmold in both directions
lax1 <= Lax[99]()
lax1 >=> val1
stdout(`>=> Lax: ${val1}`)

lax2 <= Lax["world"]()
val2 <=< lax2
stdout(`<=< Lax: ${val2}`)

// Chaining with <=< (right-to-left flow)
result <=< Reduce[@[1, 2, 3, 4, 5], 0, _ acc: Int x: Int = acc + x]()
stdout(`<=< Reduce: ${result}`)

// Chaining with >=> (left-to-right flow)
Filter[numbers, _ x: Int = x > 3]() >=> filtered
stdout(`>=> Filter: ${filtered}`)
