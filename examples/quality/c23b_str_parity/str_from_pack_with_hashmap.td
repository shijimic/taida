// C23B-003 reopen 2: BuchiPack field values must recurse through the
// full-form helper so a HashMap inside a user pack renders as
// `@(__entries <= …, __type <= "HashMap")` on all 4 backends. Before
// the reopen-2 fix, `taida_pack_to_display_string_full` /
// `_wasm_pack_to_string_full` dispatched nested typed values through
// the short-form debug helper and collapsed the HashMap to the
// `HashMap({…})` form.
hm <= hashMap().set("k", 1)
p <= @(inner <= hm)
stdout(Str[p]())
