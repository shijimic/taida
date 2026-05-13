// RC-1k: Export filter test — library module
// This module exports `publicFn` but NOT `_private_helper`.
// The WASM inliner must NOT include `_private_helper` in the main module
// unless it is transitively needed by `publicFn`.

_private_helper x: Int =
  x * 100
=> :Int

_another_private y: Str =
  "secret: " + y
=> :Str

publicFn x: Int =
  x + 1
=> :Int

publicUsesPrivate x: Int =
  _private_helper(x) + 1
=> :Int

<<< @(publicFn, publicUsesPrivate)
