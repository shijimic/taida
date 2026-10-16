//! Evaluation order on every backend: a condition branch evaluates its
//! guards in order and stops at the first that holds, and `&&` / `||`
//! evaluate their right operand only when the left one does not decide
//! the result. `| true |>` is a default arm, and arms after the default arm
//! are never reached. A constructor evaluates its arguments in the order
//! they are written.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use f65_support::{ALL_COMPILED, parity};

#[test]
fn guards_run_in_order_until_one_holds() {
    parity(
        "f65_guard_order",
        r#"say tag: Str v: Bool =
  stdout("eval " + tag)
  v
=> :Bool

pick n: Int =
  | say("a", n < 0) |> "neg"
  | say("b", n == 0) |> "zero"
  | _ |> "pos"
=> :Str

stdout(pick(-1))
stdout(pick(0))
stdout(pick(1))
"#,
        "eval a\nneg\neval a\neval b\nzero\neval a\neval b\npos",
        ALL_COMPILED,
    );
}

#[test]
fn a_guard_after_a_matching_arm_is_not_evaluated() {
    parity(
        "f65_guard_skipped",
        r#"say tag: Str v: Bool =
  stdout("eval " + tag)
  v
=> :Bool

loopy n: Int =
  | n < 1 |> 0
  | say("rec", n > 100) && loopy(n + 1) > 0 |> 1
  | _ |> loopy(n - 1)
=> :Int

stdout(loopy(3))
"#,
        "eval rec\neval rec\neval rec\n0",
        ALL_COMPILED,
    );
}

#[test]
fn and_or_short_circuit() {
    parity(
        "f65_short_circuit",
        r#"say tag: Str v: Bool =
  stdout("eval " + tag)
  v
=> :Bool

stdout(say("x", false) && say("y", true))
stdout(say("p", true) || say("q", true))
stdout(say("m", true) && say("n", false))
stdout(say("s", false) || say("t", true))

size x: Int =
  | x > 3 || say("never", true) |> "big"
  | _ |> "small"
=> :Str
stdout(size(5))
"#,
        "eval x\nfalse\neval p\ntrue\neval m\neval n\nfalse\neval s\neval t\ntrue\nbig",
        ALL_COMPILED,
    );
}

#[test]
fn true_arms_and_arms_after_the_default_arm() {
    parity(
        "f65_default_arms",
        r#"f x: Int =
  | x > 100 |> "huge"
  | true |> "other"
=> :Str

g x: Int =
  | x > 100 |> "huge"
  | _ |> "other"
  | x > 0 |> "pos"
=> :Str

h x: Int =
  | _ |> "first"
  | _ |> "second"
=> :Str

stdout(f(1000))
stdout(f(1))
stdout(g(1))
stdout(h(1))
"#,
        "huge\nother\nother\nfirst",
        ALL_COMPILED,
    );
}

#[test]
fn constructor_arguments_run_in_the_order_written() {
    // The compiled backends evaluated the given fields in declaration
    // order; omitted fields take their defaults after the given ones.
    parity(
        "f65_ctor_arg_order",
        r#"tick label: Str =
  stdout("eval " + label)
  label.length()
=> :Int

T = @(a: Int, b: Int, c: Int, d: Int <= 9)
x <= T(c <= tick("ccc"), a <= tick("a"), b <= tick("bb"))
stdout(x)
"#,
        "eval ccc\neval a\neval bb\n@(a <= 1, b <= 2, c <= 3, d <= 9, __type <= \"T\")",
        ALL_COMPILED,
    );
}
