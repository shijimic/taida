// D28B-013: Peak-RSS smoke fixture — list allocation + fold.
//
// Purpose: 16-element list allocation + Fold to drive the Cluster 4
// Arc / try_unwrap COW refcount path through `taida_release` once.
// The element count is deliberately small — no list-generator helper
// is in the prelude surface as of @d.X, and the gate's job is to
// detect *regression in the COW path itself*, not to amplify the
// list size. Peak RSS will therefore be dominated by the parser +
// interpreter footprint (a few MiB) plus one round of List COW;
// regressions in COW will show as a step change against the
// baseline EWMA, not a list-size-proportional curve.
//
// D28B-027 fixed an earlier comment that read "peak RSS dynamics
// here come from the parser + interpreter footprint, not list
// element count" — accurate but misleading because the fixture
// name contains "list". The intent is List COW path coverage; the
// constant element count is an *intentional* design choice.
//
// Expected stdout: a single integer (sum), value not load-bearing.

nums <= @[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
Fold[nums, 0, _ acc x = acc + x]() ]=> total
stdout(total)
