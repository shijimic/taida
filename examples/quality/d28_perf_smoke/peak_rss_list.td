// D28B-013: Peak-RSS smoke fixture — list allocation + fold.
//
// Purpose: exercise the List / BuchiPack value path with a 16-element
// allocation + sum so the Cluster 4 Arc/try_unwrap COW refcount path
// is reflected in peak RSS. Intentionally small (no list-generator
// helper is in the prelude surface as of @d.X); peak RSS dynamics
// here come from the parser + interpreter footprint, not list
// element count.
//
// Expected stdout: a single integer (sum), value not load-bearing.

nums <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
Fold[nums, 0, _ acc x = acc + x]() ]=> total
stdout(total)
