//! Name resolution for calls: a name the program binds itself — a function,
//! a parameter, a local — shadows the prelude / core-package builtin of the
//! same name, and an inner binding shadows an outer function. Every backend
//! resolves the call the way the checker types it.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use common::unique_temp_dir;
use f65_support::{ALL_COMPILED, parity, parity_in};

#[test]
fn user_functions_named_like_os_functions_are_called() {
    let dir = unique_temp_dir("f65_os_named_user_fn");
    std::fs::write(dir.join("victim.txt"), "precious").unwrap();
    parity_in(
        &dir,
        r#"remove name: Str =
  "kept " + name
=> :Str

run n: Int =
  n * 2
=> :Int

rename a: Str b: Str =
  a + "->" + b
=> :Str

stdout(remove("victim.txt"))
stdout(run(21))
stdout(rename("a", "b"))
"#,
        "kept victim.txt\n42\na->b",
        ALL_COMPILED,
    );
    // The program never imported taida-lang/os; nothing may be removed.
    assert_eq!(
        std::fs::read_to_string(dir.join("victim.txt")).unwrap(),
        "precious"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn user_functions_named_like_prelude_functions_are_called() {
    parity(
        "f65_prelude_named_user_fn",
        r#"nowMs =
  1234567890
=> :Int

range a: Int b: Int =
  b - a
=> :Int

typeOf x: Int =
  x + 1000
=> :Int

debug x: Int =
  x * 3
=> :Int

exit x: Int =
  x + 1
=> :Int

jsonEncode v: Int =
  "mine"
=> :Str

stdout(nowMs())
stdout(range(2, 10))
stdout(typeOf(7))
stdout(debug(5))
stdout(exit(41))
stdout(jsonEncode(1))
5 => debug => r
stdout(r)
"#,
        "1234567890\n8\n1007\n15\n42\nmine\n15",
        ALL_COMPILED,
    );
}

#[test]
fn parameter_named_like_a_builtin_is_called() {
    parity(
        "f65_param_named_builtin",
        r#"apply range: Int => :Int =
  range(3)
=> :Int

stdout(apply(_ x: Int = x * 10))
"#,
        "30",
        ALL_COMPILED,
    );
}

#[test]
fn parameters_and_locals_shadow_user_functions() {
    parity(
        "f65_param_shadows_fn",
        r#"total a: Int b: Int =
  a + b
=> :Int

scale total: Int k: Int =
  total * k
=> :Int

count n: Int =
  n
=> :Int

summary n: Int =
  count <= n * 2
  count + 1
=> :Int

double x: Int =
  x * 2
=> :Int

twice double: Int => :Int v: Int =
  double(v)
=> :Int

stdout(scale(3, 4))
stdout(summary(5))
stdout(twice(_ x: Int = x + 1000, 5))
stdout(double(5))
stdout(total(1, 2))
"#,
        "12\n11\n1005\n10\n3",
        ALL_COMPILED,
    );
}

#[test]
fn aliased_core_imports_reach_the_builtin() {
    parity(
        "f65_core_import_alias",
        r#">>> taida-lang/os => @(writeFile => save, Exists => Present)

describe flag: Bool =
  | flag |> "present"
  | _ |> "missing"
=> :Str

save("alias_out.txt", "x")
Present["alias_out.txt"]() >=> there
stdout(describe(there))
"#,
        "present",
        &["native", "wasm-wasi"],
    );
}

#[test]
fn pipeline_stage_naming_a_function_value_applies_it() {
    parity(
        "f65_pipe_function_value",
        r#"f <= _ x: Int = x * 10
5 => f => stdout(_.toString())

twice x: Int =
  x * 2
=> :Int

applyThen g: Int => :Int v: Int =
  v => g => stdout(_.toString())
  v
=> :Int

applyThen(twice, 7)
x <= 5
3 => x => stdout(x)
"#,
        "50\n14\n3",
        ALL_COMPILED,
    );
}

#[test]
fn user_names_never_meet_the_compilers_own_bindings() {
    // Partial application and `_` in a pipeline stage keep values under
    // names of their own; a top-level variable or a parameter spelled like
    // one of those keeps its own value.
    parity(
        "f65_hidden_binding_names",
        r#"add x: Int y: Int = x + y => :Int
viaParam __pa_cap_1: Int =
  m <= add(, 2)
  m(__pa_cap_1) + __pa_cap_1
=> :Int
__pa_0 <= 1000
__pa_cap_0 <= 2000
g <= add(__pa_0, )
stdout(g(1))
h <= add(, __pa_cap_0)
stdout(h(1))
f <= add
__pa_callee <= 7
k <= f(1, )
stdout(k(2))
stdout(__pa_callee)
__pipe_prev <= 100
5 => add(_, 1) => add(_, 2) => y
stdout(y)
stdout(__pipe_prev)
stdout(viaParam(40))
"#,
        "1001\n2001\n3\n7\n8\n100\n82",
        ALL_COMPILED,
    );
}

#[test]
fn prelude_functions_used_as_values_show_as_functions() {
    // A prelude function used as a value is a function value: it shows as
    // `<function name>` wherever a user function does, also after being
    // bound to a name.
    parity(
        "f65_prelude_function_values",
        r#"double x: Int = x * 2 => :Int
lam <= _ x: Int = x + 1
stdout(jsonEncode)
stdout(`t ${double} ${lam} ${jsonEncode}`)
Str[stdout]() >=> s
stdout(s)
stdout(@(a <= jsonEncode, b <= double))
stdout(@[jsonEncode, stdout])
g <= jsonEncode
stdout(g)
h <= g
stdout(`${h}`)
d <= debug(g)
stdout(g(@(n <= 1)))
"#,
        "<function jsonEncode>\nt <function double> <function <lambda>> <function jsonEncode>\n<function stdout>\n@(a <= <function jsonEncode>, b <= <function double>)\n@[<function jsonEncode>, <function stdout>]\n<function jsonEncode>\n<function jsonEncode>\n<function jsonEncode>\n{\"n\":1}",
        ALL_COMPILED,
    );
}
