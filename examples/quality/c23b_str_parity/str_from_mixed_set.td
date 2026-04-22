// C23B-007 follow-on: Sets share the list header layout in both
// native and wasm. A mixed Set `setOf(@[1, "a", 2])` must emit each
// member through its own primitive formatter, not force every member
// through the tag-dispatched fast path for the last-inserted primitive
// tag. The HETEROGENEOUS(-2) latch in `taida_list_set_elem_tag` covers
// Sets because `taida_set_add` / `taida_set_from_list` propagate the
// elem tag via the same setter on both backends.
s <= setOf(@[1, "a", 2])
stdout(Str[s]())
