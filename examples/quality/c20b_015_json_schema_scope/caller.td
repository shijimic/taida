// C20B-015 / ROOT-18: Caller side.
//
// Imports only `loadUser`, not the private `UserSchema` typedef. If JSON
// schema resolution ever falls back to the caller's scope, the program
// crashes with "Unknown schema type 'UserSchema'". The fix attaches the
// defining-module TypeDef registry to the exported function so the
// schema resolves against the module that owns it.

>>> ./schema_mod.td => @(loadUser)

stdout(loadUser())
stdout("\n")
