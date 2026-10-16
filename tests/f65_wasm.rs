//! WASM runtime regressions. Every case runs the same program on the
//! interpreter (the reference) and on the wasm-min / wasm-wasi / wasm-full
//! profiles and compares stdout and the exit code. The wasm half is
//! skipped when wasmtime is not installed.
//!
//! Covered: a throw stops the rest of its function and every caller up to
//! the `|==` ceiling; thrown Error subtypes keep their type and fields;
//! deep recursion no longer overwrites static data; HashMap Int keys and
//! set / remove churn; the Int range check of `Int[Str]()`; jsonEncode
//! escaping and non-finite Floats; Float display, parsing, `ToFixed`,
//! `Round` and the math molds; Bytes display; the profile-independent
//! features every wasm profile provides; non-ASCII identifiers; `><`
//! exit codes; the 128-bit integer support routines; the wasm-edge glue's
//! null-body statuses; and jsonEncode stopping past its depth limit as the
//! native runtime does.
mod common;

use common::{node_available, run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::{Command, Output};

const WASM_PROFILES: [&str; 3] = ["wasm-min", "wasm-wasi", "wasm-full"];

fn build(profile: &str, td: &Path, out: &Path) -> Output {
    Command::new(taida_bin())
        .args(["build", profile])
        .arg(td)
        .arg("-o")
        .arg(out)
        .output()
        .expect("taida build should run")
}

/// Build `source` for `profile` and run it under wasmtime in `dir` (with
/// `dir` preopened as `.`).
fn run_wasm(wasmtime: &Path, dir: &Path, profile: &str) -> Output {
    let td = dir.join("main.td");
    let wasm = dir.join(format!("{profile}.wasm"));
    let built = build(profile, &td, &wasm);
    assert!(
        built.status.success(),
        "{profile} build failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    Command::new(wasmtime)
        .current_dir(dir)
        .args(["run", "--dir=.", "--"])
        .arg(&wasm)
        .output()
        .expect("wasmtime should run")
}

fn write_program(prefix: &str, source: &str) -> std::path::PathBuf {
    let dir = unique_temp_dir(prefix);
    std::fs::write(dir.join("main.td"), source).unwrap();
    dir
}

/// The interpreter and every listed wasm profile print `expected` and
/// exit 0.
fn assert_parity_on(profiles: &[&str], prefix: &str, source: &str, expected: &str) {
    let dir = write_program(prefix, source);
    assert_eq!(
        run_interpreter(&dir.join("main.td")).as_deref(),
        Some(expected),
        "interpreter output"
    );
    if let Some(wasmtime) = wasmtime_bin() {
        for profile in profiles {
            let out = run_wasm(&wasmtime, &dir, profile);
            assert!(
                out.status.success(),
                "{profile} exited with {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim_end(),
                expected,
                "{profile} stdout"
            );
        }
    } else {
        eprintln!("SKIP: wasmtime is unavailable; only the interpreter was checked");
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn assert_parity(prefix: &str, source: &str, expected: &str) {
    assert_parity_on(&WASM_PROFILES, prefix, source, expected);
}

/// A throw leaves its function at once: no later statement of that
/// function or of any caller runs (a nested ceiling on the dead path used
/// to swallow the error, and Map / Fold kept calling back), and a second
/// throw on the way (a failing Result predicate) does not replace it.
#[test]
fn throw_stops_the_function_and_its_callers_until_the_ceiling() {
    assert_parity(
        "wasm_throw_propagation",
        r#"Error => AppError = @(code: Int)

check x: Int =
  | x < 0 |> AppError(type <= "AppError", message <= "neg " + x.toString(), code <= x).throw()
  | _ |> x
=> :Int

note x: Int =
  stdout("note " + x.toString())
  x
=> :Int

afterCall x: Int =
  |== e: Error =
    stdout("afterCall caught " + e.message)
    -1
  => :Int
  r <= check(x)
  stdout("afterCall ok " + r.toString())
  r
=> :Int

safe v: Int =
  |== e: Error =
    0
  => :Int
  v * 2
=> :Int

swallowed x: Int =
  |== e: Error =
    stdout("swallowed caught " + e.message)
    -1
  => :Int
  r <= check(x)
  safe(r) + 10
=> :Int

loop i: Int =
  v <= check(3 - i)
  stdout("i=" + v.toString())
  loop(i + 1)
=> :Int

tailLoop n: Int =
  |== e: Error =
    stdout("tailLoop caught " + e.message)
    -1
  => :Int
  loop(n)
=> :Int

viaMap xs: @[Int] =
  |== e: Error =
    stdout("map caught " + e.message)
    @[]
  => :@[Int]
  Map[xs, _ x = note(check(x))]()
=> :@[Int]

viaFold xs: @[Int] =
  |== e: Error =
    stdout("fold caught " + e.message)
    -1
  => :Int
  Fold[xs, 0, _ acc x = acc + note(check(x))]()
=> :Int

viaPipe x: Int =
  |== e: Error =
    stdout("pipe caught " + e.message)
    -1
  => :Int
  x => check(_) => note(_) => r
  r
=> :Int

inner2 x: Int =
  |== e: AppError =
    check(-99)
  => :Int
  check(x)
=> :Int

rethrown x: Int =
  |== e: Error =
    stdout("rethrown caught " + e.message)
    -4
  => :Int
  v <= inner2(x)
  stdout("rethrown must not print")
  v
=> :Int

predicate x: Int =
  |== e: Error =
    stdout("predicate caught " + e.type + " " + e.message)
    -3
  => :Int
  Result[x, _ v = check(v) > 100]() >=> r
  r
=> :Int

stdout(afterCall(5))
stdout(afterCall(-5))
stdout(swallowed(-5))
stdout(tailLoop(0))
stdout(viaMap(@[1, -2, 3]).length())
stdout(viaFold(@[1, 2, -3, 4]))
stdout(viaPipe(-7))
stdout(rethrown(-1))
stdout(predicate(-6))
stdout(predicate(500))
stdout("done")
"#,
        concat!(
            "afterCall ok 5\n",
            "5\n",
            "afterCall caught neg -5\n",
            "-1\n",
            "swallowed caught neg -5\n",
            "-1\n",
            "i=3\n",
            "i=2\n",
            "i=1\n",
            "i=0\n",
            "tailLoop caught neg -1\n",
            "-1\n",
            "note 1\n",
            "map caught neg -2\n",
            "0\n",
            "note 1\n",
            "note 2\n",
            "fold caught neg -3\n",
            "-1\n",
            "pipe caught neg -7\n",
            "-1\n",
            "rethrown caught neg -99\n",
            "-4\n",
            "predicate caught AppError neg -6\n",
            "-3\n",
            "500\n",
            "done",
        ),
    );
}

/// An Error subtype thrown through a failed Result, an AsyncReject or a
/// mapError keeps its own type and fields, so a typed ceiling catches it.
#[test]
fn thrown_error_subtypes_keep_their_type_and_fields() {
    assert_parity(
        "wasm_throw_payload",
        r#"Error => E1 = @(a: Int)

viaResult x: Int =
  |== error: Error =
    "caught " + error.type + ": " + error.message
  => :Str
  Result[x, _ v = v > 0](throw <= E1(type <= "E1", message <= "sub", a <= 1)) >=> v
  "ok " + v.toString()
=> :Str

viaAsync x: Int =
  |== error: E1 =
    "caught E1 a=" + error.a.toString() + " " + error.message
  => :Str
  AsyncReject[E1(type <= "E1", message <= "async sub", a <= 3)]() >=> v
  "ok"
=> :Str

viaMapError x: Int =
  |== error: E1 =
    "mapped " + error.type + " a=" + error.a.toString()
  => :Str
  Result[x, _ v = v > 0](throw <= Error(type <= "Base", message <= "base")).mapError(_ e = E1(type <= "E1", message <= "mapped", a <= 7)) >=> v
  "ok"
=> :Str

stdout(viaResult(5))
stdout(viaResult(-1))
stdout(viaAsync(1))
stdout(viaMapError(-1))
"#,
        concat!(
            "ok 5\n",
            "caught E1: sub\n",
            "caught E1 a=3 async sub\n",
            "mapped E1 a=7",
        ),
    );
}

/// A non-tail recursion 2000 calls deep ran the C stack into the string
/// literals and printed a wrong sum. The stack now sits below the static
/// data, so an overflow traps instead.
#[test]
fn deep_recursion_keeps_static_data_intact() {
    assert_parity(
        "wasm_deep_recursion",
        r#"walk n: Int =
  | n == 0 |> 0
  | _ |>
    s0 <= (n + 0).toString()
    s1 <= (n + 1).toString()
    s2 <= (n + 2).toString()
    s3 <= (n + 3).toString()
    s4 <= (n + 4).toString()
    s5 <= (n + 5).toString()
    s6 <= (n + 6).toString()
    s7 <= (n + 7).toString()
    s8 <= (n + 8).toString()
    s9 <= (n + 9).toString()
    s10 <= (n + 10).toString()
    s11 <= (n + 11).toString()
    s12 <= (n + 12).toString()
    s13 <= (n + 13).toString()
    s14 <= (n + 14).toString()
    s15 <= (n + 15).toString()
    s16 <= (n + 16).toString()
    s17 <= (n + 17).toString()
    s18 <= (n + 18).toString()
    s19 <= (n + 19).toString()
    s20 <= (n + 20).toString()
    s21 <= (n + 21).toString()
    s22 <= (n + 22).toString()
    s23 <= (n + 23).toString()
    r <= walk(n - 1)
    r + s0.length() + s1.length() + s2.length() + s3.length() + s4.length() + s5.length() + s6.length() + s7.length() + s8.length() + s9.length() + s10.length() + s11.length() + s12.length() + s13.length() + s14.length() + s15.length() + s16.length() + s17.length() + s18.length() + s19.length() + s20.length() + s21.length() + s22.length() + s23.length()
=> :Int
stdout(walk(100))
stdout(walk(1000))
stdout(walk(2000))
stdout("literal still intact")
"#,
        concat!("5055\n", "70155\n", "166155\n", "literal still intact",),
    );
}

/// Int keys whose hashes collide (1 and 3, 0 and 2) stay distinct, and
/// removals no longer exhaust the table: later insertions are kept.
#[test]
fn hashmap_int_keys_and_remove_churn_keep_every_entry() {
    assert_parity(
        "wasm_hashmap_keys",
        r#"m <= hashMap().set(3, "three").set(1, "one").set(0, "zero").set(2, "two")
stdout(m.size())
stdout(m.get(1).getOrDefault(""))
stdout(m.get(3).getOrDefault(""))
stdout(m)
stdout(m.entries())
a <= hashMap().set("a0", 0).set("a1", 1).set("a2", 2).set("a3", 3).set("a4", 4).set("a5", 5).set("a6", 6).set("a7", 7).set("a8", 8).set("a9", 9).set("a10", 10)
b <= a.remove("a0").remove("a1").remove("a2").remove("a3").remove("a4").remove("a5").remove("a6").remove("a7").remove("a8").remove("a9").remove("a10")
stdout(b.size())
c <= b.set("b0", 0).set("b1", 1).set("b2", 2).set("b3", 3).set("b4", 4).set("b5", 5).set("b6", 6).set("b7", 7).set("b8", 8).set("b9", 9).set("b10", 10)
stdout(c.size())
stdout(c.has("b10"))
stdout(c)
churn h: HashMap[Str, Int] i: Int =
  | i == 0 |> h
  | _ |> churn(h.set("tmp", i).remove("tmp"), i - 1)
=> :HashMap[Str, Int]
d <= churn(hashMap().set("keep", 1), 40).set("after", 2)
stdout(d)
"#,
        concat!(
            "4\n",
            "one\n",
            "three\n",
            "HashMap({3: \"three\", 1: \"one\", 0: \"zero\", 2: \"two\"})\n",
            "@[@(key <= 3, value <= \"three\"), @(key <= 1, value <= \"one\"), @(key <= 0, value <= \"zero\"), @(key <= 2, value <= \"two\")]\n",
            "0\n",
            "11\n",
            "true\n",
            "HashMap({\"b0\": 0, \"b1\": 1, \"b2\": 2, \"b3\": 3, \"b4\": 4, \"b5\": 5, \"b6\": 6, \"b7\": 7, \"b8\": 8, \"b9\": 9, \"b10\": 10})\n",
            "HashMap({\"keep\": 1, \"after\": 2})",
        ),
    );
}

/// `Int[Str]()` fails outside -2^63 .. 2^63-1 instead of wrapping, and
/// accepts the same grammar as the reference.
#[test]
fn int_parse_rejects_values_outside_the_int_range() {
    assert_parity(
        "wasm_int_parse",
        r#"Int["9223372036854775807"]() >=> a
stdout(a)
Int["-9223372036854775808"]() >=> b
stdout(b)
stdout(Int["9223372036854775808"]().hasValue())
stdout(Int["-9223372036854775809"]().hasValue())
stdout(Int["99999999999999999999"]().hasValue())
stdout(Int["00000000000000000000042"]().getOrDefault(-1))
stdout(Int["+7"]().getOrDefault(-1))
stdout(Int["-"]().hasValue())
stdout(Int["12a"]().hasValue())
"#,
        concat!(
            "9223372036854775807\n",
            "-9223372036854775808\n",
            "false\n",
            "false\n",
            "false\n",
            "42\n",
            "7\n",
            "false\n",
            "false",
        ),
    );
}

/// Float display is Rust's formatting: shortest round-trip digits, exact
/// digits for integral values up to the largest finite Float, `NaN`
/// without a sign; `Str[Float]()` is `f64::to_string`.
#[test]
fn float_display_matches_the_reference_formatting() {
    assert_parity(
        "wasm_float_display",
        r#"stdout(-947.8653606090633)
stdout(0.12345678901234568)
stdout(95488.93141911575)
stdout(0.00009474497007074875)
stdout(1.2345e-25)
stdout(1.0e-100)
stdout(5.0e-324)
stdout(1.7976931348623157e308)
stdout(-1.7976931348623157e308)
stdout(1.0e21)
stdout(1.0e23)
stdout(-0.0)
stdout(0.1 + 0.2)
stdout(@[1.5e-70, 0.0000001])
stdout(Sqrt[-1.0]())
stdout(Exp[1000.0]())
x <= 2.5
stdout(x.toString())
stdout(`v=${x}`)
Str[1.0e23]() >=> big
stdout(big)
Str[3.0]() >=> three
stdout(three)
Str[-0.0]() >=> negz
stdout(negz)
"#,
        concat!(
            "-947.8653606090633\n",
            "0.12345678901234568\n",
            "95488.93141911575\n",
            "0.00009474497007074875\n",
            "0.00000000000000000000000012345\n",
            "0.0000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001\n",
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000005\n",
            "179769313486231570814527423731704356798070567525844996598917476803157260780028538760589558632766878171540458953514382464234321326889464182768467546703537516986049910576551282076245490090389328944075868508455133942304583236903222948165808559332123348274797826204144723168738177180919299881250404026184124858368.0\n",
            "-179769313486231570814527423731704356798070567525844996598917476803157260780028538760589558632766878171540458953514382464234321326889464182768467546703537516986049910576551282076245490090389328944075868508455133942304583236903222948165808559332123348274797826204144723168738177180919299881250404026184124858368.0\n",
            "1000000000000000000000.0\n",
            "99999999999999991611392.0\n",
            "-0.0\n",
            "0.30000000000000004\n",
            "@[0.00000000000000000000000000000000000000000000000000000000000000000000015, 0.0000001]\n",
            "NaN\n",
            "inf\n",
            "2.5\n",
            "v=2.5\n",
            "100000000000000000000000\n",
            "3\n",
            "-0",
        ),
    );
}

/// `Float[Str]()` returns the correctly rounded value with the reference's
/// grammar.
#[test]
fn float_parse_is_correctly_rounded() {
    assert_parity(
        "wasm_float_parse",
        r#"Float["0.3"]() >=> a
stdout(a)
stdout(a == 0.3)
Float["3.14159"]() >=> b
stdout(b == 3.14159)
Float["0.30000000000000004"]() >=> c
stdout(c)
Float["99.99"]() >=> d
stdout(d)
Float["-352334.4703336753"]() >=> e
stdout(e)
Float["4.9e-324"]() >=> f
stdout(f)
Float["1.7976931348623157e308"]() >=> g
stdout(g)
Float["1e400"]() >=> h
stdout(h)
Float["2.406630e5"]() >=> i
stdout(i == 240663.0)
stdout(Float["."]().hasValue())
stdout(Float["1e"]().hasValue())
stdout(Float[" 1"]().hasValue())
stdout(Float["inf"]().hasValue())
Float["-Infinity"]() >=> j
stdout(j)
Float[".5"]() >=> k
stdout(k)
"#,
        concat!(
            "0.3\n",
            "true\n",
            "true\n",
            "0.30000000000000004\n",
            "99.99\n",
            "-352334.4703336753\n",
            "0.000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000005\n",
            "179769313486231570814527423731704356798070567525844996598917476803157260780028538760589558632766878171540458953514382464234321326889464182768467546703537516986049910576551282076245490090389328944075868508455133942304583236903222948165808559332123348274797826204144723168738177180919299881250404026184124858368.0\n",
            "inf\n",
            "true\n",
            "false\n",
            "false\n",
            "false\n",
            "true\n",
            "-inf\n",
            "0.5",
        ),
    );
}

/// `ToFixed` rounds the exact binary value half-to-even for any digit
/// count up to 20; `Round` is exact.
#[test]
fn to_fixed_and_round_match_the_reference() {
    assert_parity(
        "wasm_to_fixed",
        r#"stdout(ToFixed[2.675, 2]())
stdout(ToFixed[2.5, 0]())
stdout(ToFixed[0.5, 0]())
stdout(ToFixed[1.5, 0]())
stdout(ToFixed[0.125, 2]())
stdout(ToFixed[1.45, 1]())
stdout(ToFixed[1.5, 20]())
stdout(ToFixed[0.1, 19]())
stdout(ToFixed[123.456, 17]())
stdout(ToFixed[1.0e21, 2]())
stdout(ToFixed[-0.0, 2]())
stdout(ToFixed[-1.5, 0]())
stdout(ToFixed[3.14159, 2]())
stdout(Round[0.49999999999999994]())
stdout(Round[4503599627370497.0]())
stdout(Round[2.5]())
stdout(Round[-2.5]())
stdout(Round[-0.4]())
"#,
        concat!(
            "2.67\n",
            "2\n",
            "0\n",
            "2\n",
            "0.12\n",
            "1.4\n",
            "1.50000000000000000000\n",
            "0.1000000000000000056\n",
            "123.45600000000000307\n",
            "1000000000000000000000.00\n",
            "-0.00\n",
            "-2\n",
            "3.14\n",
            "0\n",
            "4503599627370497\n",
            "3\n",
            "-3\n",
            "0",
        ),
    );
}

/// Bytes values display as `Bytes[@[...]]` on every path, including the
/// Bytes mold on wasm-min.
#[test]
fn bytes_values_display_as_bytes() {
    assert_parity(
        "wasm_bytes_display",
        r#"Bytes["abc"]() >=> b
stdout(b)
stdout(b.toString())
stdout(b.length())
stdout(@[b])
stdout("" + b.toString())
Bytes[@[1, 255]]() >=> c
stdout(c)
U16BE[258]() >=> d
stdout(d)
"#,
        concat!(
            "Bytes[@[97, 98, 99]]\n",
            "Bytes[@[97, 98, 99]]\n",
            "3\n",
            "@[Bytes[@[97, 98, 99]]]\n",
            "Bytes[@[97, 98, 99]]\n",
            "Bytes[@[1, 255]]\n",
            "Bytes[@[1, 2]]",
        ),
    );
}

/// The shared `@()` value lives in static data: a tail loop's
/// per-iteration arena reset no longer reclaims it.
#[test]
fn empty_pack_survives_the_tail_loop_arena_reset() {
    assert_parity(
        "wasm_empty_pack",
        r#"spin n: Int acc: Int =
  | n == 0 |> acc
  | _ |>
    e <= @()
    p <= @(a <= n, b <= 2)
    Div[p.a, 1]() >=> q
    spin(n - 1, acc + q)
=> :Int
stdout(spin(3, 0))
stdout(@())
s <= "hello" + " world"
t <= "another" + " string"
stdout(@())
stdout(jsonEncode(@(a <= @())))
stdout(s)
g <= Gorillax[5]()
stdout(g)
"#,
        concat!(
            "6\n",
            "@()\n",
            "@()\n",
            "{\"a\":{}}\n",
            "hello world\n",
            "@(has_value <= true, __value <= 5, __error <= @(), __type <= \"Gorillax\")",
        ),
    );
}

/// Features that need no OS access build and run on every profile:
/// `range`, the byte-level Str molds, `StringRepeatJoin`, bitwise and
/// shift molds, Float `Div` / `Mod`, and types with Enum fields (whose
/// jsonEncode writes the variant name).
#[test]
fn profile_independent_features_work_on_every_profile() {
    assert_parity(
        "wasm_all_profile_features",
        r#"Enum => HiveState = :Creating :Running :Stopped
Hive = @(name: Str, state: HiveState)

stdout(range(0, 5))
stdout(range(3, 3).length())
ByteAt["abc", 1]() >=> b1
stdout(b1)
stdout(ByteAt["abc", 9]().hasValue())
stdout(ByteLength["héllo"]())
stdout(ByteSlice["héllo", 0, 3]())
stdout(ByteSlice["héllo", 2, 6]())
stdout(ByteSlice["héllo", 4, 1]())
stdout(StringRepeatJoin["ab", 3, "-"]())
stdout(StringRepeatJoin["ab", 0, "-"]())
stdout(BitAnd[12, 10]())
stdout(BitXor[12, 10]())
ShiftL[1, 4]() >=> sl
stdout(sl)
Div[1.0, 4.0]() >=> q
stdout(q)
Mod[5.5, 2.0]() >=> r
stdout(r)
h <= Hive(name <= "d", state <= HiveState:Running())
stdout(jsonEncode(h))
stdout(jsonEncode(@(items <= @[h, Hive(name <= "e", state <= HiveState:Stopped())])))
"#,
        concat!(
            "@[0, 1, 2, 3, 4]\n",
            "0\n",
            "98\n",
            "false\n",
            "6\n",
            "hé\n",
            "\u{fffd}llo\n",
            "él\n",
            "ab-ab-ab\n",
            "\n",
            "8\n",
            "6\n",
            "16\n",
            "0.25\n",
            "1.5\n",
            "{\"name\":\"d\",\"state\":\"Running\"}\n",
            "{\"items\":[{\"name\":\"d\",\"state\":\"Running\"},{\"name\":\"e\",\"state\":\"Stopped\"}]}",
        ),
    );
}

/// Distinct non-ASCII (and underscore-heavy) names map to distinct C
/// identifiers.
#[test]
fn non_ascii_identifiers_stay_distinct() {
    assert_parity(
        "wasm_identifiers",
        r#"あ <= 1
い <= 2
stdout(あ + い)
二倍 x: Int =
  x * 2
=> :Int
倍 x: Int =
  x * 3
=> :Int
stdout(二倍(5))
stdout(倍(5))
a_b <= 7
a__b <= 8
stdout(a_b * 10 + a__b)
"#,
        concat!("3\n", "10\n", "15\n", "78",),
    );
}

/// A Lax[Str] (`ToRadix`) encodes its value and default as strings.
#[test]
fn lax_str_results_encode_as_strings() {
    assert_parity(
        "wasm_lax_str_json",
        r#"r <= ToRadix[255, 16]()
stdout(jsonEncode(@(r <= r)))
bad <= ToRadix[255, 99]()
stdout(jsonEncode(@(bad <= bad)))
"#,
        concat!(
            "{\"r\":{\"__default\":\"\",\"__value\":\"ff\",\"has_value\":true}}\n",
            "{\"bad\":{\"__default\":\"\",\"__value\":\"\",\"has_value\":false}}",
        ),
    );
}

/// The statements after a throw run no side effects: the write on the
/// dead path used to create the file.
#[test]
fn dead_path_after_throw_performs_no_file_io() {
    assert_parity_on(
        &["wasm-wasi", "wasm-full"],
        "wasm_dead_path_io",
        r#">>> taida-lang/os => @(writeFile, Exists)

validate x: Int =
  | x < 0 |> Error(type <= "ValidationError", message <= "negative").throw()
  | _ |> x
=> :Int

save x: Int =
  |== error: Error =
    "rejected: " + error.message
  => :Str
  v <= validate(x)
  w <= writeFile("saved.txt", "value=" + v.toString())
  "saved"
=> :Str

stdout(save(-1))
Exists["saved.txt"]() >=> there
stdout("saved.txt exists: " + there.toString())
"#,
        concat!("rejected: negative\n", "saved.txt exists: false",),
    );
}

/// jsonEncode escapes every control character (the short forms `\b` and
/// `\f` included) and encodes NaN and the infinities as `null`, on native
/// and every wasm profile.
#[test]
fn json_encode_escapes_control_characters_and_nulls_non_finite_floats() {
    let source = r#"Bytes[@[1, 8, 12, 27, 31, 127]]() >=> bs
Utf8Decode[bs]() >=> ctrl
stdout(jsonEncode("a" + ctrl + "b"))
stdout(jsonEncode(@(k <= "tab\tnl\nq\"bs\\")))
nan <= Sqrt[-1.0]()
inf <= Exp[1000.0]()
ninf <= 0.0 - inf
stdout(jsonEncode(@[1.5, nan, inf, ninf]))
stdout(jsonEncode(@(a <= nan, b <= 2.5)))
"#;
    let expected = concat!(
        "\"a\\u0001\\b\\f\\u001b\\u001f\u{7f}b\"\n",
        "{\"k\":\"tab\\tnl\\nq\\\"bs\\\\\"}\n",
        "[1.5,null,null,null]\n",
        "{\"a\":null,\"b\":2.5}",
    );
    assert_parity("wasm_json_encode", source, expected);
    #[cfg(feature = "native")]
    {
        let dir = write_program("native_json_encode", source);
        let bin = dir.join("main.bin");
        let built = build("native", &dir.join("main.td"), &bin);
        assert!(
            built.status.success(),
            "native build failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        let out = Command::new(&bin).output().unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), expected);
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The math molds return the reference libm's results. Compared with the
/// interpreter at run time rather than pinned, since the reference is the
/// platform's libm.
#[test]
fn math_molds_match_the_interpreter() {
    let dir = write_program(
        "wasm_math",
        r#"stdout(Exp[1.0]())
stdout(Exp[-2.5]())
stdout(Exp[0.1]())
stdout(Ln[10.0]())
stdout(Ln[2.0]())
stdout(Ln[0.5]())
stdout(Log2[8.0]())
stdout(Log2[10.0]())
stdout(Log10[1000.0]())
stdout(Log10[2.0]())
stdout(Log[100.0, 10.0]())
stdout(Pow[2.0, 0.5]())
stdout(Pow[10.0, 0.5]())
stdout(Pow[9.0, 0.5]())
stdout(Pow[27.0, 0.3333333333333333]())
stdout(Pow[10.0, 308.0]())
stdout(Pow[2.0, 10.0]())
stdout(Pow[-2.0, 3.0]())
stdout(Sinh[1.0]())
stdout(Cosh[1.0]())
stdout(Tanh[0.5]())
stdout(Sin[1.0]())
stdout(Cos[1.0]())
stdout(Tan[1.0]())
stdout(Sin[1000000.0]())
stdout(Sin[3.141592653589793]())
stdout(Tan[1.5707963267948966]())
stdout(Asin[0.5]())
stdout(Acos[0.5]())
stdout(Atan[1.0]())
stdout(Atan2[1.0, 1.0]())
stdout(Atan2[-0.0, -1.0]())
stdout(Sqrt[2.0]())
"#,
    );
    let expected = run_interpreter(&dir.join("main.td")).expect("interpreter run");
    let Some(wasmtime) = wasmtime_bin() else {
        eprintln!("SKIP: wasmtime is unavailable");
        return;
    };
    for profile in WASM_PROFILES {
        let out = run_wasm(&wasmtime, &dir, profile);
        assert!(
            out.status.success(),
            "{profile}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            expected,
            "{profile} stdout"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// A recursion too deep for the stack stops with a failure exit and no
/// output, instead of continuing on overwritten data.
#[test]
fn runaway_recursion_stops_with_a_failure() {
    let Some(wasmtime) = wasmtime_bin() else {
        eprintln!("SKIP: wasmtime is unavailable");
        return;
    };
    let dir = write_program(
        "wasm_runaway_recursion",
        r#"down n: Int =
  | n == 0 |> 0
  | _ |>
    s <= n.toString()
    down(n - 1) + s.length()
=> :Int
stdout(down(1000000))
"#,
    );
    for profile in WASM_PROFILES {
        let out = run_wasm(&wasmtime, &dir, profile);
        assert!(
            !out.status.success(),
            "{profile} should stop with a failure"
        );
        assert!(out.stdout.is_empty(), "{profile} printed {:?}", out.stdout);
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// `><` exits with code 1 and prints nothing, like the interpreter (a trap
/// exited with 134).
#[test]
fn gorilla_exits_with_code_one() {
    let dir = write_program("wasm_gorilla_exit", "stdout(\"before\")\n><\n");
    let interp = Command::new(taida_bin())
        .arg(dir.join("main.td"))
        .output()
        .unwrap();
    assert_eq!(interp.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&interp.stdout), "before\n");
    if let Some(wasmtime) = wasmtime_bin() {
        for profile in WASM_PROFILES {
            let out = run_wasm(&wasmtime, &dir, profile);
            assert_eq!(out.status.code(), Some(1), "{profile} exit code");
            assert_eq!(
                String::from_utf8_lossy(&out.stdout),
                "before\n",
                "{profile} stdout"
            );
            assert!(out.stderr.is_empty(), "{profile} stderr");
        }
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// The 128-bit integer routines clang calls on wasm32 (no compiler-rt is
/// linked) compute exactly like Rust's i128 / u128. Generated code reaches
/// them when clang turns a self-recursive accumulation into its closed
/// form.
#[test]
fn int128_support_routines_compute_like_native_integers() {
    let Some(wasmtime) = wasmtime_bin() else {
        eprintln!("SKIP: wasmtime is unavailable");
        return;
    };
    let dir = unique_temp_dir("wasm_int128");
    let Ok(cache) = taida::codegen::driver::WasmRuntimeCache::new(dir.join("cache")) else {
        eprintln!("SKIP: clang is unavailable");
        return;
    };
    let core = cache
        .rt_core(taida::codegen::emit_wasm_c::WasmProfile::Min)
        .expect("core runtime object");

    let mut operands: Vec<i128> = vec![
        0,
        1,
        -1,
        2,
        7,
        -7,
        i128::MAX,
        i128::MIN,
        1 << 64,
        (1 << 64) - 1,
        -(1 << 64),
        i128::MAX - 12345,
    ];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..24 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let hi = state;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let v = ((hi as u128) << 64 | state as u128) as i128;
        operands.push(v >> (hi % 120));
    }
    let lit = |v: i128| {
        let u = v as u128;
        format!(
            "(((__int128)0x{:016x}ULL << 64) | (__int128)0x{:016x}ULL)",
            (u >> 64) as u64,
            u as u64
        )
    };
    let mut c = String::from(
        "#include <stdint.h>\n\
         int64_t taida_debug_int(int64_t v);\n\
         typedef __int128 i128;\ntypedef unsigned __int128 u128;\n",
    );
    c.push_str(&format!(
        "static volatile i128 ops[{}] = {{{}}};\n",
        operands.len(),
        operands
            .iter()
            .map(|&v| lit(v))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    c.push_str(
        "static void put(u128 v) { taida_debug_int((int64_t)(uint64_t)v); \
         taida_debug_int((int64_t)(uint64_t)(v >> 64)); }\n\
         int64_t _taida_main(void) {\n\
         int n = (int)(sizeof(ops) / sizeof(ops[0]));\n\
         for (int i = 0; i < n; i++) for (int j = 0; j < n; j++) {\n\
           i128 a = ops[i], b = ops[j]; int s = (int)((u128)b % 128);\n\
           put((u128)(a * b)); put((u128)a << s); put((u128)a >> s); put((u128)(a >> s));\n\
           if (b != 0) {\n\
             put((u128)a / (u128)b); put((u128)a % (u128)b);\n\
             if (!(b == -1 && a == (i128)((u128)1 << 127))) { put((u128)(a / b)); put((u128)(a % b)); }\n\
           }\n\
         }\n\
         return 0;\n}\n",
    );
    let mut expected = Vec::new();
    for &a in &operands {
        for &b in &operands {
            let s = ((b as u128) % 128) as u32;
            expected.push(a.wrapping_mul(b) as u128);
            expected.push((a as u128) << s);
            expected.push((a as u128) >> s);
            expected.push((a >> s) as u128);
            if b != 0 {
                expected.push((a as u128) / (b as u128));
                expected.push((a as u128) % (b as u128));
                if !(b == -1 && a == i128::MIN) {
                    expected.push((a / b) as u128);
                    expected.push((a % b) as u128);
                }
            }
        }
    }

    let src = dir.join("int128.c");
    let obj = dir.join("int128.o");
    let wasm = dir.join("int128.wasm");
    std::fs::write(&src, c).unwrap();
    let compiled = Command::new(cache.clang())
        .args([
            "--target=wasm32-unknown-wasi",
            "-nostdlib",
            "-O2",
            "-fwrapv",
            "-c",
            "-I",
        ])
        .arg(cache.include_dir())
        .arg(&src)
        .arg("-o")
        .arg(&obj)
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let linked = Command::new(taida::codegen::driver::find_wasm_ld().expect("wasm-ld"))
        .args(["--no-entry", "--export=_start", "--gc-sections"])
        .arg(&core)
        .arg(&obj)
        .arg("-o")
        .arg(&wasm)
        .output()
        .unwrap();
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    let out = Command::new(wasmtime).arg(&wasm).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let halves: Vec<u64> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.parse::<i64>().unwrap() as u64)
        .collect();
    let got: Vec<u128> = halves
        .chunks(2)
        .map(|p| (p[1] as u128) << 64 | p[0] as u128)
        .collect();
    assert_eq!(got.len(), expected.len());
    for (i, (g, e)) in got.iter().zip(&expected).enumerate() {
        assert_eq!(g, e, "result #{i}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// The wasm-edge glue returns 204 / 205 / 304 with the handler's status
/// (the Fetch Response constructor rejects a body on them, which surfaced
/// as an opaque 500), and names the problem for a 1xx status, which a
/// Fetch Response cannot carry.
#[test]
fn edge_glue_keeps_null_body_statuses() {
    if !node_available() || wasmtime_bin().is_none() {
        eprintln!("SKIP: node or the wasm toolchain is unavailable");
        return;
    }
    let dir = write_program(
        "wasm_edge_statuses",
        r#">>> taida-lang/abi => @(WebRequest, WebResponse, text, status)

handle req: WebRequest =
  Int[req.rawQuery]() >=> code
  status(code, text("x"))
=> :WebResponse
"#,
    );
    let wasm = dir.join("app.wasm");
    let built = Command::new(taida_bin())
        .args(["build", "wasm-edge", "--handler", "handle"])
        .arg(dir.join("main.td"))
        .arg("-o")
        .arg(&wasm)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let glue = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.to_string_lossy().ends_with(".edge.js"))
        .expect("wasm-edge build writes the JS glue");
    let harness = dir.join("run.mjs");
    std::fs::write(
        &harness,
        r#"import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
const gluePath = process.argv[2];
let src = fs.readFileSync(gluePath, "utf8");
const m = src.match(/import WASM from "\.\/([^"]+)";/);
const wasmPath = path.join(path.dirname(gluePath), m[1]);
src = src.replace(m[0], `const WASM = new WebAssembly.Module(fs.readFileSync(${JSON.stringify(wasmPath)}));`);
src = `import fs from "node:fs";\n` + src;
const tmp = path.join(path.dirname(gluePath), "glue.test.mjs");
fs.writeFileSync(tmp, src);
const glue = await import(pathToFileURL(tmp).href);
for (const code of [200, 204, 205, 304, 404, 100]) {
  const resp = await glue.handleTaidaRequest(new Request("http://h/p?" + code), {}, {});
  console.log(code + " -> " + resp.status + " " + JSON.stringify(await resp.text()));
}
"#,
    )
    .unwrap();
    let out = Command::new("node")
        .arg(&harness)
        .arg(&glue)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        concat!(
            "200 -> 200 \"x\"\n",
            "204 -> 204 \"\"\n",
            "205 -> 205 \"\"\n",
            "304 -> 304 \"\"\n",
            "404 -> 404 \"x\"\n",
            "100 -> 500 \"handler returned status 100, which a Fetch Response cannot carry\"",
        )
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn json_encode_past_the_depth_limit_stops_as_native_does() {
    // jsonEncode stops past 128 levels of nesting with the native runtime's
    // message and exit status, instead of running the stack out.
    let Some(wasmtime) = wasmtime_bin() else {
        return;
    };
    let td = write_program(
        "f65_wasm_json_depth",
        r#"Node = @(v: Int, children: @[Node])
build n: Int acc: Node =
  | n == 0 |> acc
  | _ |> build(n - 1, Node(v <= n, children <= @[acc]))
=> :Node
t <= build(80, Node(v <= 0, children <= @[]))
stdout("before")
stdout(jsonEncode(t).length())
"#,
    );
    let dir = td;
    let native_bin = dir.join("native.bin");
    assert!(
        build("native", &dir.join("main.td"), &native_bin)
            .status
            .success()
    );
    let native = Command::new(&native_bin).output().unwrap();
    assert_eq!(native.status.code(), Some(1));
    for profile in ["wasm-min", "wasm-wasi"] {
        let run = run_wasm(&wasmtime, &dir, profile);
        assert_eq!(run.status.code(), Some(1), "{profile}: exit status");
        assert_eq!(
            String::from_utf8_lossy(&run.stdout),
            "before\n",
            "{profile}: stdout"
        );
        assert!(
            String::from_utf8_lossy(&run.stderr).contains("JSON nesting depth limit exceeded"),
            "{profile}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
}
