// Phase N-Gorillax: Gorillax/RelaxedGorillax in native backend

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
