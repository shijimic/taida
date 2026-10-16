//! Regressions for the developer tools: `taida way` (check / lint / verify /
//! todo), `taida graph`, `taida doc`, the REPL, `taida init`, `taida auth`,
//! and the type checker's handling of very deep expressions.
//!
//! Each test drives the built `taida` binary and pins what a user sees.
mod common;

use common::{taida_bin, unique_temp_dir, write_file};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run(args: &[&str], dir: &Path) -> Output {
    Command::new(taida_bin())
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .output()
        .expect("run taida")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

/// A scratch directory holding `files` (path, content).
fn project(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = unique_temp_dir(name);
    for (path, content) in files {
        let file = dir.join(path);
        fs::create_dir_all(file.parent().expect("parent")).expect("create directory");
        write_file(&file, content);
    }
    dir
}

// ── way verify: direction-constraint ────────────────────────────────

#[test]
fn direction_constraint_accepts_forward_pipelines_and_backward_chains() {
    let dir = project(
        "f65_direction",
        &[(
            "main.td",
            r#"plusOne x: Int =
  x + 1
=> :Int
double x: Int =
  x * 2
=> :Int
input <= "  asuka langley  "
input => Trim[_]() => Upper[_]() => result
stdout(result)
42 => answer
5 => double(_) => double(_) => twice
chained <= plusOne(_) <= double(_) <= 4
typed: Int <= plusOne(_) <= double(_) <= 10
stdout(chained.toString())
"#,
        )],
    );
    let out = run(
        &[
            "way",
            "verify",
            "--check",
            "direction-constraint",
            "main.td",
        ],
        &dir,
    );
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("[PASS] direction-constraint"));
    // The whole gate agrees.
    let full = run(&["way", "main.td"], &dir);
    assert_eq!(code(&full), 0, "{}{}", stdout(&full), stderr(&full));
}

// ── way verify: call graph ──────────────────────────────────────────

const DEAD_CODE_SOURCE: &str = r#"double x: Int =
  x * 2
=> :Int

isBad x: Int =
  x > 3
=> :Bool

makeName x: Int =
  "n" + x.toString()
=> :Str

label x: Int =
  "v=" + x.toString()
=> :Str

inc x: Int =
  x + 1
=> :Int

ten x: Int =
  x * 10
=> :Int

Counter = @(
  count: Int
  bump =
    inc(count)
  => :Int
)

Map[@[1, 2, 3], double]() >=> ys
stdout(ys.length().toString())
stdout((!isBad(1)).toString())
p <= @(name <= makeName(1))
stdout(p.name)
stdout(`label: ${label(3)}`)
c <= Counter(count <= 1)
stdout(c.bump().toString())
half: Int <=< Div[ten(1), 2]()
stdout(half.toString())
"#;

#[test]
fn dead_code_sees_calls_in_every_expression_position() {
    let dir = project("f65_dead_code", &[("main.td", DEAD_CODE_SOURCE)]);
    let out = run(&["way", "verify", "--check", "dead-code", "main.td"], &dir);
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("[PASS] dead-code"),
        "{}",
        stdout(&out)
    );

    // A function nobody uses is still reported.
    let unused = format!("{DEAD_CODE_SOURCE}\nunused x: Int =\n  x\n=> :Int\n");
    write_file(&dir.join("main.td"), &unused);
    let out = run(&["way", "verify", "--check", "dead-code", "main.td"], &dir);
    assert!(
        stdout(&out).contains("Unreachable function: unused"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn dead_code_follows_calls_made_from_nested_functions() {
    let dir = project(
        "f65_dead_nested",
        &[(
            "main.td",
            r#"inc x: Int =
  x + 1
=> :Int
outer x: Int =
  helper y: Int =
    inc(y) * 2
  => :Int
  helper(x)
=> :Int
stdout(outer(2).toString())
"#,
        )],
    );
    let out = run(&["way", "verify", "--check", "dead-code", "main.td"], &dir);
    assert!(
        stdout(&out).contains("[PASS] dead-code"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn mutual_recursion_through_a_unary_operator_is_rejected() {
    let dir = project(
        "f65_mutual_unary",
        &[(
            "main.td",
            r#"isEven n: Int =
  | n == 0 |> true
  | _ |> !isOdd(n - 1)
=> :Bool

isOdd n: Int =
  | n == 0 |> false
  | _ |> !isEven(n - 1)
=> :Bool

stdout(isEven(4).toString())
"#,
        )],
    );
    let out = run(
        &["way", "verify", "--check", "mutual-recursion", "main.td"],
        &dir,
    );
    assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("[E1614]"), "{}", stdout(&out));
}

#[test]
fn a_name_bound_later_or_elsewhere_does_not_hide_a_call_of_the_function() {
    // The right-hand side is read before the name is bound: this is a call
    // of the function `total`, and recursion outside the tail position.
    let dir = project(
        "f65_scope_binding",
        &[(
            "main.td",
            "total n: Int =\n  | n == 0 |> 0\n  | _ |>\n    total <= n + total(n - 1)\n    total\n=> :Int\n\nstdout(total(10).toString())\n",
        )],
    );
    let out = run(&["way", "check", "main.td"], &dir);
    assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
    assert!(stderr(&out).contains("[E0701]"), "{}", stderr(&out));

    // A lambda parameter is bound in the lambda only.
    let dir = project(
        "f65_scope_lambda",
        &[(
            "main.td",
            "a n: Int =\n  | n == 0 |> 0\n  | _ |> 1 + b(n - 1)\n=> :Int\n\nb n: Int =\n  ys <= Map[@[1], _ a: Int = a + 1]()\n  | n == 0 |> ys.length()\n  | _ |> a(n - 1)\n=> :Int\n\nstdout(a(10).toString())\n",
        )],
    );
    let out = run(&["way", "check", "main.td"], &dir);
    assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
    assert!(stderr(&out).contains("[E1614]"), "{}", stderr(&out));

    // A binding made after the call does not make the function dead code.
    let dir = project(
        "f65_scope_dead",
        &[(
            "main.td",
            "helper x: Int =\n  x + 1\n=> :Int\n\nf n: Int =\n  r <= helper(n)\n  helper <= r + 1\n  helper\n=> :Int\n\nstdout(f(1).toString())\n",
        )],
    );
    let out = run(&["way", "verify", "--check", "dead-code", "main.td"], &dir);
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    assert!(!stdout(&out).contains("Unreachable"), "{}", stdout(&out));
}

#[test]
fn a_call_in_a_parameter_default_is_not_a_call_the_function_makes() {
    let dir = project(
        "f65_default_call",
        &[(
            "main.td",
            "f n: Int x: Int <= g(0) =\n  n + x\n=> :Int\n\ng n: Int =\n  | n > 100 |> 0\n  | _ |> 1 + f(n + 1, 0)\n=> :Int\n\nstdout(g(1).toString())\n",
        )],
    );
    let out = run(&["main.td"], &dir);
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    assert_eq!(stdout(&out).trim(), "3");
}

#[test]
fn a_throw_in_a_lambda_is_covered_by_the_ceiling_of_the_function_that_wrote_it() {
    let dir = project(
        "f65_lambda_ceiling",
        &[(
            "main.td",
            "Error => MyErr = @(code: Int)\n\nrisky x: Int =\n  |== e: Error =\n    0\n  => :Int\n  Map[@[1, 2], _ n: Int =\n    m <= n + x\n    | m > 2 |> MyErr(type <= \"MyErr\", message <= \"boom\", code <= m).throw()\n    | _ |> m\n  ]() >=> ys\n  ys.length()\n=> :Int\n\nstdout(risky(1).toString())\n",
        )],
    );
    let out = run(
        &["way", "verify", "--check", "error-coverage", "main.td"],
        &dir,
    );
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
}

/// The verdict of `error-coverage` on `source`: whether it reported a throw
/// site that nothing covers.
fn error_coverage_reports(name: &str, source: &str) -> bool {
    let dir = project(name, &[("main.td", source)]);
    let out = run(
        &["way", "verify", "--check", "error-coverage", "main.td"],
        &dir,
    );
    let text = stdout(&out);
    assert!(
        text.contains("[ERROR] error-coverage") || text.contains("[PASS] error-coverage"),
        "{text}{}",
        stderr(&out)
    );
    text.contains("[ERROR] error-coverage")
}

#[test]
fn error_coverage_does_not_take_a_throw_to_be_covered_where_it_cannot_see_the_call() {
    assert!(
        error_coverage_reports(
            "f65_ec_report_1",
            "risky v: Int =\n  | v > 0 |> Error(type <= \"E\", message <= \"boom\").throw()\n  | _ |> v\n=> :Int\n\nkeep f: Int => :Int =\n  f\n=> :Int => :Int\n\nmake x: Int =\n  |== e: Error =\n    _ v: Int = v\n  => :Int => :Int\n  keep(risky)\n=> :Int => :Int\n\ng <= make(1)\nstdout(g(1).toString())\n"
        ),
        "a function handed to a user function may be kept"
    );
    assert!(
        error_coverage_reports(
            "f65_ec_report_2",
            "keep f: Int => :Int =\n  f\n=> :Int => :Int\n\nmake x: Int =\n  |== e: Error =\n    _ v: Int = v\n  => :Int => :Int\n  keep(_ v: Int = (\n    | v > 0 |> Error(type <= \"E\", message <= \"boom\").throw()\n    | _ |> v\n  ))\n=> :Int => :Int\n\ng <= make(1)\nstdout(g(1).toString())\n"
        ),
        "a lambda handed to a user function may be kept"
    );
    assert!(
        error_coverage_reports(
            "f65_ec_report_3",
            "risky v: Int =\n  | v > 0 |> Error(type <= \"E\", message <= \"boom\").throw()\n  | _ |> v\n=> :Int\n\nTemp = @(\n  c: Int\n  toString =\n    risky(c).toString()\n  => :Str\n)\n\nsafe x: Int =\n  |== e: Error =\n    \"err\"\n  => :Str\n  x.toString()\n=> :Str\n\nstdout(safe(1))\nt <= Temp(c <= 1)\nstdout(t.toString())\n"
        ),
        "a builtin method name does not reach a method of the program"
    );
    assert!(
        error_coverage_reports(
            "f65_ec_report_4",
            "risky v: Int =\n  | v > 0 |> Error(type <= \"E\", message <= \"boom\").throw()\n  | _ |> v\n=> :Int\n\nMold[T] => Risky[T] = @(\n  unmold _ =\n    risky(filling)\n  => :Int\n)\n\nsafe x: Int =\n  |== e: Error =\n    0\n  => :Int\n  Div[x, 1]() >=> q\n  q\n=> :Int\n\nstdout(safe(1).toString())\nRisky[1]() >=> v\nstdout(v.toString())\n"
        ),
        "an unmold elsewhere does not cover the hook of another mold"
    );
    assert!(
        error_coverage_reports(
            "f65_ec_report_5",
            "risky v: Int =\n  | v > 0 |> Error(type <= \"E2\", message <= \"in handler\").throw()\n  | _ |> v\n=> :Int\n\nsafe x: Int =\n  |== e: Error =\n    Map[@[x], risky]() >=> ys\n    Sum[ys]() >=> s\n    s\n  => :Int\n  Error(type <= \"E\", message <= \"first\").throw()\n=> :Int\n\nstdout(safe(1).toString())\n"
        ),
        "a handler does not cover what it hands to a mold"
    );
    assert!(
        error_coverage_reports(
            "f65_ec_report_6",
            "risky v: Int =\n  | v > 0 |> Error(type <= \"E\", message <= \"boom\").throw()\n  | _ |> v\n=> :Int\n\nsafe x: Int =\n  |== e: Error =\n    0\n  => :Int\n  Cfg = @(\n    m: Int\n    n: Int <= risky(1)\n  )\n  c <= Cfg(m <= x)\n  c.n\n=> :Int\n\nstdout(safe(1).toString())\n"
        ),
        "a field default is not covered by a ceiling around the instantiation"
    );
}

#[test]
fn error_coverage_follows_the_calls_it_can_see() {
    assert!(
        !error_coverage_reports(
            "f65_ec_pass_1",
            "Mold[T] => Risky[T] = @(\n  solidify =\n    | filling > 0 |> Error(type <= \"E\", message <= \"boom\").throw()\n    | _ |> filling\n  => :T\n)\n\nsafe x: Int =\n  |== e: Error =\n    0\n  => :Int\n  Risky[x]() >=> v\n  v\n=> :Int\n\nstdout(safe(1).toString())\n"
        ),
        "the solidify hook is reached from the mold it belongs to"
    );
    assert!(
        !error_coverage_reports(
            "f65_ec_pass_2",
            "outer x: Int =\n  inner y: Int =\n    | y > 0 |> Error(type <= \"E\", message <= \"boom\").throw()\n    | _ |> y\n  => :Int\n  |== e: Error =\n    0\n  => :Int\n  inner(x)\n=> :Int\n\nstdout(outer(1).toString())\n"
        ),
        "a nested function is reached by its own name"
    );
    assert!(
        !error_coverage_reports(
            "f65_ec_pass_3",
            "outer x: Int =\n  count y: Int =\n    | y > 2 |> Error(type <= \"E\", message <= \"boom\").throw()\n    | _ |> count(y + 1)\n  => :Int\n  |== e: Error =\n    0\n  => :Int\n  count(x)\n=> :Int\n\nstdout(outer(1).toString())\n"
        ),
        "a nested function that calls itself"
    );
}

#[test]
fn a_nested_function_calls_itself_by_its_own_name() {
    // Inside `outer`, `h` is the nested function, in its own body too, so the
    // top-level `h` is never called.
    let dir = project(
        "f65_nested_self",
        &[(
            "main.td",
            "h x: Int =\n  x * 100\n=> :Int\n\nouter n: Int =\n  h x: Int =\n    | x < 1 |> 0\n    | _ |> h(x - 1) + 1\n  => :Int\n  h(n)\n=> :Int\n\nstdout(outer(3).toString())\n",
        )],
    );
    let ran = run(&["main.td"], &dir);
    assert_eq!(code(&ran), 0, "{}{}", stdout(&ran), stderr(&ran));
    assert_eq!(stdout(&ran).trim(), "3");
    let verify = run(&["way", "verify", "--check", "dead-code", "main.td"], &dir);
    assert!(
        stdout(&verify).contains("Unreachable function: h"),
        "{}",
        stdout(&verify)
    );
}

#[test]
fn graph_lists_the_calls_of_doubly_nested_functions() {
    let dir = project(
        "f65_nested_calls",
        &[(
            "main.td",
            "helper y: Int =\n  y * 2\n=> :Int\n\nouter n: Int =\n  mid x: Int =\n    inner y: Int =\n      helper(y) + 1\n    => :Int\n    inner(x)\n  => :Int\n  mid(n)\n=> :Int\n\nstdout(outer(1).toString())\n",
        )],
    );
    let out = run(&["graph", "main.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let outer = value["functions"]
        .as_array()
        .expect("functions")
        .iter()
        .find(|f| f["name"] == "outer")
        .expect("outer");
    let calls: Vec<&str> = outer["calls"]
        .as_array()
        .expect("calls")
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    for callee in ["helper", "inner", "mid"] {
        assert!(calls.contains(&callee), "{callee} in {calls:?}");
    }
}

#[test]
fn a_call_of_a_parameter_is_not_a_call_of_the_function_named_alike() {
    // `twice` calls its parameter `f`, not the function `f` that calls it.
    let dir = project(
        "f65_shadow",
        &[(
            "main.td",
            "inc n: Int =\n  n + 1\n=> :Int\n\nf x: Int =\n  twice(inc, x)\n=> :Int\n\ntwice f: Int => :Int x: Int =\n  | x > 100 |> x\n  | _ |>\n    y <= f(x)\n    y + 1\n=> :Int\n\nstdout(f(1).toString())\n",
        )],
    );
    let ran = run(&["main.td"], &dir);
    assert_eq!(code(&ran), 0, "{}{}", stdout(&ran), stderr(&ran));
    assert_eq!(stdout(&ran).trim(), "3");
    let verify = run(
        &["way", "verify", "--check", "mutual-recursion", "main.td"],
        &dir,
    );
    assert_eq!(code(&verify), 0, "{}{}", stdout(&verify), stderr(&verify));
    // The function `total` is not called by `compute`, which calls its own
    // parameter of that name, so it is still dead code.
    let dir = project(
        "f65_shadow_dead",
        &[(
            "main.td",
            "total x: Int =\n  x\n=> :Int\n\ncompute total: Int =\n  total + 1\n=> :Int\n\nstdout(compute(1).toString())\n",
        )],
    );
    let dead = run(&["way", "verify", "--check", "dead-code", "main.td"], &dir);
    assert!(
        stdout(&dead).contains("Unreachable function: total"),
        "{}",
        stdout(&dead)
    );
}

#[test]
fn a_function_named_as_a_pipeline_stage_is_called_and_closes_a_cycle() {
    let dir = project(
        "f65_pipe_cycle",
        &[(
            "main.td",
            "a n: Int =\n  | n < 1 |> 0\n  | _ |> step(n)\n=> :Int\n\nstep n: Int =\n  n - 1 => b => r\n  r\n=> :Int\n\nb n: Int =\n  x <= a(n)\n  x + 1\n=> :Int\n",
        )],
    );
    let out = run(&["way", "check", "main.td"], &dir);
    assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
    assert!(stderr(&out).contains("[E1614]"), "{}", stderr(&out));
}

#[test]
fn a_pipeline_inside_a_template_is_not_a_second_direction() {
    let dir = project(
        "f65_template_pipe",
        &[(
            "main.td",
            "double x: Int =\n  x * 2\n=> :Int\n\ns <= `v=${3 => double(_)}`\nstdout(s)\n",
        )],
    );
    let out = run(
        &[
            "way",
            "verify",
            "--check",
            "direction-constraint",
            "main.td",
        ],
        &dir,
    );
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    let ran = run(&["main.td"], &dir);
    assert_eq!(stdout(&ran).trim(), "v=6");
}

#[test]
fn a_deep_expression_in_a_template_slot_does_not_crash_the_analyses() {
    let terms = 3000;
    let dir = project(
        "f65_deep_slot",
        &[(
            "main.td",
            &format!(
                "t <= `v=${{{}}}`\nstdout(t)\n",
                vec!["1"; terms].join(" + ")
            ),
        )],
    );
    for args in [
        vec!["way", "verify", "main.td"],
        vec!["way", "todo", "main.td"],
        vec!["graph", "summary", "main.td"],
    ] {
        let out = run(&args, &dir);
        assert_eq!(
            code(&out),
            0,
            "taida {}: {}{}",
            args.join(" "),
            stdout(&out),
            stderr(&out)
        );
    }
}

#[test]
fn a_function_passed_as_a_value_is_a_call_like_a_lambda() {
    // A tree walk through `Map[..., f]() >=> x` recurses through a function
    // value, as `Map[..., _ k = f(k)]()` does through a lambda: both are
    // recursion outside the tail position, rejected before the program runs.
    let mutual = |arg: &str| {
        format!(
            "Node = @(v: Int, kids: @[Node])\n\nsumAll n: Node =\n  Map[n.kids, {arg}]() >=> parts\n  n.v + 1\n=> :Int\n\nsumOne k: Node =\n  sumAll(k) + 1\n=> :Int\n\nleaf <= Node(v <= 1, kids <= @[])\nstdout(sumAll(leaf).toString())\n"
        )
    };
    let direct = |arg: &str| {
        format!(
            "Node = @(v: Int, kids: @[Node])\n\ntotal n: Node =\n  Map[n.kids, {arg}]() >=> parts\n  Sum[parts]() >=> s\n  n.v + s\n=> :Int\n\nleaf <= Node(v <= 1, kids <= @[])\nstdout(total(leaf).toString())\n"
        )
    };
    // `[E1614]` also stops `taida <FILE>`; `[E0701]` is reported by `taida way`.
    let cases = [
        ("f65_walk_value", mutual("sumOne"), "[E1614]"),
        (
            "f65_walk_lambda",
            mutual("_ k: Node = sumOne(k)"),
            "[E1614]",
        ),
        ("f65_walk_self_value", direct("total"), "[E0701]"),
        (
            "f65_walk_self_lambda",
            direct("_ k: Node = total(k)"),
            "[E0701]",
        ),
    ];
    for (name, source, diagnostic) in cases {
        let dir = project(name, &[("main.td", source.as_str())]);
        if diagnostic == "[E1614]" {
            let out = run(&["main.td"], &dir);
            let text = format!("{}{}", stdout(&out), stderr(&out));
            assert_ne!(code(&out), 0, "{name}: {text}");
            assert!(text.contains(diagnostic), "{name}: {text}");
        }
        let check = run(&["way", "check", "main.td"], &dir);
        let text = format!("{}{}", stdout(&check), stderr(&check));
        assert_eq!(code(&check), 1, "{name}: {text}");
        assert!(text.contains(diagnostic), "{name}: {text}");
    }
}

#[test]
fn error_coverage_follows_function_values_methods_and_later_definitions() {
    // The ceiling in `safe` covers `risky`, which is passed as a value.
    let by_value = project(
        "f65_cover_value",
        &[(
            "main.td",
            r#"risky x: Int =
  | x > 0 |> Error(type <= "E", message <= "boom").throw()
  | _ |> x
=> :Int

safe xs: @[Int] =
  |== e: Error =
    0 - 1
  => :Int
  Map[xs, risky]() >=> ys
  ys.length()
=> :Int

stdout(safe(@[1]).toString())
"#,
        )],
    );
    let out = run(
        &["way", "verify", "--check", "error-coverage", "main.td"],
        &by_value,
    );
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));

    // The callee is defined after the covered caller.
    let later = project(
        "f65_cover_later",
        &[(
            "main.td",
            r#"safe x: Int =
  |== e: Error =
    0 - 1
  => :Int
  risky(x)
=> :Int

risky x: Int =
  | x > 0 |> Error(type <= "E", message <= "boom").throw()
  | _ |> x
=> :Int

stdout(safe(1).toString())
"#,
        )],
    );
    let out = run(
        &["way", "verify", "--check", "error-coverage", "main.td"],
        &later,
    );
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
    let ran = run(&["main.td"], &later);
    assert!(
        !stderr(&ran).contains("Uncovered throw site"),
        "no warning for a covered throw: {}",
        stderr(&ran)
    );

    // A throw in a method body nobody covers is reported.
    let method = project(
        "f65_cover_method",
        &[(
            "main.td",
            r#"Guard = @(
  limit: Int
  check x: Int =
    | x > limit |> Error(type <= "E", message <= "over").throw()
    | _ |> x
  => :Int
)
g <= Guard(limit <= 1)
stdout(g.check(5).toString())
"#,
        )],
    );
    let out = run(
        &["way", "verify", "--check", "error-coverage", "main.td"],
        &method,
    );
    assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("Uncovered throw site"));

    // ... and a ceiling around the method call covers it.
    let covered = project(
        "f65_cover_method_ok",
        &[(
            "main.td",
            r#"Guard = @(
  limit: Int
  check x: Int =
    | x > limit |> Error(type <= "E", message <= "over").throw()
    | _ |> x
  => :Int
)
run x: Int =
  |== e: Error =
    0 - 1
  => :Int
  g <= Guard(limit <= 1)
  g.check(x)
=> :Int
stdout(run(5).toString())
"#,
        )],
    );
    let out = run(
        &["way", "verify", "--check", "error-coverage", "main.td"],
        &covered,
    );
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
}

#[test]
fn unchecked_lax_accepts_get_or_default_and_unmold() {
    let dir = project(
        "f65_lax",
        &[(
            "main.td",
            r#"lax <= Div[10, 0]()
stdout(lax.has_value.toString())
v <= lax.getOrDefault(99)
stdout(v.toString())
opt <= Lax[5]()
w <= opt.unmold() + 1
stdout(w.toString())
"#,
        )],
    );
    let out = run(
        &["way", "verify", "--check", "unchecked-lax", "main.td"],
        &dir,
    );
    assert!(
        stdout(&out).contains("[PASS] unchecked-lax"),
        "{}",
        stdout(&out)
    );
}

// ── way lint: @Since ────────────────────────────────────────────────

#[test]
fn lint_rejects_a_since_value_that_is_not_a_taida_version() {
    let dir = project(
        "f65_since",
        &[(
            "main.td",
            r#"///@ Purpose: add
///@ Since: e.32
add x: Int y: Int =
  x + y
=> :Int

///@ Since: e.32.alpha
sub x: Int y: Int =
  x - y
=> :Int

///@ Since: 1.2.3
mul x: Int y: Int =
  x * y
=> :Int

///@ Since: not-a-version
div x: Int y: Int =
  x
=> :Int

stdout(add(1, 2).toString())
"#,
        )],
    );
    let out = run(&["way", "lint", "main.td"], &dir);
    assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
    let text = stdout(&out);
    assert_eq!(text.matches("[E1810]").count(), 2, "{text}");
    assert!(
        text.contains("'1.2.3'") && text.contains("'not-a-version'"),
        "{text}"
    );
}

#[test]
fn doc_reads_a_colon_written_after_a_space_as_the_colon_of_the_tag() {
    let dir = project(
        "f65_doc_tag_space",
        &[(
            "main.td",
            "///@ Purpose : doubles a number\n///@ Since : e.32\ndouble x: Int =\n  x * 2\n=> :Int\n",
        )],
    );
    let md = stdout(&run(&["doc", "generate", "main.td"], &dir));
    assert!(md.contains("> doubles a number"), "{md}");
    assert!(md.contains("**Since**: e.32"), "{md}");
    let lint = run(&["way", "lint", "main.td"], &dir);
    assert_eq!(code(&lint), 0, "{}{}", stdout(&lint), stderr(&lint));
}

#[test]
fn the_since_lint_reads_the_tag_the_way_the_documentation_does() {
    let dir = project(
        "f65_since_shapes",
        &[(
            "main.td",
            // A value on the next line is the value; a space before the
            // colon leaves `: 6.0` as the value, which is not a version.
            "///@ Since:\n///@   e.32\nadd x: Int =\n  x\n=> :Int\n\n///@ Since : 6.0\nsub x: Int =\n  x\n=> :Int\n",
        )],
    );
    let out = run(&["way", "lint", "main.td"], &dir);
    let text = stdout(&out);
    assert_eq!(text.matches("[E1810]").count(), 1, "{text}");
    assert!(text.contains("'6.0'"), "{text}");

    // A prose line that begins with the word is not the tag.
    let dir = project(
        "f65_since_prose",
        &[(
            "main.td",
            "///@ Purpose: doubles.\n///@ Since the result can overflow, keep inputs small.\ndouble x: Int =\n  x * 2\n=> :Int\n",
        )],
    );
    let out = run(&["way", "lint", "main.td"], &dir);
    assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
}

// ── way todo / release gate ─────────────────────────────────────────

#[test]
fn todo_and_release_gate_find_molds_in_arms_templates_and_stubs() {
    let cases: [(&str, &str, &str); 3] = [
        (
            "arm",
            r#"pick x: Int =
  | x > 0 |>
    t <= TODO[Int](id <= "ARM-1", task <= "in arm let-binding", sol <= 5)
    x + 1
  | _ |> 0
=> :Int
stdout(pick(2).toString())
"#,
            "ARM-1",
        ),
        (
            "template",
            r#"t <= `value=${TODO[Int](id <= "TPL-1", task <= "in template", sol <= 5).toString()}`
stdout(t)
"#,
            "TPL-1",
        ),
        (
            "stub",
            "x <= Stub[Int](5)\nstdout(x.toString())\n",
            "Stubs: 1",
        ),
    ];
    for (name, source, marker) in cases {
        let dir = project(&format!("f65_todo_{name}"), &[("main.td", source)]);
        let out = run(&["way", "todo", "main.td"], &dir);
        assert_eq!(code(&out), 0, "{name}: {}{}", stdout(&out), stderr(&out));
        assert!(stdout(&out).contains(marker), "{name}: {}", stdout(&out));
        assert!(
            !stdout(&out).contains("No TODO"),
            "{name}: {}",
            stdout(&out)
        );

        let json = run(&["way", "todo", "--format", "json", "main.td"], &dir);
        let value: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
        let listed =
            value["todos"].as_array().unwrap().len() + value["stubs"].as_array().unwrap().len();
        assert_eq!(listed, 1, "{name}: {value}");

        let build = run(
            &["build", "native", "--release", "main.td", "-o", "out.bin"],
            &dir,
        );
        assert_eq!(
            code(&build),
            1,
            "{name}: {}{}",
            stdout(&build),
            stderr(&build)
        );
        assert!(
            stderr(&build).contains("Release gate failed"),
            "{name}: {}",
            stderr(&build)
        );
    }
}

// ── way: output formats, quiet, exit codes ──────────────────────────

#[test]
fn full_gate_prints_one_document_per_machine_readable_format() {
    let dir = project(
        "f65_gate_docs",
        &[("main.td", "x <= 1\nstdout(x.toString())\n")],
    );
    let json = run(&["way", "--format", "json", "main.td"], &dir);
    assert_eq!(code(&json), 0, "{}", stderr(&json));
    let doc: serde_json::Value = serde_json::from_slice(&json.stdout).expect("one JSON document");
    let stages = doc["stages"].as_object().expect("stages");
    assert!(
        stages.contains_key("check")
            && stages.contains_key("lint")
            && stages.contains_key("verify")
    );

    let sarif = run(&["way", "--format", "sarif", "main.td"], &dir);
    let doc: serde_json::Value = serde_json::from_slice(&sarif.stdout).expect("one SARIF document");
    assert_eq!(doc["version"], "2.1.0");
    assert_eq!(doc["runs"].as_array().expect("runs").len(), 3);

    // Every stage is a tool of its own in the log.
    let names: Vec<&str> = doc["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["tool"]["driver"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["taida way check", "taida way lint", "taida way verify"]
    );

    // jsonl stays a stream of documents, one per line.
    let jsonl = run(&["way", "--format", "jsonl", "main.td"], &dir);
    for line in stdout(&jsonl).lines() {
        serde_json::from_str::<serde_json::Value>(line).expect("each line is JSON");
    }
}

#[test]
fn full_gate_document_holds_only_the_stages_that_ran() {
    let dir = project("f65_gate_stop", &[("main.td", "x <= (\n")]);
    let json = run(&["way", "--format", "json", "main.td"], &dir);
    assert_eq!(code(&json), 1);
    let doc: serde_json::Value = serde_json::from_slice(&json.stdout).expect("one JSON document");
    let stages = doc["stages"].as_object().expect("stages");
    assert!(stages.contains_key("check"));
    assert!(
        !stages.contains_key("lint"),
        "the gate ends at the failing stage"
    );
}

#[test]
fn input_failures_follow_the_format_and_quiet_in_every_subcommand() {
    let dir = project(
        "f65_input_failures",
        &[("pe.td", "x <= (\n"), ("ok.td", "x <= 1\n")],
    );
    for sub in ["check", "lint", "verify", "todo"] {
        let json = run(&["way", sub, "--format", "json", "pe.td"], &dir);
        let value: serde_json::Value = serde_json::from_slice(&json.stdout)
            .unwrap_or_else(|e| panic!("{sub}: stdout is not JSON ({e}): {}", stdout(&json)));
        assert!(
            value.to_string().contains("parse"),
            "{sub} reports the parse failure: {value}"
        );
        assert_ne!(code(&json), 0, "{sub}");

        let quiet = run(&["way", sub, "--quiet", "pe.td"], &dir);
        assert_ne!(code(&quiet), 0, "{sub}");
        assert_eq!(stdout(&quiet), "", "{sub} --quiet prints nothing");
        assert_eq!(stderr(&quiet), "", "{sub} --quiet prints nothing");

        let missing = run(&["way", sub, "--format", "json", "nonexistent.td"], &dir);
        serde_json::from_slice::<serde_json::Value>(&missing.stdout)
            .unwrap_or_else(|e| panic!("{sub}: missing file, stdout is not JSON ({e})"));
        assert_ne!(code(&missing), 0, "{sub}");
    }
    // The files that are fine are still verified next to the broken one.
    let both = run(&["way", "verify", "--format", "json", "."], &dir);
    let value: serde_json::Value = serde_json::from_slice(&both.stdout).expect("json");
    assert_eq!(value["summary"]["errors"], 1, "{value}");
}

#[test]
fn sarif_findings_without_a_place_carry_no_null_location() {
    let dir = unique_temp_dir("f65_sarif_no_place");
    fs::create_dir_all(dir.join("empty")).unwrap();
    for args in [
        vec!["way", "check", "--format", "sarif", "empty"],
        vec!["way", "todo", "--format", "sarif", "missing.td"],
    ] {
        let out = run(&args, &dir);
        let text = stdout(&out);
        let doc: serde_json::Value = serde_json::from_str(&text).expect("SARIF");
        let results = doc["runs"][0]["results"].as_array().expect("results");
        assert!(!results.is_empty(), "{text}");
        for result in results {
            for location in result["locations"].as_array().into_iter().flatten() {
                let physical = &location["physicalLocation"];
                assert!(physical["artifactLocation"]["uri"].is_string(), "{text}");
                if let Some(region) = physical.get("region") {
                    assert!(region["startLine"].is_u64(), "{text}");
                }
            }
        }
    }
}

#[test]
fn lint_does_not_claim_a_check_ran_when_no_file_could_be_read() {
    let dir = project("f65_lint_unparsed", &[("bad.td", "x <= (\n")]);
    let out = run(&["way", "lint", "--format", "jsonl", "bad.td"], &dir);
    let summary = stdout(&out)
        .lines()
        .last()
        .map(str::to_string)
        .unwrap_or_default();
    assert!(!summary.contains("naming-convention"), "{}", stdout(&out));
}

#[test]
fn usage_errors_exit_with_2_in_every_way_subcommand() {
    let dir = project("f65_usage", &[("ok.td", "x <= 1\n")]);
    for sub in ["check", "lint", "verify", "todo"] {
        let no_format_value = run(&["way", sub, "--format"], &dir);
        assert_eq!(code(&no_format_value), 2, "{sub} --format");
        let unknown = run(&["way", sub, "--bogus", "ok.td"], &dir);
        assert_eq!(code(&unknown), 2, "{sub} --bogus");
        let two_paths = run(&["way", sub, "ok.td", "ok.td"], &dir);
        assert_eq!(code(&two_paths), 2, "{sub} two paths");
    }
}

#[test]
fn directory_walk_ends_on_symlink_cycles_and_counts_files_once() {
    let dir = project("f65_symlinks", &[("two/a.td", "x <= 1\n")]);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(".", dir.join("two/s1")).unwrap();
        std::os::unix::fs::symlink(".", dir.join("two/s2")).unwrap();
        let out = run(&["way", "check", "--format", "json", "two"], &dir);
        assert_eq!(code(&out), 0, "{}{}", stdout(&out), stderr(&out));
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
        assert_eq!(value["summary"]["files"], 1, "{value}");
    }
}

#[cfg(unix)]
#[test]
fn directory_walk_lists_a_file_where_it_lives_not_under_a_link_to_it() {
    // A local dependency is installed as a link inside `.taida/deps/`; the
    // link sorts before the real directory, but the file is reported at its
    // own path.
    let dir = project(
        "f65_walk_alias",
        &[
            ("libs/foo/util.td", "Bad x: Int =\n  x\n=> :Int\n"),
            ("main.td", "x <= 1\n"),
            (".taida/deps/.keep", ""),
        ],
    );
    std::os::unix::fs::symlink("../../libs/foo", dir.join(".taida/deps/foo")).unwrap();
    let out = run(&["way", "lint", "--format", "json", "."], &dir);
    let text = stdout(&out);
    assert!(text.contains("libs/foo/util.td"), "{text}");
    assert!(!text.contains(".taida/deps/foo"), "{text}");
    // A link to a directory outside the tree is followed.
    let outside = project("f65_walk_outside", &[("ext/e.td", "x <= 1\n")]);
    let root = project("f65_walk_root", &[("main.td", "x <= 1\n")]);
    std::os::unix::fs::symlink(outside.join("ext"), root.join("linked")).unwrap();
    let out = run(&["way", "check", "--format", "json", "."], &root);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(value["summary"]["files"], 2, "{value}");
}

#[cfg(unix)]
#[test]
fn doc_generate_reports_a_directory_it_cannot_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = project(
        "f65_doc_unreadable",
        &[("sub/bad.td", "x <= 1\n"), ("ok.td", "x <= 1\n")],
    );
    let locked = dir.join("sub");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&locked).is_ok();
    let out = run(&["doc", "generate", "."], &dir);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
        assert!(
            stderr(&out).contains("Error reading directory"),
            "{}",
            stderr(&out)
        );
    }
}

#[cfg(unix)]
#[test]
fn a_directory_that_cannot_be_read_is_an_input_failure() {
    use std::os::unix::fs::PermissionsExt;
    let dir = project(
        "f65_walk_unreadable",
        &[("sub/bad.td", "x <= (\n"), ("ok.td", "x <= 1\n")],
    );
    let locked = dir.join("sub");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    // A user that can read it anyway (root) has nothing to test.
    let readable = fs::read_dir(&locked).is_ok();
    let out = run(&["way", "check", "."], &dir);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if !readable {
        assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
        assert!(
            stderr(&out).contains("Error reading directory"),
            "{}",
            stderr(&out)
        );
    }
}

// ── taida graph ─────────────────────────────────────────────────────

#[test]
fn graph_keeps_the_direction_of_pipelines() {
    let dir = project(
        "f65_graph_direction",
        &[(
            "main.td",
            r#"input <= "  asuka  "
input => Trim[_]() => Upper[_]() => result
stdout(result)
42 => answer
stdout(answer.toString())
"#,
        )],
    );
    let graph = run(&["graph", "main.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&graph.stdout).expect("json");
    let flow: Vec<&str> = value["main_flow"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(
        flow.contains(&"input => Trim[_]() => Upper[_]() => result"),
        "{flow:?}"
    );
    assert!(flow.contains(&"42 => answer"), "{flow:?}");
    assert!(flow.contains(&"input <= \"  asuka  \""), "{flow:?}");

    let summary = run(&["graph", "summary", "--format", "json", "main.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&summary.stdout).expect("json");
    assert_eq!(value["dataflow"]["forward_pipes"], 4, "{value}");
    assert_eq!(value["dataflow"]["backward_pipes"], 1, "{value}");
    assert_eq!(value["dataflow"]["total_pipes"], 5, "{value}");
}

#[test]
fn graph_reports_calls_from_every_position_and_lists_methods() {
    let dir = project(
        "f65_graph_calls",
        &[(
            "main.td",
            r#"Pilot = @(name: Str)

makeName x: Int =
  "n" + x.toString()
=> :Str

inc x: Int =
  x + 1
=> :Int

build x: Int =
  p <= Pilot(name <= makeName(x))
  q <= @(v <= inc(x))
  xs <= @[inc(1), inc(2)]
  t <= `t=${inc(3)}`
  p.name + q.v.toString() + xs.length().toString() + t
=> :Str

outer x: Int =
  helper y: Int =
    inc(y) * 2
  => :Int
  helper(x)
=> :Int

Counter = @(
  count: Int
  bump =
    inc(count)
  => :Int
)

stdout(build(1))
"#,
        )],
    );
    let graph = run(&["graph", "main.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&graph.stdout).expect("json");
    let calls = |name: &str| -> Vec<String> {
        let function = value["functions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .unwrap_or_else(|| panic!("no function {name} in {value}"));
        function["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(calls("build"), vec!["makeName", "inc"]);
    assert_eq!(calls("outer"), vec!["inc", "helper"]);
    assert_eq!(calls("Counter.bump"), vec!["inc"]);
}

#[test]
fn graph_recursive_names_modules_the_same_way_however_it_is_started() {
    let dir = project(
        "f65_graph_recursive",
        &[
            (
                "main.td",
                ">>> ./lib.td => @(double)\nstdout(double(21).toString())\n",
            ),
            (
                "lib.td",
                "double x: Int =\n  x * 2\n=> :Int\n<<< @(double)\n",
            ),
        ],
    );
    let files = |out: &Output| -> Vec<String> {
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
        value["modules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["file"].as_str().unwrap().to_string())
            .collect()
    };
    let relative = files(&run(&["graph", "-r", "main.td"], &dir));
    assert_eq!(relative, vec!["main.td", "./lib.td"]);
    let abs = dir.join("main.td");
    let elsewhere = unique_temp_dir("f65_graph_recursive_cwd");
    let absolute = files(&run(&["graph", "-r", abs.to_str().unwrap()], &elsewhere));
    assert_eq!(absolute[1], "./lib.td");
}

#[test]
fn graph_recursive_lists_imports_it_could_not_follow() {
    let dir = project(
        "f65_graph_unresolved",
        &[(
            "main.td",
            ">>> ./nonexistent.td => @(nothing)\nstdout(\"x\")\n",
        )],
    );
    let out = run(&["graph", "-r", "main.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        value["modules"][0]["unresolved_imports"],
        serde_json::json!(["./nonexistent.td"])
    );
}

#[test]
fn graph_recursive_lists_an_absolute_import_it_could_not_follow() {
    let dir = project(
        "f65_graph_unresolved_abs",
        &[(
            "main.td",
            ">>> /nonexistent/f65/nope.td => @(nothing)\nstdout(\"x\")\n",
        )],
    );
    let out = run(&["graph", "-r", "main.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(
        value["modules"][0]["unresolved_imports"],
        serde_json::json!(["/nonexistent/f65/nope.td"])
    );
}

#[test]
fn graph_recursive_lists_a_broken_module_for_every_module_that_imports_it() {
    let dir = project(
        "f65_graph_unresolved_twice",
        &[
            (
                "main.td",
                ">>> ./lib.td => @(one)\n>>> ./broken.td => @(nothing)\nstdout(one(1).toString())\n",
            ),
            (
                "lib.td",
                ">>> ./broken.td => @(nothing)\none x: Int =\n  x\n=> :Int\n<<< @(one)\n",
            ),
            ("broken.td", "x <= (\n"),
        ],
    );
    let out = run(&["graph", "-r", "main.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let modules = value["modules"].as_array().expect("modules");
    assert_eq!(modules.len(), 2, "{value}");
    for module in modules {
        assert_eq!(
            module["unresolved_imports"],
            serde_json::json!(["./broken.td"]),
            "{module}"
        );
    }
}

#[test]
fn graph_summary_counts_definitions_once_and_sees_import_cycles() {
    let dir = project(
        "f65_summary_counts",
        &[
            (
                "types.td",
                "Pilot = @(name: Str, age: Int)\nError => ValidationError = @(field: Str)\nMold[T] => Box[T] = @(value: T)\nEnum => Status = :Active :Inactive\n",
            ),
            ("a.td", ">>> ./b.td => @(y)\nx <= 1\n<<< @(x)\n"),
            ("b.td", ">>> ./a.td => @(x)\ny <= 2\n<<< @(y)\n"),
        ],
    );
    let out = run(&["graph", "summary", "--format", "json", "types.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(value["stats"]["types"], 4, "{value}");
    assert_eq!(value["stats"]["mold_types"], 1, "{value}");
    assert_eq!(value["stats"]["error_types"], 1, "{value}");

    let cyc = run(&["graph", "summary", "--format", "json", "a.td"], &dir);
    let value: serde_json::Value = serde_json::from_slice(&cyc.stdout).expect("json");
    assert_eq!(value["modules"]["has_cycles"], true, "{value}");
}

#[test]
fn graph_accepts_one_path_only() {
    let dir = project("f65_graph_paths", &[("ok.td", "x <= 1\n")]);
    let out = run(&["graph", "nonexistent.td", "ok.td"], &dir);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("Only one <PATH>"), "{}", stderr(&out));
    let out = run(&["graph", "query", "ok.td"], &dir);
    assert_eq!(code(&out), 1);
}

// ── taida doc ───────────────────────────────────────────────────────

#[test]
fn doc_keeps_every_line_of_a_tag_and_the_code_of_an_example() {
    let dir = project(
        "f65_doc_tags",
        &[(
            "main.td",
            r#"///@ Purpose: first line of the purpose.
///@   second line of the purpose.
///@ Returns: first line of the result
///@   second line of the result
wrapped x: Int =
  x
=> :Int

///@ Purpose: what the type is
///@ Note: a line that opens no known tag
Point = @(
  x: Int
  y: Int
)

///@ Throws:
///@   - ValidationError: the id is negative
///@   - NetworkError: cannot connect
///@ Example:
///@   f <= _ x: Int =
///@     y <= x * 2
///@     y + 1
///@
///@   stdout(f(1).toString())
helper a: Int =
  a
=> :Int
"#,
        )],
    );
    let out = run(&["doc", "generate", "main.td"], &dir);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let md = stdout(&out);
    assert!(
        md.contains("> first line of the purpose. second line of the purpose."),
        "{md}"
    );
    assert!(
        md.contains("**Returns**: `Int` - first line of the result second line of the result"),
        "{md}"
    );
    assert!(md.contains("Note: a line that opens no known tag"), "{md}");
    assert!(
        md.contains("- ValidationError: the id is negative\n"),
        "{md}"
    );
    assert!(!md.contains("- - "), "{md}");
    assert!(
        md.contains("f <= _ x: Int =\n  y <= x * 2\n  y + 1\n\nstdout(f(1).toString())\n"),
        "{md}"
    );
}

#[test]
fn doc_reads_lists_without_dashes_and_indentation_in_any_whitespace() {
    let dir = project(
        "f65_doc_shapes",
        &[(
            "main.td",
            // The second example line is indented with a full-width space.
            "///@ Params:\n///@   x: the first number\n///@   y: the second number\n///@ Example:\n///@   a <= 1\n///@ \u{3000}b <= 2\nadd x: Int y: Int =\n  x + y\n=> :Int\n\nPoint = @(\n  ///@ Purpose: the horizontal position\n  ///@ Deprecated: use pos instead\n  x: Int\n  ///@ Since: e.32\n  y: Int\n)\n",
        )],
    );
    let out = run(&["doc", "generate", "main.td"], &dir);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let md = stdout(&out);
    assert!(
        md.contains("`x`") && md.contains("the first number"),
        "{md}"
    );
    assert!(
        md.contains("`y`") && md.contains("the second number"),
        "{md}"
    );
    assert!(!md.contains("the first number y"), "{md}");
    assert!(md.contains("b <= 2"), "{md}");
    assert!(
        md.contains("the horizontal position; Deprecated: use pos instead"),
        "{md}"
    );
    assert!(md.contains("Since: e.32"), "{md}");
}

#[test]
fn doc_documents_enums_and_methods() {
    let dir = project(
        "f65_doc_enum",
        &[(
            "main.td",
            r#"///@ Purpose: colours
Enum => Color = :Red :Green :Blue

///@ Purpose: a point
Point = @(
  ///@ x coordinate
  x: Int
  ///@ Purpose: distance
  ///@ Params:
  ///@   - other: the other value
  ///@ Returns: the distance
  dist other: Int =
    x + other
  => :Int
)
"#,
        )],
    );
    let out = run(&["doc", "generate", "main.td"], &dir);
    let md = stdout(&out);
    assert!(
        md.contains("## Enums") && md.contains("### Color") && md.contains("| `Green` |"),
        "{md}"
    );
    assert!(md.contains("| `x` | `Int` | x coordinate |"), "{md}");
    assert!(md.contains("#### dist"), "{md}");
    assert!(md.contains("| `other` | `Int` | the other value |"), "{md}");
    assert!(
        !md.contains("| `dist` |"),
        "a method is not a field row: {md}"
    );
}

#[test]
fn doc_fails_when_a_file_cannot_be_read_or_parsed() {
    let dir = project(
        "f65_doc_failure",
        &[
            ("bad.td", "x <= (\n"),
            ("ok.td", "///@ Purpose: fine\nf x: Int =\n  x\n=> :Int\n"),
        ],
    );
    let bad = run(&["doc", "generate", "bad.td"], &dir);
    assert_eq!(code(&bad), 1, "{}", stderr(&bad));
    let missing = run(&["doc", "generate", "nonexistent.td"], &dir);
    assert_eq!(code(&missing), 1);
    // The directory is still documented, and the command fails.
    let both = run(&["doc", "generate", "."], &dir);
    assert_eq!(code(&both), 1);
    assert!(stdout(&both).contains("### f"), "{}", stdout(&both));
}

// ── REPL ────────────────────────────────────────────────────────────

fn repl(input: &str, dir: &Path, extra: &[&str]) -> Output {
    let mut child = Command::new(taida_bin())
        .args(extra)
        .current_dir(dir)
        .env("HOME", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn repl");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn repl_remembers_bindings_between_inputs() {
    let dir = unique_temp_dir("f65_repl_bindings");
    let out = repl("x <= 42\nx + 1\ny <= x * 2\ny\n", &dir, &[]);
    let text = stdout(&out);
    assert!(text.contains("  43") && text.contains("  84"), "{text}");
    assert!(
        !stderr(&out).contains("E1542") && !stderr(&out).contains("E1525"),
        "no false type errors: {}",
        stderr(&out)
    );
    // A real type error is still reported.
    let out = repl("x <= 1\nstr <= \"a\"\nstr + x\n", &dir, &[]);
    assert!(stderr(&out).contains("E1619"), "{}", stderr(&out));
}

#[test]
fn repl_checks_each_input_on_its_own_spans() {
    let dir = unique_temp_dir("f65_repl_setof");
    // Every input starts again at line 1, so the `setOf` argument of the first
    // input sits where the list of the second one does. Only the first is
    // excused from `[E0401]`, as in a file.
    let out = repl(
        "s <= setOf(@[1, \"x\"])\nxxxxxxx <= @[1, \"x\"]\n",
        &dir,
        &[],
    );
    let errors = stderr(&out);
    assert_eq!(errors.matches("[E0401]").count(), 1, "{errors}");
}

#[test]
fn repl_forgets_the_names_of_an_input_that_failed_to_run() {
    let dir = unique_temp_dir("f65_repl_failed_input");
    // `nope` does not exist, so `y` is never bound; binding it afterwards is
    // not a second definition.
    let out = repl("y <= nope\ny <= 5\ny\n", &dir, &[]);
    assert!(!stderr(&out).contains("E1501"), "{}", stderr(&out));
    assert!(stdout(&out).contains("  5"), "{}", stdout(&out));
    // A name that was bound stays known.
    let out = repl("z <= 1\nz <= 2\n", &dir, &[]);
    assert!(
        stderr(&out).contains("E1501") || stderr(&out).contains("already"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn repl_gives_up_waiting_when_more_lines_cannot_help() {
    let dir = unique_temp_dir("f65_repl_giving_up");
    // A bad escape in a plain string is reported at once and the next line
    // still runs.
    let out = repl("stdout(\"\\u{41\")\nstdout(\"next\")\n", &dir, &[]);
    assert!(
        stdout(&out).contains("next"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    // A definition without a return type is reported when a line back at the
    // margin ends its body, and the inputs after that run.
    let out = repl(
        "f x: Int =\n  x + 1\nstdout(\"ends it\")\nstdout(\"after\")\n",
        &dir,
        &[],
    );
    assert!(stderr(&out).contains("E1526"), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("after"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    // A comment after the `=` of a lambda does not end its body early.
    let out = repl(
        "f <= _ x: Int = // doubles\n  z <= x * 2\n  z + 1\n\nf(3)\n",
        &dir,
        &[],
    );
    assert!(
        stdout(&out).contains("  7"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn repl_does_not_swallow_the_line_after_an_unfinishable_input() {
    let dir = unique_temp_dir("f65_repl_operator");
    // No line break is allowed after an operator: the error is reported at
    // once and the next line runs as its own input.
    let out = repl("y <= 1 +\nstdout(\"next\")\n", &dir, &[]);
    assert!(
        stdout(&out).contains("next"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn repl_reads_past_comments_when_it_decides_whether_an_input_is_finished() {
    let dir = unique_temp_dir("f65_repl_comments");
    // A block comment after the `=` that opens a lambda's body.
    let out = repl(
        "g <= _ x: Int = /* triple */\n  y <= x * 3\n  y + 1\n\nstdout(g(2).toString())\n",
        &dir,
        &[],
    );
    assert!(
        stdout(&out).contains("7"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    // A comment line between the body and the return type.
    let out = repl(
        "add x: Int y: Int =\n  x + y\n// returns the sum\n=> :Int\nstdout(add(1, 2).toString())\n",
        &dir,
        &[],
    );
    assert!(
        stdout(&out).contains("3"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    assert!(!stderr(&out).contains("E1526"), "{}", stderr(&out));
}

#[test]
fn repl_starts_the_next_input_at_a_line_that_ends_a_lambda() {
    let dir = unique_temp_dir("f65_repl_margin");
    // `f(3)` is not part of the lambda above it.
    let out = repl("f <= _ n: Int =\n  n * 2\nf(3)\n", &dir, &[]);
    assert!(
        stdout(&out).contains("  6"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    // A failing statement after it leaves what the lambda bound known.
    let out = repl(
        "x <= 1\nf <= _ n: Int =\n  n * 2\nx <= 2\ng <= f\n",
        &dir,
        &[],
    );
    assert!(
        !stderr(&out).contains("Undefined variable 'f'"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn repl_keeps_the_spaces_of_a_template_that_runs_over_lines() {
    let dir = unique_temp_dir("f65_repl_template_spaces");
    let out = repl(
        "t <= `ab  \ncd`\nstdout(t.length().toString())\n",
        &dir,
        &[],
    );
    assert!(
        stdout(&out).contains("7"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn repl_does_not_end_a_lambda_at_a_comment_line() {
    let dir = unique_temp_dir("f65_repl_comment_margin");
    let out = repl(
        "f <= _ n: Int =\n  z <= n * 2\n// note\n  z + 1\n\nstdout(f(3).toString())\n",
        &dir,
        &[],
    );
    assert!(
        stdout(&out).contains("7"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn repl_keeps_a_blank_line_inside_a_template() {
    let dir = unique_temp_dir("f65_repl_template");
    let out = repl("t <= `line1\n\nline3`\nstdout(t)\n", &dir, &[]);
    assert!(
        stdout(&out).contains("line1\n\nline3"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
}

#[test]
fn repl_reads_multi_line_definitions_and_literals() {
    let dir = unique_temp_dir("f65_repl_multiline");
    let out = repl(
        "add x: Int y: Int =\n  x + y\n=> :Int\nadd(1, 2)\nxs <= @[1,\n2, 3]\nxs.length()\nf <= _ x: Int =\n  z <= x * 2\n  z + 1\n\nf(3)\n",
        &dir,
        &[],
    );
    let text = stdout(&out);
    let err = stderr(&out);
    assert!(text.contains("  3\n"), "add(1, 2): {text}\n{err}");
    assert!(text.contains("  7\n"), "f(3): {text}\n{err}");
    assert!(!err.contains("Undefined variable"), "{err}");
    assert!(!err.contains("E1526"), "{err}");
}

// ── taida init / auth / removed commands ────────────────────────────

#[test]
fn init_with_an_invalid_name_creates_nothing() {
    let dir = unique_temp_dir("f65_init");
    for name in ["Bad_Name", "my project", "a/B_c"] {
        let out = run(&["init", name], &dir);
        assert_eq!(code(&out), 1, "{name}");
    }
    let leftovers: Vec<_> = fs::read_dir(&dir).unwrap().flatten().collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    let ok = run(&["init", "good-name"], &dir);
    assert_eq!(code(&ok), 0, "{}", stderr(&ok));
    assert!(dir.join("good-name/packages.tdm").exists());
}

#[test]
fn auth_reports_a_damaged_credentials_file() {
    let dir = unique_temp_dir("f65_auth");
    fs::create_dir_all(dir.join(".taida")).unwrap();
    fs::write(dir.join(".taida/auth.json"), "{bad json").unwrap();
    let status = run(&["auth", "status"], &dir);
    assert_eq!(code(&status), 1, "{}", stdout(&status));
    assert!(
        stderr(&status).contains("not valid credentials data"),
        "{}",
        stderr(&status)
    );
    let logout = run(&["auth", "logout"], &dir);
    assert_eq!(code(&logout), 0, "{}", stderr(&logout));
    assert!(
        stdout(&logout).contains("Removed the unusable credentials file"),
        "{}",
        stdout(&logout)
    );
    assert!(!dir.join(".taida/auth.json").exists());
    let again = run(&["auth", "status"], &dir);
    assert_eq!(code(&again), 0);
    assert!(stdout(&again).contains("Not logged in"));
}

#[test]
fn removed_command_messages_carry_no_placeholder_version() {
    let dir = unique_temp_dir("f65_removed");
    let out = run(&["check", "x.td"], &dir);
    assert_eq!(code(&out), 2);
    let text = stderr(&out);
    assert!(
        text.contains("[E1700] Command 'check' was removed. Use 'taida way check' instead."),
        "{text}"
    );
    assert!(!text.contains("@e.X"), "{text}");
    let build = run(&["build", "--target", "native", "x.td"], &dir);
    assert!(!stderr(&build).contains("@e.X"), "{}", stderr(&build));
    let help = run(&["upgrade", "--help"], &dir);
    assert!(!stdout(&help).contains("@e.X"), "{}", stdout(&help));
}

// ── the checker and very deep expressions ───────────────────────────

fn chain(terms: usize) -> String {
    format!(
        "x <= {}\nstdout(x.toString())\n",
        vec!["1"; terms].join(" + ")
    )
}

#[test]
fn a_very_long_expression_is_checked_and_run() {
    let dir = project("f65_deep", &[("deep.td", &chain(6000))]);
    let check = run(&["way", "check", "deep.td"], &dir);
    assert_eq!(code(&check), 0, "{}{}", stdout(&check), stderr(&check));
    let ran = run(&["deep.td"], &dir);
    assert_eq!(code(&ran), 0, "{}{}", stdout(&ran), stderr(&ran));
    assert_eq!(stdout(&ran).trim(), "6000");
}

#[test]
fn the_analyses_walk_a_very_long_expression() {
    let dir = project("f65_deep_tools", &[("deep.td", &chain(6000))]);
    for args in [
        vec!["way", "lint", "deep.td"],
        vec!["way", "verify", "deep.td"],
        vec!["way", "todo", "deep.td"],
        vec!["graph", "deep.td"],
        vec!["graph", "summary", "deep.td"],
    ] {
        let out = run(&args, &dir);
        assert_eq!(
            code(&out),
            0,
            "taida {}: {}{}",
            args.join(" "),
            stdout(&out),
            stderr(&out)
        );
    }
}

#[test]
fn an_expression_too_deep_to_check_is_reported_not_crashed_on() {
    let dir = project("f65_too_deep", &[("deep.td", &chain(8300))]);
    let out = run(&["way", "check", "deep.td"], &dir);
    assert_eq!(code(&out), 1, "{}{}", stdout(&out), stderr(&out));
    assert!(
        stderr(&out).contains("nested too deeply"),
        "{}",
        stderr(&out)
    );
    let ran = run(&["deep.td"], &dir);
    assert_eq!(code(&ran), 1, "{}{}", stdout(&ran), stderr(&ran));
    assert!(
        stderr(&ran).contains("nested too deeply"),
        "{}",
        stderr(&ran)
    );
}

// ── imports of files that do not exist ──────────────────────────────

#[test]
fn a_missing_local_module_is_reported_before_the_program_runs() {
    let dir = project(
        "f65_missing_import",
        &[(
            "main.td",
            ">>> ./nonexistent.td => @(nothing)\nstdout(\"never\")\n",
        )],
    );
    let check = run(&["way", "check", "main.td"], &dir);
    assert_eq!(code(&check), 1);
    assert!(
        stderr(&check).contains("[E1701] Module './nonexistent.td' not found"),
        "{}",
        stderr(&check)
    );
    let ran = run(&["main.td"], &dir);
    assert_eq!(code(&ran), 1);
    assert!(stderr(&ran).contains("[E1701]"), "{}", stderr(&ran));
    assert!(!stdout(&ran).contains("never"));
}
