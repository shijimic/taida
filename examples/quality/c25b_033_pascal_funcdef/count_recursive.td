// C25B-033: recursive user FuncDef whose name (`Count`) collides with the
// prelude `Count(list, fn)` list-mold. Exercises the JS trampoline /
// tail-call path (`__inner_Count` helper + `const Count = __taida_trampoline(__inner_Count)`)
// which must also be emitted under the mangled name `_td_user_Count` so
// the prelude helper declaration at the top of the runtime stays intact.
//
// Interpreter / native / wasm-wasi accept PascalCase user FuncDef names
// unconditionally — no mangling is required there. Output must match
// across all 4 backends.
Count n: Int acc: Int =
  | n < 1 |> acc
  | _ |> Count(n - 1, acc + n)
=> :Int

stdout(Count(10, 0))
