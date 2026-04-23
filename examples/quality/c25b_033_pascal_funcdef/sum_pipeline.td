// C25B-033: pipeline-form reference to a user FuncDef whose Taida-level
// name (`Sum`) collides with the prelude list-mold `Sum(list)`. The pipeline
// fall-through in `gen_pipeline` emits `{name}(__p)` which pre-fix would
// produce a reference to the prelude `Sum`; post-fix it resolves to the
// mangled `_td_user_Sum(__p)` so the call lands on the user's definition.
Sum a b =
  a + b
=> :Int

stdout(Sum(5, 3))
