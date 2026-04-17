// C18B-003 fix: two distinct Enums sharing the same field name
// (`state`) must emit their own variant-name Str in `jsonEncode`.
// Pre-fix Native mis-keyed the enum descriptor by field name only,
// so the second registration overwrote the first and both packs
// printed the wrong variant label.

Enum => BuildState = :Queued :Built
Enum => RunState = :Idle :Running

// Case 1: anonymous BuchiPack literals with the same field name but
// different enum types. Pre-fix Native printed the wrong variant for
// the first pack after the second registration landed.
a <= @(state <= BuildState:Built())
b <= @(state <= RunState:Idle())

stdout(jsonEncode(a))
stdout(jsonEncode(b))

// Case 2: same scenario but through TypeDef construction. Exercises
// the per-pack enum registration emitted by `lower_type_instantiation`.
BuildRecord = @(state: BuildState, label: Str)
RunRecord = @(state: RunState, label: Str)

c <= BuildRecord(state <= BuildState:Queued(), label <= "pending")
d <= RunRecord(state <= RunState:Running(), label <= "live")

stdout(jsonEncode(c))
stdout(jsonEncode(d))

// Case 3: swap registration order to guard against first-wins vs
// last-wins bias in the global field registry.
e <= @(state <= RunState:Running())
f <= @(state <= BuildState:Queued())

stdout(jsonEncode(e))
stdout(jsonEncode(f))
