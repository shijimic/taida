// C23B-003 reopen 2: List items must recurse through the full-form
// helper so nested HashMaps inside a list produce
// `@(__entries <= …, __type <= "HashMap")` on all 4 backends. Before
// the reopen-2 fix, native/wasm emitted the short-form `HashMap({…})`
// for list items because `taida_list_to_display_string` /
// `_wasm_list_to_string` dispatched through the short-form
// debug-string helper.
out <= @[hashMap().set("a", 1), hashMap().set("b", 2)]
stdout(Str[out]())
