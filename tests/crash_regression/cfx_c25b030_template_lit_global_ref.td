// C25B-012 / C25B-030 Phase 1F: pre-existing `TemplateLit` global-lookup
// bug isolated during Phase 1E facade parity verify (see
// .dev/C25_PROGRESS.md 2026-04-23 agent session 5). A top-level binding
// (`sep <= "-"`) referenced only from a FuncDef body's `${sep}`
// template expression used to vanish under native codegen (output
// "hiworld" instead of "hi-world") because `collect_free_vars_inner`
// catch-all dropped `Expr::TemplateLit`. Fixed by updating both the
// codegen free-var walker and the addon facade reachability walker
// to recurse into TemplateLit `${...}` fragments. Pinned here so the
// 3-backend parity cannot silently regress under future codegen
// refactors. The lowercase function name avoids a separate
// pre-existing JS codegen duplicate-identifier bug on PascalCase
// user FuncDef names (see C25B-033, isolated during Phase 9).
sep <= "-"
joiner a b =
  `${a}${sep}${b}`
=> :Str

stdout(joiner("hi", "world"))
