// compile_c12_11_tag_prop.td — C12-11 (FB-1) regression fixture
//
// Exercises `param_tag_vars` propagation so that Bool values flowing
// through a user function parameter (or returned from a user function)
// are displayed as "true"/"false" on the Native backend instead of
// "1"/"0". Before C12-11, the stdout codegen at the call-site was the
// only place where the Bool tag was consulted, so a parameter with an
// unknown compile-time type dropped through to `convert_to_string`
// which used the polymorphic path and miscategorised Bool as Int.
//
// C12-11 extends the tagged-stdout path with two new cases:
//   (a) Ident whose name is a parameter in `param_tag_vars` — the
//       caller's compile-time tag is available as a runtime IrVar.
//   (b) FuncCall to a user function — the callee's return tag is
//       available via `taida_get_return_tag` / `return_tag_vars`.
// Combined with body-based Bool inference for `bool_returning_funcs`,
// user-function return values captured in `<=` bindings also preserve
// the Bool tag.
//
// 3 backend (interp / JS / native) must produce the same output.

// ── (a) direct Bool through user func param ─────────────────
printAny v =
    stdout(v)
printAny(true)
printAny(false)

// ── (a) Bool-returning MoldInst through user func param ─────
// (TypeIs returns Bool per mold_returns.rs; emit_call_arg_tags
//  propagates tag=2 into the callee's param_tag_vars slot.)
printAny(TypeIs[42, :Int]())

// ── (b) user func returning Bool consumed directly by stdout ─
isIntReturning v =
    TypeIs[v, :Int]()
stdout(isIntReturning(42))

// ── let-binding of Bool-returning user func result ──────────
// (body-based inference adds `isIntReturning` to
//  bool_returning_funcs so `b <= isIntReturning(42)` tags `b`
//  as Bool and `stdout(b)` routes through the tagged path.)
b1 <= isIntReturning(42)
stdout(b1)
