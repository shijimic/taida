// C20-1 (ROOT-5 / C19B-009): multi-line `| cond |> body` on the
// right-hand side of `<=` must be rejected with `[E0303]` instead
// of silently swallowing `processResult(name)`.
//
// The canonical escape hatch is to wrap the branch in parentheses
// (which restores `CondBranchContext::TopLevel`), or to extract a
// helper function, or to rewrite as `If[cond, then, else]()`.

Ctx = @(mode: Str)

processResult name =
  "processed: " + name
=> :Str

runFlow ctx: Ctx =
  name <=
    | ctx.mode == "A" |> "foo"
    | _ |> "bar"

  processResult(name)
=> :Str

stdout(runFlow(Ctx(mode <= "A")))
