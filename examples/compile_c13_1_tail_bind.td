// C13-1: expression-block tail-binding semantics.
//
// Under C13-1 the tail statement of a `| |>` arm body, a function
// body, or an error-ceiling body may be a binding
// (`name <= expr`, `expr => name`, `expr ]=> name`, `name <=[ expr`).
// The bound value becomes the block's result, so a redundant trailing
// `name` line is no longer required.
//
// This example exercises every legal form across all three backends
// so that `tests/parity.rs` picks it up automatically.
//
// See docs/guide/07_control_flow.md for the full rule.

// ── 1. arm body, tail `name <= expr` ─────────────────────────
armTailAssign x =
  | x > 0 |>
    doubled <= x * 2
    result <= doubled + 1
  | _ |> 0
=> :Int

stdout(armTailAssign(5).toString())

// ── 2. arm body, tail `expr => name` ─────────────────────────
armTailForward x =
  | x > 0 |>
    x * 2 => doubled
  | _ |> 0
=> :Int

stdout(armTailForward(7).toString())

// ── 3. arm body, tail `expr ]=> name` (unmold forward) ──────
armTailUnmold x =
  | x > 0 |>
    lax <= Lax[x * 10]()
    lax ]=> n
  | _ |> 0
=> :Int

stdout(armTailUnmold(3).toString())

// ── 4. function body, tail `expr => name` ────────────────────
funcTailForward =
  1 => one
  one + 2 => total
=> :Int

stdout(funcTailForward().toString())

// ── 5. function body, tail `name <= expr` ────────────────────
funcTailAssign x y =
  total <= x + y
  shifted <= total + 1
=> :Int

stdout(funcTailAssign(10, 20).toString())

// ── 6. error-ceiling body, tail `name <= expr` ──────────────
errorCeilingTail =
  |== err: Error =
    fallback <= "caught"
  => :Str
  Error(type <= "Error", message <= "boom", code <= 1).throw()
=> :Str

stdout(errorCeilingTail())

// ── 7. pipeline middle bind-and-forward ─────────────────────
add x: Int y: Int = x + y => :Int

1 => add(3, _) => addResult => stdout(addResult.toString())

// Expected output:
// 11
// 14
// 30
// 3
// 31
// caught
// 4
