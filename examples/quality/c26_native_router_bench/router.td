// C26B-024 (Round 9 / wγ) — self-contained router micro-bench fixture.
//
// Self-contained (no `>>>` external router dep): mirrors the shape of
// hono-inspired/bench_router.td but collapses router.td inline so this
// fixture can serve as both a 3-backend parity test and an in-tree
// allocation stress test for the Step 4 Arena / Str / slice-view work.
//
// Compared to Round 8 / wT's `lax_churn.td` (200-element inner scan),
// this fixture exercises the *full* router hot path:
//   - split path into segments (String allocation)
//   - linear scan over routes list (per-element Lax wrappers)
//   - nested pattern / path segment comparison (String + Lax)
//   - param extraction via Slice (sub-string allocation)
//   - BuchiPack construction per-iteration (return value)
//
// Parity invariant:
//   Deterministic output across Interpreter / JS / Native — the fixture
//   prints only `acc` sums and hit counters, never timings.

// ── router (collapsed inline) ─────────────────────────────
splitPath p: Str =
  | p == "" |> @[]
  | p == "/" |> @[""]
  | _ |>
    stripped <= | p.startsWith("/") |> Slice[p](start <= 1, end <= p.length()) | _ |> p
    stripped.split("/")
=> :@[Str]

matchSegs pat: @[Str] pth: @[Str] i: Int n: Int =
  | i >= n |> true
  | _ |>
    pat.get(i) ]=> pseg
    pth.get(i) ]=> sseg
    | pseg.startsWith(":") |> matchSegs(pat, pth, i + 1, n)
    | pseg == sseg |> matchSegs(pat, pth, i + 1, n)
    | _ |> false
=> :Bool

matchPattern pattern: Str path: Str =
  patSegs <= splitPath(pattern)
  pthSegs <= splitPath(path)
  | patSegs.length() != pthSegs.length() |> false
  | _ |> matchSegs(patSegs, pthSegs, 0, patSegs.length())
=> :Bool

matchRoutes routes: @[@(method: Str, pattern: Str)] method: Str path: Str i: Int n: Int =
  | i >= n |> -1
  | _ |>
    routes.get(i) ]=> r
    | r.method != method |> matchRoutes(routes, method, path, i + 1, n)
    | _ |>
      | matchPattern(r.pattern, path) |> i
      | _ |> matchRoutes(routes, method, path, i + 1, n)
=> :Int

matchRoute routes: @[@(method: Str, pattern: Str)] method: Str path: Str =
  matchRoutes(routes, method, path, 0, routes.length())
=> :Int

// ── routes builder ────────────────────────────────────────
buildRoutesIter acc: @[@(method: Str, pattern: Str)] i: Int n: Int =
  | i >= n |> acc
  | _ |>
    kind <= Mod[i, 3]().getOrDefault(0)
    pattern <= (| kind == 0 |> "/route/" + i.toString() + "/:id"
                | kind == 1 |> "/api/v1/resource/" + i.toString()
                | _ |> "/users/" + i.toString() + "/:name")
    newRoute <= @(method <= "GET", pattern <= pattern)
    buildRoutesIter(Append[acc, newRoute](), i + 1, n)
=> :@[@(method: Str, pattern: Str)]

buildRoutes n: Int = buildRoutesIter(@[], 0, n) => :@[@(method: Str, pattern: Str)]

// ── bench loop ────────────────────────────────────────────
benchLoop routes: @[@(method: Str, pattern: Str)] method: Str path: Str i: Int m: Int acc: Int =
  | i >= m |> acc
  | _ |>
    r <= matchRoute(routes, method, path)
    benchLoop(routes, method, path, i + 1, m, acc + r)
=> :Int

runScenario routes: @[@(method: Str, pattern: Str)] method: Str path: Str m: Int label: Str =
  acc <= benchLoop(routes, method, path, 0, m, 0)
  stdout(label + " acc=" + acc.toString())
=> :Int

// ── main ──────────────────────────────────────────────────
n50 <= buildRoutes(50)
n200 <= buildRoutes(200)

m <= 500

s1 <= runScenario(n50, "GET", "/route/0/hello", m, "best  N=50 ")
s2 <= runScenario(n50, "GET", "/route/48/x", m, "worst N=50 ")
s3 <= runScenario(n50, "GET", "/noop/zzz", m, "miss  N=50 ")

s4 <= runScenario(n200, "GET", "/route/0/hello", m, "best  N=200")
s5 <= runScenario(n200, "GET", "/route/198/x", m, "worst N=200")
s6 <= runScenario(n200, "GET", "/noop/zzz", m, "miss  N=200")

stdout("done")
