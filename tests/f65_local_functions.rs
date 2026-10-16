//! Functions defined inside functions, lambdas bound to names, and partial
//! application of function values behave the same on every backend: a local
//! function is visible only in the body that defines it, may call itself and
//! its siblings, and reads the enclosing variables and top-level values. A
//! binding inside a lambda or a local function is local to it, even when a
//! top-level value has the same name.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use f65_support::{ALL_COMPILED, parity};

#[test]
fn local_functions_recurse_and_call_each_other() {
    parity(
        "f65_local_recursion",
        r#"countdown n: Int =
  down k: Int =
    | k == 0 |> 0
    | _ |> down(k - 1)
  => :Int
  down(n)
=> :Int

fact n: Int =
  go k: Int =
    | k < 2 |> 1
    | _ |> k * go(k - 1)
  => :Int
  go(n)
=> :Int

parity n: Int =
  ev i: Int =
    | i == 0 |> true
    | _ |> od(i - 1)
  => :Bool
  od i: Int =
    | i == 0 |> false
    | _ |> ev(i - 1)
  => :Bool
  ev(n)
=> :Bool

scaled n: Int step: Int =
  walk k: Int acc: Int =
    | k == 0 |> acc
    | _ |> walk(k - 1, acc + step)
  => :Int
  walk(n, 0)
=> :Int

stdout(countdown(3))
stdout(fact(5))
stdout(parity(4))
stdout(scaled(4, 3))
"#,
        "0\n120\ntrue\n12",
        ALL_COMPILED,
    );
}

#[test]
fn local_functions_are_scoped_to_their_body() {
    parity(
        "f65_local_scope",
        r#"helper x: Int =
  x + 1
=> :Int

outerA n: Int =
  helper x: Int =
    x * 100
  => :Int
  helper(n)
=> :Int

outerB n: Int =
  helper x: Int =
    x - 1
  => :Int
  helper(n)
=> :Int

base <= 100

outerC n: Int =
  inner m: Int =
    m + n + base
  => :Int
  inner(1)
=> :Int

stdout(outerA(5))
stdout(outerB(5))
stdout(helper(5))
stdout(outerC(2))
"#,
        "500\n4\n6\n103",
        ALL_COMPILED,
    );
}

#[test]
fn a_local_function_is_called_by_its_name_from_its_definition_on() {
    // Before the definition, the body calls the top-level function of the
    // same name; the local functions themselves see each other throughout.
    parity(
        "f65_local_order",
        r#"h x: Int =
  x * 100
=> :Int

plain n: Int =
  a <= h(n)
  h x: Int =
    x + 1
  => :Int
  b <= h(n)
  a + b
=> :Int

captured n: Int step: Int =
  a <= h(n)
  h x: Int =
    x + step
  => :Int
  b <= h(n)
  a + b
=> :Int

stdout(plain(1))
stdout(captured(1, 5))
"#,
        "102\n106",
        ALL_COMPILED,
    );
}

#[test]
fn partial_application_of_function_values() {
    parity(
        "f65_partial_values",
        r#"add <= _ a: Int b: Int = a + b
add5 <= add(5, )
stdout(add5(10))

run2 =
  j <= 5
  addk <= _ x: Int y: Int = x + y + j
  p <= addk(1, )
  p(2)
=> :Int
stdout(run2())
"#,
        "15\n8",
        ALL_COMPILED,
    );
}

#[test]
fn lambdas_call_themselves_by_their_binding_name() {
    parity(
        "f65_lambda_self_call",
        r#"f <= _ n: Int acc: Str = (| n < 1 |> acc | _ |> f(n - 1, acc + "x"))
stdout(f(3, ""))

g n: Int =
  down <= _ k: Int = (| k < 1 |> n | _ |> down(k - 1))
  down(4)
=> :Int
stdout(g(7))
"#,
        "xxx\n7",
        ALL_COMPILED,
    );
}

#[test]
fn tail_recursion_under_an_error_ceiling_does_not_nest_ceilings() {
    parity(
        "f65_ceiling_tail_recursion",
        r#"walk n: Int acc: Int =
  |== e: Error =
    n * 1000 + acc
  => :Int
  | n == 3 |> Error(type <= "Error", message <= "stop").throw()
  | n == 0 |> acc
  | _ |> walk(n - 1, acc + 1)
=> :Int

total n: Int acc: Int =
  |== e: Error =
    -1
  => :Int
  | n == 0 |> acc
  | _ |> total(n - 1, acc + n)
=> :Int

stdout(walk(10, 0))
stdout(walk(2, 0))
stdout(walk(5000, 0))
stdout(total(5000, 0))
"#,
        "3007\n2\n7997\n12502500",
        &["native", "wasm-wasi"],
    );
}

#[test]
fn a_piped_call_in_tail_position_is_a_tail_call() {
    // The last stage `f(_)` of a pipeline in tail position is the call
    // `f(piped value)` in tail position: deep enough to overflow otherwise.
    parity(
        "f65_piped_tail_call",
        r#"countdown n: Int =
  | n == 0 |> 0
  | _ |> n - 1 => countdown(_)
=> :Int

sumTo n: Int acc: Int =
  | n == 0 |> acc
  | _ |> n - 1 => sumTo(_, acc + n)
=> :Int

guarded n: Int =
  |== e: Error =
    -1
  => :Int
  | n == 0 |> 0
  | _ |> n - 1 => guarded(_)
=> :Int

inc x: Int =
  x + 1
=> :Int

twice n: Int =
  n => inc(_) => inc(_)
=> :Int

stdout(countdown(100000))
stdout(sumTo(100000, 0))
stdout(guarded(100000))
stdout(twice(1))
"#,
        "0\n5000050000\n0\n3",
        &["native", "wasm-wasi"],
    );
}

#[test]
fn bindings_in_lambdas_and_closures_stay_local() {
    parity(
        "f65_local_bindings_stay_local",
        r#"total <= 100

useGlobal n: Int =
  n + total
=> :Int

calc <= _ x: Int =
  total <= x * 2
  total + 1

outer k: Int =
  inner x: Int =
    total <= x * k
    total + 1
  => :Int
  inner(5)
=> :Int

stdout(useGlobal(1))
stdout(calc(5))
stdout(useGlobal(1))
stdout(outer(2))
stdout(useGlobal(1))
stdout(total)
"#,
        "101\n11\n101\n11\n101\n100",
        ALL_COMPILED,
    );
}

#[test]
fn omitted_arguments_through_aliases_and_enclosing_defaults() {
    parity(
        "f65_alias_defaults",
        r#"add3 a: Int b: Int <= 10 c: Int <= a + 100 =
  a + b + c
=> :Int
f <= add3
stdout(f(1))
stdout(f(1, 2))
g <= f
stdout(g(5))
outer n: Int =
  inner x: Int y: Int <= n =
    x + y
  => :Int
  h <= inner
  inner(1) + h(2)
=> :Int
stdout(outer(40))
"#,
        "112\n104\n120\n83",
        ALL_COMPILED,
    );
}
