// C26B-024 (Round 8 / wT) — 4-field Pack (Lax) churn parity fixture.
//
// Purpose:
//   Pattern this produces is the same shape as `bench_router.td`'s hot
//   loop — repeated `list.get(i) ]=> x` inside a tail-recursive scan —
//   which drove the 80%-sys-time Native regression reported in HI-009
//   (C26B-024). With the thread-local 4-field Pack freelist landed in
//   core.c, the repeated Lax wrapper from `list.get` cycles through
//   the freelist instead of hammering malloc/free.
//
// Parity invariant:
//   Output must be identical across Interpreter / JS / Native. The
//   freelist is a pure allocation-behavioural optimisation with no
//   observable semantics change.
//
// Keep the workload small enough to finish under the default test
// budget — the perf ratio is validated separately via the internal
// micro-bench harness (tests/c26b_024_pack_freelist_*).

scan items: @[Int] target: Int i: Int n: Int hits: Int =
  | i >= n |> hits
  | _ |>
    items.get(i) ]=> v
    | v == target |> scan(items, target, i + 1, n, hits + 1)
    | _ |> scan(items, target, i + 1, n, hits)
=> :Int

buildList acc: @[Int] i: Int n: Int =
  | i >= n |> acc
  | _ |>
    v <= Mod[i, 7]().getOrDefault(0)
    buildList(Append[acc, v](), i + 1, n)
=> :@[Int]

n <= 200
xs <= buildList(@[], 0, n)

// Each call exercises n = 200 Lax wrappers (one per `items.get(i)`).
h0 <= scan(xs, 0, 0, n, 0)
h1 <= scan(xs, 3, 0, n, 0)
h2 <= scan(xs, 6, 0, n, 0)

stdout("hits(0)=" + h0.toString())
stdout("hits(3)=" + h1.toString())
stdout("hits(6)=" + h2.toString())
