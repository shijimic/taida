// RC-1k: Export filter test — transitive dependency
// Imports `publicUsesPrivate` which internally calls `_private_helper`.
// The WASM inliner should include `_private_helper` (needed transitively)
// but NOT `_another_private` (not needed by anything imported).

>>> ./lib.td => @(publicUsesPrivate)

result <= publicUsesPrivate(5)
stdout(result.toString())
