// === Taida Lang: Gorillax ===
//
// Gorillax[T] is a protected container that wraps values.
// Cage[subject, runner]() executes a CageRilla[Branch, Out] capability
// (e.g. JSRilla / FileRilla) on Molten data and returns Gorillax[Out].
//
// NOTE: Cage requires a subject with a resolved Molten branch and a runner
// of the matching CageRilla subfamily. Since Molten values cannot be
// generated in the interpreter (npm imports are JS-backend only), Cage tests
// are in unit tests only. This example demonstrates Gorillax without Cage.

// Direct Gorillax creation (success case)
Gorillax[42]() => g
stdout(g.hasValue())
g ]=> num
stdout(num)

// Gorillax toString
stdout(g.toString())

// Gorillax with relax (success case)
Gorillax[99]() => g2
g2.relax() => relaxed
stdout(relaxed.hasValue())
relaxed ]=> val
stdout(val)

// RelaxedGorillax toString
stdout(relaxed.toString())

// Gorillax is_empty (success case)
stdout(g.isEmpty())
