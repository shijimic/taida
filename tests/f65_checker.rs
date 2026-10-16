//! Programs the checker rejects because every backend fails on them at run
//! time: calling a value that is not a function, one name imported from two
//! modules, piping into a value, adding operands `+` does not accept inside
//! a builtin's arguments, calling a function or type defined nowhere, giving
//! a builtin mold an argument of a kind it does not take, reading a field of
//! a value that has none, calling a method a value's kind does not have,
//! returning a value of another type than declared, binding a top-level
//! name a type already has, adding a pack without the fields of a list's
//! packs to that list, showing a name before anything binds it.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use f65_support::{assert_rejected, way_check_files};

#[test]
fn calling_a_local_that_hides_the_function_is_rejected() {
    assert_rejected(
        "f65_check_rebound_call",
        r#"f x: Int =
  f <= 3
  f(x)
=> :Int
stdout(f(-1))
"#,
        "[E1510]",
    );
}

#[test]
fn one_name_from_two_imports_is_rejected() {
    let (ok, out) = way_check_files(
        "f65_check_two_imports",
        &[
            ("a.td", "get n: Int =\n  n + 1\n=> :Int\n<<< @(get)\n"),
            ("b.td", "get n: Int =\n  n + 1000\n=> :Int\n<<< @(get)\n"),
            (
                "main.td",
                ">>> ./a.td => @(get)\n>>> ./b.td => @(get)\nstdout(get(1))\n",
            ),
        ],
    );
    assert!(
        !ok && out.contains("[E1501]"),
        "expected [E1501], got:\n{out}"
    );
    let (ok, out) = way_check_files(
        "f65_check_two_imports_aliased",
        &[
            ("a.td", "get n: Int =\n  n + 1\n=> :Int\n<<< @(get)\n"),
            ("b.td", "get n: Int =\n  n + 1000\n=> :Int\n<<< @(get)\n"),
            (
                "main.td",
                ">>> ./a.td => @(get)\n>>> ./b.td => @(get => getB)\nstdout(get(1) + getB(1))\n",
            ),
        ],
    );
    assert!(ok, "an aliased second import must pass:\n{out}");
}

#[test]
fn piping_into_a_value_is_rejected() {
    assert_rejected(
        "f65_check_pipe_into_value",
        r#"x <= "outer"
5 => x => y
stdout(y)
"#,
        "[E1544]",
    );
}

#[test]
fn mismatched_addition_inside_builtin_arguments_is_rejected() {
    for (tag, source) in [
        ("f65_check_add_lax", "l <= Div[1, 0]()\nstdout(\"x\" + l)\n"),
        ("f65_check_add_int", "n <= 5\nstdout(\"x\" + n)\n"),
        (
            "f65_check_add_nested",
            "l <= Div[1, 0]()\nstdout(\"a\" + (\"b\" + l))\n",
        ),
        ("f65_check_add_bool", "b <= true\nstdout(b + 1)\n"),
    ] {
        assert_rejected(tag, source, "[E1619]");
    }
    // Matching operands, and operands the checker cannot pin down, pass.
    let (ok, out) = way_check_files(
        "f65_check_add_ok",
        &[(
            "main.td",
            r#"s <= "a"
n <= 3
stdout(s + n.toString())
stdout(n + 2)
stdout(2.5 + n)
l <= Div[1, 0]()
stdout("x" + l.getOrDefault(0).toString())
"#,
        )],
    );
    assert!(ok, "expected no errors, got:\n{out}");
}

#[test]
fn calls_of_names_defined_nowhere_are_rejected() {
    assert_rejected(
        "f65_check_undefined_function",
        r#"helper x: Int =
  | x > 100 |> notDefinedAnywhere(x)
  | _ |> x + 1
=> :Int
stdout(helper(1))
"#,
        "[E1547]",
    );
    assert_rejected(
        "f65_check_undefined_type",
        "p <= Pilto(name <= \"a\")\nstdout(p)\n",
        "[E1547]",
    );
    // A function defined later in the file, or a local function, is not
    // undefined.
    let (ok, out) = way_check_files(
        "f65_check_defined_later",
        &[(
            "main.td",
            r#"first x: Int =
  second(x) + 1
=> :Int
second y: Int =
  y * 2
=> :Int
outer n: Int =
  inner k: Int =
    k + n
  => :Int
  inner(1)
=> :Int
stdout(first(3))
stdout(outer(4))
"#,
        )],
    );
    assert!(ok, "expected no errors, got:\n{out}");
}

#[test]
fn builtin_mold_arguments_of_the_wrong_kind_are_rejected() {
    for (tag, source) in [
        ("f65_check_mold_upper_int", "stdout(Upper[42]())\n"),
        ("f65_check_mold_sqrt_str", "stdout(Sqrt[\"x\"]())\n"),
        ("f65_check_mold_floor_lax", "stdout(Floor[Div[7, 2]()]())\n"),
        ("f65_check_mold_pow_exponent", "stdout(Pow[2, \"x\"]())\n"),
        (
            "f65_check_mold_repeat_float",
            "stdout(Repeat[\"a\", 2.0]())\n",
        ),
        ("f65_check_mold_reverse_int", "stdout(Reverse[42]())\n"),
        (
            "f65_check_mold_join_str",
            "stdout(Join[\"abc\", \",\"]())\n",
        ),
    ] {
        assert_rejected(tag, source, "[E1506]");
    }
    let (ok, out) = way_check_files(
        "f65_check_mold_args_ok",
        &[(
            "main.td",
            r#"s <= "abc"
n <= 4
stdout(Upper[s]())
stdout(Sqrt[n]())
stdout(Pow[2.5, n]())
stdout(Repeat[s, n]())
stdout(Reverse[@[1, 2]]())
stdout(Reverse[s]())
stdout(Join[@["a", "b"], ","]())
stdout(ToFixed[1.25, 1]())
"#,
        )],
    );
    assert!(ok, "expected no errors, got:\n{out}");
}

#[test]
fn fields_of_values_that_have_none_are_rejected() {
    for (tag, source) in [
        ("f65_check_field_list", "xs <= @[1, 2]\nstdout(xs.length)\n"),
        ("f65_check_field_str", "s <= \"abc\"\nstdout(s.length)\n"),
        ("f65_check_field_int", "n <= 42\nstdout(n.x)\n"),
        ("f65_check_field_lax", "l <= Div[1, 2]()\nstdout(l.value)\n"),
        (
            "f65_check_field_lax_is_empty",
            "l <= Lax[1]()\nstdout(l.isEmpty)\n",
        ),
    ] {
        assert_rejected(tag, source, "[E1602]");
    }
    let (ok, out) = way_check_files(
        "f65_check_field_ok",
        &[(
            "main.td",
            r#"xs <= @[1, 2]
stdout(xs.length())
l <= Lax[1]()
stdout(l.has_value)
stdout(l.isEmpty())
p <= @(a <= 1)
stdout(p.a)
"#,
        )],
    );
    assert!(ok, "expected no errors, got:\n{out}");
}

#[test]
fn expressions_inside_builtin_arguments_are_checked() {
    // A builtin takes a value of any type, but an operator or a method its
    // argument applies to operands of known types they cannot take fails
    // every time it runs.
    for (tag, source, code) in [
        ("f65_check_stdout_sub", "stdout(5 - \"a\")\n", "[E1619]"),
        ("f65_check_stdout_neg", "stdout(-true)\n", "[E1607]"),
        (
            "f65_check_json_neg",
            "x <= jsonEncode(-\"a\")\nstdout(x)\n",
            "[E1607]",
        ),
        (
            "f65_check_stdout_unknown_method",
            "stdout(\"A \" + \"\".isEmpty().toString())\n",
            "[E1509]",
        ),
    ] {
        assert_rejected(tag, source, code);
    }
}

#[test]
fn function_forms_that_exist_nowhere_are_rejected() {
    // `.toString()` is a method; the function forms are undefined names.
    for (tag, source) in [
        ("f65_check_fn_tostring", "stdout(toString(5))\n"),
        ("f65_check_fn_tostr", "stdout(toStr(5))\n"),
        ("f65_check_fn_typeof_camel", "stdout(typeOf(5))\n"),
    ] {
        assert_rejected(tag, source, "[E1547]");
    }
}

#[test]
fn a_call_that_reaches_a_later_definition_is_rejected() {
    // `first` is defined, but its body calls `second`, which top-level code
    // has not reached yet when `first(1)` runs.
    assert_rejected(
        "f65_check_indirect_forward",
        r#"first n: Int =
  second(n) + 1
=> :Int

stdout(first(1))

second n: Int =
  n * 10
=> :Int
"#,
        "[E1539]",
    );
    // After both definitions the same call is fine, as is mutual recursion.
    let (ok, out) = way_check_files(
        "f65_check_indirect_forward_ok",
        &[(
            "main.td",
            r#"isEven n: Int =
  | n == 0 |> true
  | _ |> isOdd(n - 1)
=> :Bool

isOdd n: Int =
  | n == 0 |> false
  | _ |> isEven(n - 1)
=> :Bool

stdout(isEven(4))
"#,
        )],
    );
    assert!(ok, "expected no errors, got:\n{out}");
}

#[test]
fn a_local_function_is_unknown_outside_its_body() {
    assert_rejected(
        "f65_check_local_fn_leak",
        r#"outer x: Int =
  helper z: Int =
    z + 1
  => :Int
  helper(x)
=> :Int
stdout(outer(1))
stdout(helper(2))
"#,
        "[E1547]",
    );
}

#[test]
fn class_like_fields_and_methods_keep_their_declared_types() {
    for (tag, source, code) in [
        (
            "f65_check_positional_ctor",
            "P = @(a: Int, b: Str)\np <= P(1, \"x\")\nstdout(p)\n",
            "[E1503]",
        ),
        (
            "f65_check_field_default",
            "P = @(a: Int <= \"x\", b: Str)\np <= P(b <= \"k\")\nstdout(p.a)\n",
            "[E1303]",
        ),
        (
            "f65_check_method_return",
            "T = @(\n  n: Int\n  f =\n    \"text\"\n  => :Int\n)\nt <= T(n <= 1)\nstdout(t.f())\n",
            "[E1601]",
        ),
        (
            "f65_check_method_body_op",
            "T = @(\n  n: Int\n  s: Str\n  bad =\n    n + s\n  => :Int\n)\nt <= T(n <= 1, s <= \"x\")\nstdout(t.bad())\n",
            "[E1619]",
        ),
    ] {
        assert_rejected(tag, source, code);
    }
}

#[test]
fn values_of_a_known_kind_keep_to_what_that_kind_accepts() {
    for (tag, source, code) in [
        (
            "f65_check_enum_return",
            "Enum => Color = :Red :Green :Blue\npick n: Int =\n  \"green\"\n=> :Color\nstdout(pick(1))\n",
            "[E1601]",
        ),
        (
            "f65_check_pack_method",
            "p <= @(a <= 1)\nstdout(p.nope())\n",
            "[E1509]",
        ),
        (
            "f65_check_pack_field_call",
            "p <= @(a <= 1)\nstdout(p.a())\n",
            "[E1509]",
        ),
        (
            "f65_check_map_method",
            "m <= hashMap().set(\"a\", 1)\nr <= m.nope()\nstdout(r)\n",
            "[E1509]",
        ),
        (
            "f65_check_map_method_in_arg",
            "m <= hashMap().set(\"a\", 1)\nstdout(m.length() > 0)\n",
            "[E1509]",
        ),
        (
            "f65_check_sleep_arg",
            "sleep(\"x\") >=> r\nstdout(r)\n",
            "[E1506]",
        ),
        (
            "f65_check_regex_arg",
            "r <= Regex(5)\nstdout(r)\n",
            "[E1506]",
        ),
        (
            "f65_check_div_arg",
            "Div[\"a\", \"b\"]() >=> q\nstdout(q)\n",
            "[E1506]",
        ),
        (
            "f65_check_join_arg",
            "ss <= @[\"a\", \"b\"]\nstdout(Join[ss, 1]())\n",
            "[E1506]",
        ),
        (
            "f65_check_generic_fixed",
            "make[T] =\n  1\n=> :T\nc <= make[Str]()\nstdout(c)\n",
            "[E1601]",
        ),
        (
            "f65_check_generic_fixed_inferred",
            "ident[T] x: T =\n  \"not T\"\n=> :T\nr <= ident(5)\nstdout(r)\n",
            "[E1601]",
        ),
        (
            "f65_check_handler_return",
            "Error => MyErr = @(field: Str)\n\nf n: Int =\n  |== e: MyErr =\n    \"oops\"\n  => :Str\n  | n > 0 |> MyErr(type <= \"MyErr\", message <= \"m\", field <= \"f\").throw()\n  | _ |> 1\n=> :Int\n\nstdout(f(1))\n",
            "[E1601]",
        ),
        (
            "f65_check_type_name_binding",
            "Thing = @(a: Int)\nThing <= 5\nstdout(Thing)\n",
            "[E1501]",
        ),
        (
            "f65_check_append_pack",
            "xs <= @[@(a <= 1)]\nAppend[xs, @(b <= 2)]() >=> zs\nstdout(zs)\n",
            "[E0401]",
        ),
        (
            "f65_check_concat_packs",
            "xs <= @[@(a <= 1)]\nys <= @[@(b <= 2)]\nConcat[xs, ys]() >=> zs\nstdout(zs)\n",
            "[E0401]",
        ),
    ] {
        assert_rejected(tag, source, code);
    }
    // A function-valued field, a generic function called with the type its
    // body returns, a type name shadowed inside a function, numbers added
    // to a list of numbers, a handler of the function's own type, and a
    // method called through a parameter declared with a pack type all run.
    let (ok, out) = way_check_files(
        "f65_check_known_kinds_ok",
        &[(
            "main.td",
            r#"p <= @(f <= _ x: Int = x + 1, n <= 2)
stdout(p.f(1))
defaultOf[T] =
  0
=> :T
stdout(defaultOf[Int]() + 1)
Thing = @(a: Int)
f n: Int =
  Thing <= n
  Thing + 1
=> :Int
stdout(f(3))
xs <= @[1, 2]
Append[xs, 2.5]() >=> a
stdout(a)
Error => MyErr = @(field: Str)
g n: Int =
  |== e: MyErr =
    0
  => :Int
  | n > 0 |> MyErr(type <= "MyErr", message <= "m", field <= "f").throw()
  | _ |> 1
=> :Int
stdout(g(1))
Pilot = @(
  name: Str
  greet =
    "hi " + name
  => :Str
)
hello p: @(name: Str) =
  p.greet()
=> :Str
stdout(hello(Pilot(name <= "Rei")))
"#,
        )],
    );
    assert!(ok, "expected no errors, got:\n{out}");
}

#[test]
fn hashmaps_built_by_set_keep_to_their_declared_types() {
    for (tag, source, code) in [
        (
            "f65_check_map_annotation",
            "m: HashMap[Str, Int] <= hashMap().set(\"a\", \"x\")\nstdout(m)\n",
            "Type mismatch in assignment to 'm'",
        ),
        (
            "f65_check_map_argument",
            "total m: HashMap[Str, Int] =\n  m.get(\"a\") >=> v\n  v + 1\n=> :Int\nm <= hashMap().set(\"a\", \"x\")\nstdout(total(m))\n",
            "[E1506]",
        ),
        (
            "f65_check_map_unmold_annotation",
            "pilots <= hashMap().set(\"Misato\", 29)\npilots.get(\"Misato\") >=> name: Str\nstdout(name)\n",
            "Type mismatch in unmold binding to 'name'",
        ),
    ] {
        assert_rejected(tag, source, code);
    }
    // A HashMap may hold values of several types; numbers fit a Float map.
    let (ok, out) = way_check_files(
        "f65_check_map_entries_ok",
        &[(
            "main.td",
            r#"m <= hashMap().set("a", 1).set("b", "x")
stdout(m)
m.get("a") >=> v: Int
stdout(v + 1)
n: HashMap[Str, Float] <= hashMap().set("a", 1)
stdout(n)
k <= hashMap().set("a", 1)
total h: HashMap[Str, Int] =
  h.get("a") >=> x
  x + 1
=> :Int
stdout(total(k))
"#,
        )],
    );
    assert!(ok, "expected no errors, got:\n{out}");
}

#[test]
fn nested_molds_and_branches_are_checked_in_little_time() {
    // Each level of nesting used to infer the levels below it two or three
    // times over, so the time doubled or tripled with every level.
    let mut map = "@[1]".to_string();
    for i in 0..16 {
        map = format!("Map[{map}, _ a{i}: Int = a{i} + 1]()");
    }
    let mut lambda = "v + 1".to_string();
    for _ in 0..14 {
        lambda = format!("Sum[Map[@[1, 2], _ v: Int = {lambda}]()]()");
    }
    let mut cond = "0".to_string();
    for _ in 0..30 {
        cond = format!("(| 1 == 1 |> {cond} | _ |> 0)");
    }
    let source = format!(
        "a <= {map}\nstdout(a)\nb <= Map[@[1], _ v: Int = {lambda}]()\nstdout(b)\nc <= {cond}\nstdout(c)\n"
    );
    let dir = common::unique_temp_dir("f65_check_nesting_time");
    std::fs::write(dir.join("main.td"), source).unwrap();
    let mut child = std::process::Command::new(common::taida_bin())
        .args(["way", "check"])
        .arg(dir.join("main.td"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if start.elapsed() > std::time::Duration::from_secs(60) {
            let _ = child.kill();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let _ = std::fs::remove_dir_all(&dir);
    let status = status.expect("`way check` of nested molds and branches took over 60 s");
    assert!(
        status.success(),
        "`way check` failed on nested molds and branches"
    );
}

#[test]
fn names_shown_before_they_are_bound_are_rejected() {
    // `stdout` / `stderr` arguments that are names, operators or templates
    // were not inferred, so a name bound later (or nowhere) passed the
    // check; the interpreter stops on it, and since imported modules run
    // where their import stands, a compiled program would read an unset
    // value.
    for (tag, source) in [
        ("f65_check_shown_nowhere", "stdout(zzz)\n"),
        (
            "f65_check_shown_before_assignment",
            "stdout(x + 1)\nx <= 1\n",
        ),
        (
            "f65_check_shown_in_template",
            "stderr(`x is ${x}`)\nx <= 1\n",
        ),
    ] {
        assert_rejected(tag, source, "[E1542]");
    }
    let (ok, out) = way_check_files(
        "f65_check_shown_before_import",
        &[
            ("lib.td", "seven <= 7\n<<< @(seven)\n"),
            (
                "main.td",
                "stdout(seven)\n>>> ./lib.td => @(seven)\nstdout(seven)\n",
            ),
        ],
    );
    assert!(
        !ok && out.contains("[E1542]"),
        "expected [E1542], got:\n{out}"
    );
    let (ok, out) = way_check_files(
        "f65_check_shown_after_import",
        &[
            ("lib.td", "seven <= 7\n<<< @(seven)\n"),
            (
                "main.td",
                ">>> ./lib.td => @(seven)\nstdout(seven)\nstdout(`${seven}`)\n",
            ),
        ],
    );
    assert!(ok, "names bound before they are shown must pass:\n{out}");
}
