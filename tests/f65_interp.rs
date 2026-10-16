//! The interpreter is the reference implementation: these tests pin the
//! values it computes for lexical scope, module-owned names and types,
//! prelude functions used as values, error values, and builtins that must
//! not panic. Where the compiled backends already compute the same values,
//! a test checks them too; tests marked "interpreter" pin the reference
//! value the compiled backends have yet to match.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use common::{taida_bin, unique_temp_dir};
use f65_support::{ALL_COMPILED, parity, parity_files};
use std::path::Path;
use std::process::Command;

/// Run `main.td` (plus `files`) on the interpreter; return stdout, stderr
/// and the exit code.
fn interp_files(tag: &str, files: &[(&str, &str)]) -> (String, String, Option<i32>) {
    let dir = unique_temp_dir(tag);
    for (name, source) in files {
        std::fs::write(dir.join(name), source).unwrap();
    }
    let result = run_in(&dir, &[]);
    std::fs::remove_dir_all(dir).unwrap();
    result
}

fn run_in(dir: &Path, flags: &[&str]) -> (String, String, Option<i32>) {
    let output = Command::new(taida_bin())
        .args(flags)
        .arg("main.td")
        .current_dir(dir)
        .output()
        .unwrap();
    (
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code(),
    )
}

fn interp(tag: &str, source: &str) -> (String, String, Option<i32>) {
    interp_files(tag, &[("main.td", source)])
}

/// The interpreter prints `expected` and exits 0.
fn interp_ok(tag: &str, source: &str, expected: &str) {
    let (out, err, code) = interp(tag, source);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert_eq!(out, expected, "stderr: {err}");
}

fn interp_files_ok(tag: &str, files: &[(&str, &str)], expected: &str) {
    let (out, err, code) = interp_files(tag, files);
    assert_eq!(code, Some(0), "stderr: {err}");
    assert_eq!(out, expected, "stderr: {err}");
}

// ── Lexical scope ──────────────────────────────────────────

#[test]
fn method_body_resolves_names_where_the_type_is_declared() {
    // The caller's parameter `rate` must not stand in for the top-level
    // `rate` the method body names.
    parity(
        "f65i_method_lexical",
        r#"rate <= 10

Item = @(
  n: Int
  cost =
    n * rate
  => :Int
)

quote rate: Int =
  it <= Item(n <= 2)
  it.cost()
=> :Int

stdout(quote(99))
"#,
        "20",
        ALL_COMPILED,
    );
}

#[test]
fn callee_names_resolve_where_the_call_is_written() {
    // interpreter: a caller's local function of the same name never
    // replaces the one the callee's body names — in a tail call, in a
    // forward reference, or behind a partial application.
    interp_ok(
        "f65i_tail_callee",
        r#"tax x: Int =
  x + 1
=> :Int

price x: Int =
  tax(x)
=> :Int

price2 x: Int =
  r <= tax(x)
  r
=> :Int

quote n: Int =
  tax y: Int =
    y * 100
  => :Int
  a <= price(n)
  b <= price2(n)
  a * 1000 + b
=> :Int

stdout(quote(1))
"#,
        "2002",
    );
    interp_ok(
        "f65i_forward_ref",
        r#"price x: Int =
  r <= tax(x)
  r
=> :Int

tax x: Int =
  x + 1
=> :Int

quote n: Int =
  tax y: Int =
    y * 100
  => :Int
  a <= price(n)
  a
=> :Int

stdout(quote(1))
"#,
        "2",
    );
    interp_ok(
        "f65i_partial_callee",
        r#"add a: Int b: Int =
  a + b
=> :Int

inc1 <= add(1, )

caller n: Int =
  add x: Int y: Int =
    x * y
  => :Int
  r <= inc1(n)
  r
=> :Int

stdout(caller(5))
"#,
        "6",
    );
}

#[test]
fn top_level_functions_refer_to_each_other_in_any_order() {
    parity(
        "f65i_mutual_toplevel",
        r#"isEven n: Int =
  | n == 0 |> true
  | _ |> isOdd(n - 1)
=> :Bool

isOdd n: Int =
  | n == 0 |> false
  | _ |> isEven(n - 1)
=> :Bool

stdout(isEven(10))
stdout(isOdd(7))
"#,
        "true\ntrue",
        ALL_COMPILED,
    );
}

#[test]
fn local_functions_see_themselves_and_their_siblings() {
    // interpreter: a local function returned from its defining function
    // still calls itself, and local functions call siblings defined after
    // them while the defining function runs.
    interp_ok(
        "f65i_local_recursion",
        r#"mk =
  count n: Int =
    | n >= 3 |> n
    | _ |> count(n + 1)
  => :Int
  count
=> :Int => :Int

c <= mk()
stdout(c(0))

outer n: Int =
  a k: Int =
    | k == 0 |> 0
    | _ |> b(k - 1)
  => :Int
  b k: Int =
    | k == 0 |> 1
    | _ |> a(k - 1)
  => :Int
  a(n)
=> :Int

stdout(outer(5))
"#,
        "3\n1",
    );
}

#[test]
fn condition_arm_bindings_stay_in_their_arm() {
    parity(
        "f65i_arm_scope",
        r#"calc x: Int =
  a <= (
    | x > 0 |>
      tmp <= x * 2
      tmp
    | _ |> 0
  )
  b <= (
    | x > 1 |>
      tmp <= x * 3
      tmp
    | _ |> 0
  )
  a + b
=> :Int

stdout(calc(5))
"#,
        "25",
        ALL_COMPILED,
    );
}

#[test]
fn a_caught_throw_leaves_no_scope_and_no_call_depth_behind() {
    // A throw from a pipeline stage caught by the enclosing ceiling must
    // not leave the pipeline's binding scope in place (the caller would
    // read the wrong `a`, or fail to define `total`).
    parity(
        "f65i_pipe_scope",
        r#"Error => E1 = @(n: Int)
thrower y: Int =
  | y > 0 |> E1(type <= "E1", message <= "boom", n <= y).throw()
  | _ |> y
=> :Int

a <= 1000

f x: Int =
  |== e: Error =
    -1
  => :Int
  x => v => Map[@[1, 2], _ y = thrower(y + v)]() => r
  0
=> :Int

g a: Int =
  first <= f(a)
  a + first
=> :Int

h a: Int =
  first <= f(a)
  total <= first * 2
  total
=> :Int

stdout(g(1))
stdout(h(1))
"#,
        "0\n-2",
        ALL_COMPILED,
    );
    // Arguments that throw inside a caught call do not count toward the
    // depth limit.
    parity(
        "f65i_depth_leak",
        r#"Error => MyErr = @(n: Int)

risky x: Int =
  | x > 0 |> MyErr(type <= "MyErr", message <= "boom", n <= x).throw()
  | _ |> 0
=> :Int

ident x: Int =
  x
=> :Int

safe x: Int =
  |== e: Error =
    0
  => :Int
  ident(risky(x))
=> :Int

Map[range(0, 9000), _ i = safe(1)]() >=> ys
stdout(ys.length())
"#,
        "9000",
        &["native"],
    );
}

// ── Modules own their names and types ──────────────────────

#[test]
fn imported_closures_keep_their_captured_values() {
    parity_files(
        "f65i_import_closure",
        &[
            (
                "lib.td",
                r#"x <= 100

makeAdder x: Int =
  _ y: Int = x + y
=> :Int => :Int

add5 <= makeAdder(5)

<<< @(add5)
"#,
            ),
            (
                "main.td",
                r#">>> ./lib.td => @(add5)
stdout(add5(1))
"#,
            ),
        ],
        "6",
        ALL_COMPILED,
    );
}

#[test]
fn imported_functions_use_their_own_types_enums_and_methods() {
    parity_files(
        "f65i_module_types",
        &[
            (
                "a.td",
                r#"Enum => Mode = :Off :On

Item = @(label: Str, qty: Int)

Counter = @(
  n: Int
  next =
    n + 1
  => :Int
)

onOrdinal unused: Int =
  Ordinal[Mode:On()]()
=> :Int

mkA an: Int =
  Item(label <= "a-item", qty <= an)
=> :Item

bump start: Int =
  c <= Counter(n <= start)
  c.next()
=> :Int

<<< @(onOrdinal, mkA, bump)
"#,
            ),
            (
                "main.td",
                r#">>> ./a.td => @(onOrdinal, mkA, bump)

Enum => Mode = :On :Off :Auto

Item = @(id: Int, qty: Str, tags: @[Str])

stdout(onOrdinal(0))
stdout(jsonEncode(mkA(3)))
stdout(bump(41))
"#,
            ),
        ],
        "1\n{\"label\":\"a-item\",\"qty\":3}\n42",
        ALL_COMPILED,
    );
}

#[test]
fn a_value_keeps_the_methods_of_its_own_type() {
    // The type is not imported, the importer declares a type of the same
    // name, or the importer names it through an alias: the value still
    // has the methods of the type it was built from.
    parity_files(
        "f65i_value_methods",
        &[
            (
                "lib.td",
                r#"Point = @(
  x: Int
  describe =
    "lib-point " + x.toString()
  => :Str
)

Counter = @(
  n: Int
  bump =
    n + 1
  => :Int
)

makePoint px: Int =
  Point(x <= px)
=> :Point

mkCounter cn: Int =
  Counter(n <= cn)
=> :Counter

<<< @(makePoint, mkCounter, Counter)
"#,
            ),
            (
                "main.td",
                r#">>> ./lib.td => @(makePoint, mkCounter, Counter => Ctr)

Point = @(
  y: Int
  describe =
    "main-point"
  => :Str
)

stdout(makePoint(7).describe())
stdout(mkCounter(1).bump())
c <= Ctr(n <= 5)
stdout(c.bump())
"#,
            ),
        ],
        "lib-point 7\n2\n6",
        ALL_COMPILED,
    );
}

#[test]
fn enums_of_the_same_name_in_two_modules_stay_distinct() {
    parity_files(
        "f65i_enum_modules",
        &[
            (
                "a.td",
                r#"Enum => Color = :Red :Green
paintA pn: Int =
  | pn > 0 |> Color:Green()
  | _ |> Color:Red()
=> :Color
<<< @(Color, paintA)
"#,
            ),
            (
                "b.td",
                r#"Enum => Color = :Blue :Red :Green
paintB pn: Int =
  | pn > 0 |> Color:Green()
  | _ |> Color:Blue()
=> :Color
<<< @(Color, paintB)
"#,
            ),
            (
                "main.td",
                r#">>> ./a.td => @(Color, paintA)
>>> ./b.td => @(Color => ColorB, paintB)

stdout(paintA(1))
stdout(paintB(1))
stdout(paintA(1) == Color:Green())
stdout(paintB(1) == ColorB:Green())
stdout(Ordinal[paintB(1)]())
stdout(Ordinal[ColorB:Red()]())
"#,
            ),
        ],
        "1\n2\ntrue\ntrue\n2\n1",
        ALL_COMPILED,
    );
}

#[test]
fn method_bodies_use_the_declaring_module_scope() {
    // interpreter: whether the value is built in the importer or in the
    // module, `decorate` / `prefix` are the module's.
    interp_files_ok(
        "f65i_method_module_scope",
        &[
            (
                "lib.td",
                r#"prefix <= "lib-prefix"

decorate dtext: Str =
  "[" + dtext + "]"
=> :Str

Pilot = @(
  name: Str
  intro =
    decorate(name) + "/" + prefix
  => :Str
)

mkPilot mname: Str =
  Pilot(name <= mname)
=> :Pilot

<<< @(Pilot, mkPilot)
"#,
            ),
            (
                "main.td",
                r#">>> ./lib.td => @(Pilot, mkPilot)

prefix <= "MAIN-prefix"

decorate mtext: Str =
  "<<" + mtext + ">>"
=> :Str

p <= Pilot(name <= "Rei")
stdout(p.intro())
q <= mkPilot("Asuka")
stdout(q.intro())
"#,
            ),
        ],
        "[Rei]/lib-prefix\n[Asuka]/lib-prefix",
    );
}

#[test]
fn an_import_without_a_symbol_list_brings_in_every_export() {
    // interpreter: `>>> ./file.td` imports every export; the importer's
    // own definitions take precedence over those names.
    interp_files_ok(
        "f65i_import_all",
        &[
            (
                "util.td",
                r#"double dx: Int =
  dx * 2
=> :Int

label =
  "util"
=> :Str

<<< @(double, label)
"#,
            ),
            (
                "main.td",
                r#">>> ./util.td

label =
  "main"
=> :Str

stdout(double(4))
stdout(label())
"#,
            ),
        ],
        "8\nmain",
    );
}

// ── Prelude functions as values, pipelines ─────────────────

#[test]
fn debug_in_a_pipeline_prints_the_value_and_passes_it_on() {
    parity(
        "f65i_debug_pipe",
        r#"scores <= @[95, 82, 78, 91]
scores => Filter[_, _ x: Int = x > 80]() => debug => Map[_, _ x: Int = x * 2]() => result
stdout(result)
"#,
        "@[95, 82, 91]\n@[190, 164, 182]",
        ALL_COMPILED,
    );
}

#[test]
fn prelude_functions_are_values() {
    parity(
        "f65i_prelude_values",
        r#"enc <= jsonEncode
stdout(enc(@[1, 2]))
@[1, 2, 3] => jsonEncode => s
stdout(s)
Map[@[@[1], @[2, 3]], jsonEncode]() >=> encoded
stdout(encoded.length())
"hello" => stdout => n
stdout(n)
"#,
        "[1,2]\n[1,2,3]\n2\nhello\n5",
        &["native", "wasm-wasi"],
    );
}

// ── Result, Async, error values ────────────────────────────

#[test]
fn result_methods_decide_with_the_predicate() {
    parity(
        "f65i_result_predicate",
        r#"Error => VE = @(field: Str)
a <= Result[25, _ x = x >= 18](throw <= VE(type <= "VE", message <= "m", field <= "age"))
stdout(a.isSuccess())
stdout(a.getOrDefault(-1))
stdout(a.toString())
b <= Result[15, _ x = x >= 18](throw <= VE(type <= "VE", message <= "m", field <= "age"))
stdout(b.isError())
stdout(b.getOrDefault(-1))
"#,
        "true\n25\nResult(25)\ntrue\n-1",
        &["native"],
    );
}

#[test]
fn map_on_a_running_async_applies_the_function() {
    parity(
        "f65i_async_map",
        r#"sleep(10).map(_ ms = ms + 1000) >=> v
stdout(v)
p <= sleep(5)
stdout(p.getOrDefault(99))
"#,
        "1010\n5",
        &["native"],
    );
}

#[test]
fn omitted_error_type_and_message_read_as_type_name_and_empty() {
    // interpreter: the value keeps the fields omitted (display and
    // jsonEncode unchanged); reading them gives the type name and "".
    interp_ok(
        "f65i_error_omitted",
        r#"Error => SilentError = @(detail: Str)
e1 <= SilentError(type <= "SilentError", detail <= "ignored")
e2 <= SilentError(detail <= "ignored")
stdout("[" + e1.message + "]")
stdout("[" + e2.type + "]")
stdout(jsonEncode(e1))

f kind: Int =
  |== error: Error =
    stdout("message=[" + error.message + "]")
    -1
  => :Int
  | kind == 2 |> SilentError(type <= "SilentError", detail <= "ignored").throw()
  | _ |> 0
=> :Int

stdout(f(2))
"#,
        "[]\n[SilentError]\n{\"detail\":\"ignored\",\"type\":\"SilentError\"}\nmessage=[]\n-1",
    );
}

#[test]
fn an_uncaught_error_shows_an_empty_message_as_written() {
    // interpreter: an explicit `message <= ""` is shown as it is; an
    // omitted message shows the type name.
    let (_, err, code) = interp(
        "f65i_uncaught_empty",
        r#"Error => SilentError = @(detail: Str)
SilentError(type <= "SilentError", message <= "", detail <= "ignored").throw()
"#,
    );
    assert_eq!(code, Some(1));
    assert!(
        err.trim_end().ends_with("Error[SilentError]:"),
        "stderr: {err}"
    );
    let (_, err, code) = interp(
        "f65i_uncaught_omitted",
        r#"Error => SilentError = @(detail: Str)
SilentError(type <= "SilentError", detail <= "ignored").throw()
"#,
    );
    assert_eq!(code, Some(1));
    assert!(
        err.trim_end().ends_with("Error[SilentError]: SilentError"),
        "stderr: {err}"
    );
}

#[test]
fn a_caught_io_error_has_error_info() {
    parity(
        "f65i_ioerror_info",
        r#">>> taida-lang/os => @(writeFile)

f p: Str =
  |== error: Error =
    error.errorInfo() >=> info
    stdout("kind=" + info.kind)
    -1
  => :Int
  writeFile(p, "x") >=> n
  n
=> :Int
stdout(f("no_dir/x.txt"))
"#,
        "kind=not_found\n-1",
        &["native"],
    );
}

#[test]
fn socket_argument_failures_are_values_and_throws_stay_catchable() {
    parity(
        "f65i_socket_args",
        r#">>> taida-lang/os => @(tcpListen)
Error => NoPort = @()

getPort x: Int =
  | x < 0 |> NoPort(type <= "NoPort", message <= "no port").throw()
  | _ |> x
=> :Int

f p: Int =
  |== e: Error =
    "caught: " + e.message
  => :Str
  r <= tcpListen(p)
  r >=> res
  res.toString()
=> :Str

g x: Int =
  |== e: Error =
    "caught: " + e.message
  => :Str
  r <= tcpListen(getPort(x))
  "unreachable"
=> :Str

stdout(f(-1))
stdout(f(65536))
stdout(g(-1))
"#,
        "Result(throw <= tcpListen: port must be within 0..=65535)\nResult(throw <= tcpListen: port must be within 0..=65535)\ncaught: no port",
        &["native"],
    );
}

// ── Builtins never panic ───────────────────────────────────

#[test]
fn clamp_keeps_int_precision_and_returns_the_lower_bound_when_reversed() {
    parity(
        "f65i_clamp_precision",
        r#"stdout(Clamp[9223372036854775806, 0, 9223372036854775807]())
stdout(Clamp[-9223372036854775807, -9223372036854775807, 0]())
stdout(Clamp[9007199254740993, 0, 9223372036854775807]())
"#,
        "9223372036854775806\n-9223372036854775807\n9007199254740993",
        ALL_COMPILED,
    );
    // interpreter: reversed bounds give the lower bound (for every input);
    // a NaN bound leaves the value as it is.
    interp_ok(
        "f65i_clamp_reversed",
        r#"stdout(Clamp[5, 10, 0]())
stdout(Clamp[15, 10, 0]())
stdout(Clamp[0.5, 1.0, 0.0]())
n <= Sqrt[-1.0]()
stdout(Clamp[0.5, n, 1.0]())
"#,
        "10\n10\n1.0\n0.5",
    );
}

#[test]
fn div_and_mod_of_the_minimum_int_by_minus_one_fail_softly() {
    // interpreter: like a zero divisor, a failed Lax.
    interp_ok(
        "f65i_div_min",
        r#"minInt <= -9223372036854775807 - 1
d <= Div[minInt, -1]()
m <= Mod[minInt, -1]()
stdout(d.has_value)
stdout(m.has_value)
d >=> q
stdout(q)
"#,
        "false\nfalse\n0",
    );
}

#[test]
fn sort_places_nan_last() {
    // interpreter: NaN goes after every other value, in either direction.
    interp_ok(
        "f65i_sort_nan",
        r#"nan <= Sqrt[-1.0]()
xs <= @[3.0, nan, 1.0, 2.0]
Sort[xs]() >=> asc
stdout(asc)
Sort[xs](reverse <= true) >=> desc
stdout(desc)
"#,
        "@[1.0, 2.0, 3.0, NaN]\n@[3.0, 2.0, 1.0, NaN]",
    );
}

#[test]
fn length_mold_counts_elements() {
    // interpreter
    interp_ok(
        "f65i_length_mold",
        r#"n <= Length[@[1, 2, 3]]()
stdout(n + 1)
stdout(Length["日本"]())
"#,
        "4\n2",
    );
}

#[test]
fn a_huge_bytes_length_is_a_failed_lax() {
    parity(
        "f65i_bytes_huge",
        r#"b <= Bytes[9223372036854775807]()
stdout(b.has_value)
"#,
        "false",
        &["native", "wasm-wasi"],
    );
}

#[test]
fn a_throw_in_a_mold_argument_reaches_the_ceiling() {
    parity(
        "f65i_throw_mold_arg",
        r#"Error => E1 = @()
boom x: Int =
  E1(type <= "E1", message <= "boom").throw()
=> :Int

f =
  |== e: Error =
    "caught " + e.message
  => :Str
  s <= Slice["hello", boom(1), 3]()
  "after " + s
=> :Str
stdout(f())
"#,
        "caught boom",
        ALL_COMPILED,
    );
    // interpreter: in an option too (`default <= ...`).
    interp_ok(
        "f65i_throw_mold_option",
        r#"Error => E1 = @()
boom x: Int =
  E1(type <= "E1", message <= "boom").throw()
=> :Int

f =
  |== e: Error =
    "caught " + e.message
  => :Str
  d <= Div[10, 0](default <= boom(1))
  "after"
=> :Str
stdout(f())
"#,
        "caught boom",
    );
}

#[test]
fn the_gorilla_in_a_callback_ends_the_program() {
    // interpreter
    let (out, _, code) = interp(
        "f65i_gorilla_callback",
        r#"g x: Int =
  | x == 2 |> ><
  | _ |> x
=> :Int
Map[@[1, 2, 3], _ x = g(x)]() >=> ys
stdout(ys)
stdout("still running")
"#,
    );
    assert_eq!(code, Some(1));
    assert_eq!(out, "");
}

#[test]
fn method_recursion_stops_at_the_depth_limit() {
    // interpreter: a runtime error, not a stack overflow.
    let (_, err, code) = interp(
        "f65i_method_depth",
        r#"Node = @(
  v: Int
  down n: Int =
    | n == 0 |> 0
    | _ |> 1 + Node(v <= n).down(n - 1)
  => :Int
)

root <= Node(v <= 0)
stdout(root.down(100))
stdout(root.down(20000))
"#,
    );
    assert_eq!(code, Some(1));
    assert!(err.contains("Maximum call depth"), "stderr: {err}");
}

#[test]
fn an_overlong_unicode_escape_is_a_diagnostic() {
    let (_, err, code) = interp("f65i_unicode_escape", "stdout(\"\\u{FFFFFFFFF}\")\n");
    assert_eq!(code, Some(1), "stderr: {err}");
    assert!(
        err.contains("code point must be <= U+10FFFF"),
        "stderr: {err}"
    );
}

#[test]
fn omitted_collection_and_result_fields_get_their_defaults() {
    // interpreter: an empty HashMap / Set, a success Result, and an empty
    // Gorillax — never `@()`.
    interp_ok(
        "f65i_field_defaults",
        r#"R = @(
  name: Str
  m: HashMap[Str, Int]
  s: Set[Int]
  r: Result[Int, Error]
  g: Gorillax[Int]
)
x <= R(name <= "x")
stdout(x.m.size())
stdout(x.s.size())
stdout(x.r.isSuccess())
stdout(x.g.hasValue())
"#,
        "0\n0\ntrue\nfalse",
    );
}

// ── Tail recursion under an error ceiling ──────────────────

#[test]
fn self_tail_recursion_under_a_leading_ceiling_does_not_grow() {
    // interpreter: the recursion loops inside the ceiling, and a caught
    // error runs the handler with the innermost call's arguments.
    interp_ok(
        "f65i_ceiling_tail",
        r#"walk n: Int acc: Int =
  |== error: Error =
    stdout("caught at n=" + n.toString())
    -1
  => :Int
  | n == 0 |> acc
  | n == -5 |> Error(type <= "E", message <= "bottom").throw()
  | _ |> walk(n - 1, acc + 1)
=> :Int

stdout(walk(100000, 0))
stdout(walk(-1, 0))
"#,
        "100000\ncaught at n=-5\n-1",
    );
}
