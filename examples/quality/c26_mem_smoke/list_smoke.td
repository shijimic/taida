// C26B-010: Memory smoke fixture — list allocation + fold teardown.
//
// Purpose: exercise List / BuchiPack value drop paths so valgrind catches
// regressions in Arc/try_unwrap COW refcount logic (Cluster 4 abstraction,
// C26B-012 / C26B-024 territory). Kept to a 4-element list to stay well
// under valgrind's 10x slowdown budget.
//
// Expected stdout (3-backend identical):
//   10

nums <= @[1, 2, 3, 4]
Fold[nums, 0, _ acc: Int x: Int = acc + x]() >=> total
stdout(total)
