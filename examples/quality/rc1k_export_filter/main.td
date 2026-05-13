// RC-1k: Export filter test — main module
// Imports only `publicFn` from lib.td.
// The WASM inliner should NOT include `_another_private` from lib.td.
// `_private_helper` should also NOT be included since `publicFn` does not use it.

>>> ./lib.td => @(publicFn)

result <= publicFn(41)
stdout(result.toString())
