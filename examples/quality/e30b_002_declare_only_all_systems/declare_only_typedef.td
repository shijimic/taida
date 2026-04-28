// E30B-002 / E30 Phase 4: declare-only function field on a class-like
// (TypeDef) variant. Pre-existing behaviour, regression guard.
//
// Phase 4 pins that this surface keeps working unchanged after the
// Mold / Inheritance variants gain the same capability.
Pilot = @(name: Str, greet: Str => :Str)
p <= Pilot(name <= "Rei")
stdout(p.name)
