// C25B-012 / C25B-029: locally-bound addon-Value::Record read path.
// C25B-029 (Phase 5 Must Fix) addresses a performance regression
// where the interpreter deep-cloned `Value::Record` on every read
// inside a hot loop, and the parity target was that the same 3-way
// rendering continued to work semantically. This fixture pins that
// a nested pack read under a loop produces identical output on all
// three backends, even if the interpreter's storage strategy flips
// from deep-clone to Rc-shared later.
cell <= @(text <= "*", row <= 0, col <= 0)
row <= @[cell, cell, cell]
grid <= @[row, row]
stdout(grid.length().toString())
stdout(grid.get(0).getOrDefault(@[]).length().toString())
stdout(grid.get(0).getOrDefault(@[]).get(1).getOrDefault(@(text <= "?", row <= 0, col <= 0)).text)
