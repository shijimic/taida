//! Errors on every backend: an Async rejected with a value that is not an
//! error reaches an `|== e: Error` handler as an `AsyncError` whose message
//! is the value's text, while the rejected Async itself still shows the
//! value. The built-in `Error` declares `type` and `message` only, so an
//! error type may declare `kind` / `code` of its own types; a caught error
//! has `kind` and `code` either way, and `jsonEncode` writes them. An error
//! type imported from a module keeps its parents for `TypeExtends`.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use f65_support::{ALL_COMPILED, assert_rejected, parity, parity_files};

#[test]
fn a_rejection_that_is_not_an_error_is_caught_as_an_async_error() {
    parity(
        "f65_async_reject_value",
        r#"Error => MyErr = @(field: Str)

fetchData id: Int =
  |== error: Error =
    @(found <= false, data <= "error: " + error.message)
  => :@(found: Bool, data: Str)

  result <= AsyncReject["timeout error"]()
  result >=> data

  @(found <= true, data <= data)
=> :@(found: Bool, data: Str)

r <= fetchData(1)
stdout(r.found.toString())
stdout(r.data)

go x: Int =
  |== e: Error =
    "caught: [" + e.type + "] [" + e.message + "]"
  => :Str
  a <= AsyncReject[42]()
  a >=> v
  "value"
=> :Str
stdout(go(1))

all x: Int =
  |== e: Error =
    "all: " + e.message
  => :Str
  All[@[Async[1](), AsyncReject["bad"]()]]() >=> xs
  "ok"
=> :Str
stdout(all(1))

mine x: Int =
  |== e: MyErr =
    "mine: " + e.field
  => :Str
  b <= AsyncReject[MyErr(type <= "MyErr", message <= "m", field <= "f")]()
  b >=> w
  "ok"
=> :Str
stdout(mine(1))

stdout(AsyncReject["x"]())
stdout(AsyncReject[true]())
stdout(AsyncReject[1.5]())
"#,
        "false\nerror: timeout error\ncaught: [AsyncError] [42]\nall: bad\nmine: f\nAsync[rejected: x]\nAsync[rejected: true]\nAsync[rejected: 1.5]",
        ALL_COMPILED,
    );
}

#[test]
fn a_rejecting_async_takes_the_value_type_its_context_needs() {
    parity(
        "f65_async_reject_context_type",
        r#"failing x: Int =
  | x > 0 |> AsyncReject["bad"]()
  | _ |> Async[x]()
=> :Async[Int]

handle x: Int =
  |== e: Error =
    0 - 1
  => :Int
  failing(x) >=> v
  v + 1
=> :Int

stdout(handle(1))
stdout(handle(0))
mixed <= @[Async[1](), AsyncReject["bad"]()]
stdout(mixed.length())
"#,
        "-1\n1\n2",
        ALL_COMPILED,
    );
}

#[test]
fn an_error_type_may_declare_kind_and_code_of_any_type() {
    parity(
        "f65_error_code_str",
        r#"Error => HttpError = @(status: Int, code: Str)
Error => MyErr = @(field: Str)

f x: Int =
  |== e: HttpError =
    "caught " + e.code + " " + e.kind
  => :Str
  | x == 0 |> HttpError(message <= "bad", status <= 404, code <= "NOT_FOUND").throw()
  | _ |> "ok"
=> :Str
stdout(f(0))
stdout(f(1))

g x: Int =
  |== e: MyErr =
    e.kind + ":" + e.code.toString() + ":" + e.field
  => :Str
  MyErr(message <= "m", field <= "f").throw()
  "no"
=> :Str
stdout(g(1))

e2 <= MyErr(message <= "m", field <= "f")
stdout(e2)
"#,
        "caught NOT_FOUND HttpError\nok\nMyErr:0:f\n@(field <= \"f\", message <= \"m\", __type <= \"MyErr\")",
        ALL_COMPILED,
    );
    // An error that was not caught has only the fields its type declares.
    assert_rejected(
        "f65_error_kind_not_caught",
        "Error => MyErr = @(field: Str)\ne2 <= MyErr(message <= \"m\", field <= \"f\")\nstdout(e2.kind)\n",
        "[E1602]",
    );
}

#[test]
fn a_caught_error_is_encoded_with_kind_and_code() {
    parity(
        "f65_caught_error_json",
        r#"f x: Int =
  |== e: Error =
    stdout(jsonEncode(e))
    stdout(e.code)
    e.message
  => :Str
  | x == 0 |> Error(type <= "Plain", message <= "m").throw()
  | _ |> "ok"
=> :Str
stdout(f(0))
"#,
        "{\"code\":0,\"kind\":\"Plain\",\"message\":\"m\",\"type\":\"Plain\"}\n0\nm",
        ALL_COMPILED,
    );
}

#[test]
fn an_imported_error_type_keeps_its_parents() {
    parity_files(
        "f65_typeextends_imported",
        &[
            (
                "errs.td",
                "Error => AppError = @(code: Int)\nAppError => DbError = @(table: Str)\n<<< @(AppError, DbError)\n",
            ),
            (
                "main.td",
                ">>> ./errs.td => @(AppError, DbError)\nstdout(TypeExtends[:DbError, :AppError]().toString())\nstdout(TypeExtends[:DbError, :Error]().toString())\nstdout(TypeExtends[:AppError, :Error]().toString())\nstdout(TypeExtends[:AppError, :DbError]().toString())\n",
            ),
        ],
        "true\ntrue\ntrue\nfalse",
        ALL_COMPILED,
    );
}

#[test]
fn results_failing_with_different_error_types_join() {
    // A Result's error is read through `errorInfo()`, which every error
    // gives, so arms failing with different error types have one type.
    let (ok, out) = f65_support::way_check_files(
        "f65_result_error_types_join",
        &[(
            "main.td",
            "Error => Fail = @(message: Str)\nsafe x: Int =\n  | x > 0 |> Result[x]()\n  | _ |> Result[0](throw <= Fail(message <= \"bad\"))\n=> :Result[Int, ErrorInfo]\nstdout(safe(1).isSuccess())\nstdout(safe(0).isSuccess())\n",
        )],
    );
    assert!(ok, "expected no errors, got:\n{out}");
}

#[test]
fn same_named_error_types_of_two_modules_are_different_types() {
    // `a.td`'s NotFound extends Error, `b.td`'s extends AppError: only the
    // latter is caught as an AppError, whichever module is imported first,
    // and both still show as `NotFound`.
    let a = "Error => NotFound = @(key: Str)\n\nfailA ak: Str =\n  NotFound(message <= \"a: not found\", key <= ak).throw()\n=> :Str\n\nisA x: NotFound = TypeIs[x, :NotFound]() => :Bool\n\n<<< @(failA, NotFound, isA)\n";
    let b = "Error => AppError = @(code: Int)\nAppError => NotFound = @(path: Str)\n\nfailB bp: Str =\n  NotFound(message <= \"b: not found\", code <= 404, path <= bp).throw()\n=> :Str\n\n<<< @(failB, AppError, NotFound)\n";
    let body = r#"
tryAs tag: Str =
  |== e: AppError =
    "caught as AppError: " + e.message
  => :Str
  | tag == "a" |> failA("k")
  | _ |> failB("p")
=> :Str

outer tag: Str =
  |== e: Error =
    "caught as Error: " + e.message + " (" + e.type + ")"
  => :Str
  tryAs(tag)
=> :Str

stdout(outer("a"))
stdout(outer("b"))
an <= ANotFound(message <= "m", key <= "q")
stdout(an)
stdout(isA(an))
stdout(TypeIs[an, :NotFound]().toString())
stdout(an.type)
stdout(TypeExtends[:NotFound, :AppError]().toString())
stdout(TypeExtends[:ANotFound, :AppError]().toString())
"#;
    let expected = "caught as Error: a: not found (NotFound)\ncaught as AppError: b: not found\n@(key <= \"q\", message <= \"m\", __type <= \"ANotFound\")\ntrue\nfalse\nANotFound\ntrue\nfalse";
    let imports_a_first = ">>> ./a.td => @(failA, NotFound => ANotFound, isA)\n>>> ./b.td => @(failB, AppError, NotFound)\n";
    let imports_b_first = ">>> ./b.td => @(failB, AppError, NotFound)\n>>> ./a.td => @(failA, NotFound => ANotFound, isA)\n";
    for (tag, imports) in [
        ("f65_error_name_collision", imports_a_first),
        ("f65_error_name_collision_swapped", imports_b_first),
    ] {
        parity_files(
            tag,
            &[
                ("a.td", a),
                ("b.td", b),
                ("main.td", &format!("{imports}{body}")),
            ],
            expected,
            ALL_COMPILED,
        );
    }
}

#[test]
fn a_local_error_type_may_extend_an_imported_one() {
    // The child gets the imported parent's fields and is an error type, so
    // it can be built with them, thrown and caught as the parent.
    parity_files(
        "f65_extend_imported_error",
        &[
            (
                "b.td",
                "Error => AppError = @(code: Int)\n<<< @(AppError)\n",
            ),
            (
                "main.td",
                r#">>> ./b.td => @(AppError)
AppError => DbError = @(table: Str)
f x: Int =
  |== e: AppError =
    "caught " + e.type + " " + e.message + " " + e.code.toString()
  => :Str
  DbError(message <= "db down", code <= 500, table <= "t").throw()
  "no"
=> :Str
stdout(f(1))
stdout(TypeExtends[:DbError, :AppError]().toString())
d <= DbError(message <= "m", code <= 1, table <= "t")
stdout(d.table + " " + d.code.toString())
"#,
            ),
        ],
        "caught DbError db down 500\ntrue\nt 1",
        ALL_COMPILED,
    );
}
