//! The compiled backends take each value's kind (Int, Float, Bool, Str, list,
//! pack) from the checker's type of the expression, so values keep their
//! kind through variables that share a name, lambda parameters, pipelines,
//! closures, generic calls, template interpolation and imports, and
//! `jsonEncode` writes each value by that kind.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use f65_support::{ALL_COMPILED, parity, parity_files};

#[test]
fn same_named_variables_keep_their_own_kinds() {
    parity(
        "f65_same_name_kinds",
        r#"Fold[@[1, 2, 3, 4, 5], 0, _ acc: Int x: Int = acc + x]() >=> total

joinNames acc: Str name: Str =
  | acc == "" |> name
  | _ |> acc + ", " + name
=> :Str

label x: Str =
  "[" + x + "]"
=> :Str

double x: Int =
  x + x
=> :Int

stdout(total)
stdout(double(21))
stdout(label("a"))
stdout(joinNames("x", "y"))
"#,
        "15\n42\n[a]\nx, y",
        ALL_COMPILED,
    );
}

#[test]
fn float_lambdas_folds_and_sums_compute_in_float() {
    parity(
        "f65_float_hof",
        r#"xs <= @[1.5, 2.5, 3.5]
Map[xs, _ x = x * 2.0]() >=> ys
stdout(ys)
Fold[xs, 0.0, _ acc x = acc + x]() >=> s
stdout(s)
Filter[xs, _ x = x > 2.0]() >=> f
stdout(f)
Map[xs, _ x = x > 2.0]() >=> bs
stdout(bs)
items <= @[@(n <= "a", v <= 1.5, ok <= true), @(n <= "b", v <= 0.5, ok <= false)]
Map[items, _ i = i.v]() >=> vs
stdout(vs)
Sum[vs]() >=> total
stdout(total)
stdout(jsonEncode(items))
"#,
        "@[3.0, 5.0, 7.0]\n7.5\n@[2.5, 3.5]\n@[false, true, true]\n@[1.5, 0.5]\n2.0\n\
[{\"n\":\"a\",\"ok\":true,\"v\":1.5},{\"n\":\"b\",\"ok\":false,\"v\":0.5}]",
        ALL_COMPILED,
    );
}

#[test]
fn float_and_bool_flow_through_calls_pipelines_and_closures() {
    parity(
        "f65_kind_flow",
        r#"half x: Float =
  x * 0.5
=> :Float

pick flag: Bool =
  | flag |> 1.5
  | _ |> 2.5
=> :Float

isBig x: Int =
  x > 10
=> :Bool

3.0 => half => r
stdout(r)
3.0 => half(_) => v => If[v > 1.0, v, 0.0]() => c
stdout(c)
true => b => stdout(b.toString())
2.5 => stdout(_)
stdout(pick(false).toString())
stdout(@[isBig(1), isBig(20)])
k <= 2.5
flag <= true
scale <= _ x: Float = x * k
stdout(scale(2.0))
Map[@[1, 2], _ x = flag]() >=> fs
stdout(fs)
"#,
        "1.5\n1.5\ntrue\n2.5\n2.5\n@[false, true]\n5.0\n@[true, true]",
        ALL_COMPILED,
    );
}

#[test]
fn error_fields_and_generic_results_keep_their_kinds() {
    parity(
        "f65_error_generic_kinds",
        r#"Error => MyErr = @(code2: Int, ratio: Float, ok: Bool)

f x: Int =
  |== e: MyErr =
    e.message + ":" + e.code2.toString() + ":" + e.ratio.toString() + ":" + e.ok.toString()
  => :Str
  | x > 0 |> MyErr(type <= "MyErr", message <= "m", code2 <= 3, ratio <= 1.5, ok <= true).throw()
  | _ |> "fine"
=> :Str

ident[T] x: T =
  x
=> :T

stdout(f(1))
stdout(f(0))
stdout(ident(2.5))
stdout(ident(true))
stdout(ident("s"))
"#,
        "m:3:1.5:true\nfine\n2.5\ntrue\ns",
        ALL_COMPILED,
    );
}

#[test]
fn template_interpolation_keeps_the_interpolated_kind() {
    parity(
        "f65_template_kinds",
        r#"flag <= true
age <= 14
n <= 42
stdout(`${age}`)
stdout(`a:${n > 40}:${n}`)
ratio <= 1.5
stdout(`r=${ratio}`)
"#,
        "14\na:true:42\nr=1.5",
        ALL_COMPILED,
    );
}

#[test]
fn get_or_default_with_a_default_of_another_kind() {
    parity(
        "f65_get_or_default_kind",
        r#"rejected: Async[Str] <= AsyncReject["something went wrong"]()
stdout("rejected: " + rejected.getOrDefault(99).toString())
"#,
        "rejected: 99",
        ALL_COMPILED,
    );
}

#[test]
fn int_passed_for_float_computes_like_the_interpreter() {
    parity(
        "f65_int_for_float",
        r#"Point = @(x: Float, y: Float)
p <= Point(x <= 3, y <= 4)
stdout(p.x + 1.5)
stdout(p.y * 0.5)
half x: Float =
  x * 0.5
=> :Float
stdout(half(3))
stdout(half(3) + 1.0)
"#,
        "4.5\n2.0\n1.5\n2.5",
        ALL_COMPILED,
    );
}

#[test]
fn imported_values_keep_their_kinds() {
    parity_files(
        "f65_imported_value_kinds",
        &[
            (
                "lib.td",
                r#"rate <= 0.75
enabled <= true
title <= "hello"
suffix <= "world"
nums <= @[1.5, 2.5]
<<< @(rate, enabled, title, suffix, nums)
"#,
            ),
            (
                "main.td",
                r#">>> ./lib.td => @(rate, enabled, title, suffix, nums)
stdout(rate * 2.0)
stdout(enabled)
stdout(title + " " + suffix)
stdout(Sum[nums]())
"#,
            ),
        ],
        "1.5\ntrue\nhello world\n4.0",
        ALL_COMPILED,
    );
}

#[test]
fn mold_results_follow_their_arguments() {
    parity(
        "f65_mold_result_kinds",
        r#"stdout(Reverse["abc"]() + "!")
stdout(Slice[@[1, 2, 3, 4], 1, 3]())
"#,
        "cba!\n@[2, 3]",
        ALL_COMPILED,
    );
    parity(
        "f65_div_float_kind",
        r#"Div[5, 2.0]() >=> d
stdout(d)
"#,
        "2.5",
        &["native", "wasm-wasi"],
    );
}

#[test]
fn method_bodies_see_their_float_fields() {
    parity(
        "f65_method_float_fields",
        r#"Account = @(
  amount: Float
  bonus x: Float =
    amount + x
  => :Float
)
a <= Account(amount <= 1.5)
stdout(a.bonus(1.0))
stdout(a.amount + 1.0)
"#,
        "2.5\n2.5",
        ALL_COMPILED,
    );
}

#[test]
fn json_encode_writes_values_by_their_static_type() {
    parity(
        "f65_json_encode_kinds",
        r#"Enum => Color = :Red :Green
User = @(name: Str, age: Int, score: Float, tags: @[Str], color: Color)
u <= User(name <= "a", age <= 0, score <= 1.5, tags <= @["x"], color <= Color:Green())
stdout(jsonEncode(u))
stdout(jsonEncode(@(status <= 404, zero <= 0, big <= 120000, ok <= true)))
stdout(jsonEncode(0))
stdout(jsonEncode(@[0, 256, 120000]))
stdout(jsonEncode(Color:Red()))
stdout(jsonEncode(@[Color:Red(), Color:Green()]))
Tree = @(v: Int, kids: @[Tree])
t <= Tree(v <= 0, kids <= @[Tree(v <= 404, kids <= @[])])
stdout(jsonEncode(t))
stdout(jsonEncode(Result[0]()))
stdout(jsonEncode(Lax["hi"]()))
stdout(jsonPretty(@(a <= 0, b <= @[404, 0])))
"#,
        concat!(
            "{\"age\":0,\"color\":\"Green\",\"name\":\"a\",\"score\":1.5,\"tags\":[\"x\"]}\n",
            "{\"big\":120000,\"ok\":true,\"status\":404,\"zero\":0}\n",
            "0\n",
            "[0,256,120000]\n",
            "\"Red\"\n",
            "[\"Red\",\"Green\"]\n",
            "{\"kids\":[{\"kids\":[],\"v\":404}],\"v\":0}\n",
            "{\"__predicate\":{},\"__value\":0,\"throw\":{\"message\":\"\",\"type\":\"\"}}\n",
            "{\"__default\":\"\",\"__value\":\"hi\",\"has_value\":true}\n",
            "{\n  \"a\": 0,\n  \"b\": [\n    404,\n    0\n  ]\n}"
        ),
        ALL_COMPILED,
    );
}

#[test]
fn ints_are_never_taken_for_addresses() {
    parity(
        "f65_int_not_address",
        r#"build acc: @[Int] i: Int n: Int =
  | i >= n |> acc
  | _ |> build(Append[acc, i * 138](), i + 1, n)
=> :@[Int]
e: @[Int] <= @[]
xs <= build(e, 0, 20000)
Join[xs, ","]() >=> j
stdout(j.length())
y <= 4294967295
stdout(y.toString())
"abc".get(10) >=> d
stdout("[" + d + "]")
stdout("abc".get(10).toString())
"#,
        "151944\n4294967295\n[]\nLax(default: )",
        ALL_COMPILED,
    );
}

#[test]
fn list_accessors_and_relaxed_gorillax_keep_the_reference_defaults() {
    parity(
        "f65_list_lax_defaults",
        r#"names <= @["a", "b"]
stdout(names.get(5))
names.get(5) >=> x
stdout("[" + x + "]")
stdout(names.first())
fs <= @[1.5]
stdout(fs.get(3))
bs <= @[true]
stdout(bs.get(2))
Bytes["hi"]() >=> raw
stdout(jsonEncode(@(data <= raw)))
g <= Gorillax[42]().relax()
stdout(g)
"#,
        concat!(
            "@(has_value <= false, __value <= \"\", __default <= \"\", __type <= \"Lax\")\n",
            "[]\n",
            "@(has_value <= true, __value <= \"a\", __default <= \"\", __type <= \"Lax\")\n",
            "@(has_value <= false, __value <= 0.0, __default <= 0.0, __type <= \"Lax\")\n",
            "@(has_value <= false, __value <= false, __default <= false, __type <= \"Lax\")\n",
            "{\"data\":null}\n",
            "@(has_value <= true, __value <= 42, __error <= @(), __type <= \"RelaxedGorillax\")"
        ),
        ALL_COMPILED,
    );
}

#[test]
fn list_elements_of_a_type_parameter_keep_the_callers_kind() {
    // Inside `wrap`, `x` has the type parameter's type; each list element
    // takes its kind from the value the caller passed.
    parity(
        "f65_generic_list_elements",
        r#"wrap[T] x: T =
  @[x, x]
=> :@[T]

firstOf[T] xs: @[T] =
  xs.get(0).unmold()
=> :T

stdout(wrap(1.5))
stdout(wrap(true))
stdout(wrap("ab"))
stdout(wrap(@[1, 2]))
stdout(wrap(@(a <= 1.5)))
stdout(firstOf(wrap(0.25)))
stdout(jsonEncode(wrap(2.5)))
"#,
        concat!(
            "@[1.5, 1.5]\n",
            "@[true, true]\n",
            "@[\"ab\", \"ab\"]\n",
            "@[@[1, 2], @[1, 2]]\n",
            "@[@(a <= 1.5), @(a <= 1.5)]\n",
            "0.25\n",
            "[2.5,2.5]"
        ),
        ALL_COMPILED,
    );
}

#[test]
fn pack_fields_show_their_own_kind() {
    // Another pack's Bool field of the same name does not turn an Int
    // field into `true`.
    parity(
        "f65_field_own_kind",
        r#"settings <= @[@(name <= "debug", value <= true)]
port <= @(name <= "port", value <= 8080)
p <= @(a <= 1, b <= 3)
q <= @(a <= 1, b <= true)
stdout(port)
stdout(p)
stdout(q)
stdout(settings)
"#,
        concat!(
            "@(name <= \"port\", value <= 8080)\n",
            "@(a <= 1, b <= 3)\n",
            "@(a <= 1, b <= true)\n",
            "@[@(name <= \"debug\", value <= true)]"
        ),
        ALL_COMPILED,
    );
}

#[test]
fn hashmap_lookups_keep_the_value_kind() {
    parity(
        "f65_hashmap_lookup_kinds",
        r#"m <= hashMap().set("a", 1.5).set("b", 2.5)
m.get("a") >=> a
stdout(a)
b <= hashMap().set("x", true)
b.get("x") >=> bx
stdout(bx)
stdout(m.get("zzz"))
names <= hashMap().set("k", "v")
stdout(names.get("nope"))
"#,
        concat!(
            "1.5\n",
            "true\n",
            "@(has_value <= false, __value <= 0.0, __default <= 0.0, __type <= \"Lax\")\n",
            "@(has_value <= false, __value <= \"\", __default <= \"\", __type <= \"Lax\")"
        ),
        ALL_COMPILED,
    );
}

#[test]
fn lax_payloads_show_as_their_kind() {
    // A Lax of a Str, Float or Bool shows its value and default as that
    // kind, including defaults built from a type annotation.
    parity(
        "f65_lax_payload_kinds",
        r#"s <= "日本語🍣"
stdout(CharAt[s, 3]())
stdout(CharAt[s, 9]())
stdout(Max[@[true, false]]())
Outer = @(b: Bool, inner: @(a: Int, b: Str))
r <= Outer(b <= false)
stdout(r)
"#,
        concat!(
            "@(has_value <= true, __value <= \"🍣\", __default <= \"\", __type <= \"Lax\")\n",
            "@(has_value <= false, __value <= \"\", __default <= \"\", __type <= \"Lax\")\n",
            "@(has_value <= true, __value <= true, __default <= false, __type <= \"Lax\")\n",
            "@(b <= false, inner <= @(a <= 0, b <= \"\"), __type <= \"Outer\")"
        ),
        ALL_COMPILED,
    );
}

#[test]
fn json_floats_and_gorillax_values_use_the_reference_form() {
    // jsonEncode writes a Float in plain notation for decimal exponents
    // -5..15 and as `1e+21` / `1.5e-7` otherwise; Gorillax.toString()
    // shows its value by kind.
    parity(
        "f65_json_float_gorillax_form",
        r#"big <= 1000000000000000000000.0
tiny <= 0.00000015
stdout(jsonEncode(@[big, tiny, 0.00015, 100.0, 1.5, 123456789012345.0, 1234567890123456.0, 0.0]))
stdout(jsonEncode(@(v <= big * 1500000000000000000000.0)))
gs <= Gorillax["str"]()
gf <= Gorillax[1.5]()
gb <= Gorillax[true]()
stdout(gs.toString())
stdout(gf.toString())
stdout(gb.toString())
stdout(gf.relax().toString())
"#,
        concat!(
            "[1e+21,1.5e-7,0.00015,100.0,1.5,123456789012345.0,1234567890123456.0,0.0]\n",
            "{\"v\":1.5e+42}\n",
            "Gorillax(str)\n",
            "Gorillax(1.5)\n",
            "Gorillax(true)\n",
            "RelaxedGorillax(1.5)"
        ),
        ALL_COMPILED,
    );
}

#[test]
fn hashmap_entries_keep_their_kinds() {
    // Each entry keeps the kinds of its key and value: Bool and Float keys,
    // a map of mixed values, an Enum value (named by jsonEncode), list and
    // pack keys compared by content, and `==` entry by entry.
    parity(
        "f65_hashmap_entry_kinds",
        r#"Enum => Pri = :Low :Mid :High
m <= hashMap().set("pi", 3.14).set("flag", true).set("n", 7).set("p", Pri:High()).set("s", "x")
stdout(m)
stdout(jsonEncode(m))
stdout(m.values())
stdout(m.entries())
m.get("pi") >=> pi
stdout(pi + 1.0)
b <= hashMap().set(true, "yes").set(false, "no")
stdout(b)
stdout(b.keys())
stdout(jsonEncode(b))
f <= hashMap().set(1.5, "a").set(2.5, "b")
stdout(f.keys())
l <= hashMap().set(@[1, 2], "a").set(@[3], "b").set(@[1, 2], "c")
stdout(l.size())
stdout(l.has(@[1, 2]))
stdout(hashMap().set("a", 1).set("b", 2) == hashMap().set("a", 1).set("b", 2))
stdout(hashMap().set("a", 1).set("b", 2) == hashMap().set("b", 2).set("a", 1))
stdout(setOf(@[1, 2, 3]) == setOf(@[1, 2, 3]))
e <= hashMap()
e.get("zz") >=> z
stdout("[" + z + "]")
"#,
        concat!(
            "HashMap({\"pi\": 3.14, \"flag\": true, \"n\": 7, \"p\": 2, \"s\": \"x\"})\n",
            "{\"pi\":3.14,\"flag\":true,\"n\":7,\"p\":\"High\",\"s\":\"x\"}\n",
            "@[3.14, true, 7, 2, \"x\"]\n",
            "@[@(key <= \"pi\", value <= 3.14), @(key <= \"flag\", value <= true), @(key <= \"n\", value <= 7), @(key <= \"p\", value <= 2), @(key <= \"s\", value <= \"x\")]\n",
            "4.140000000000001\n",
            "HashMap({true: \"yes\", false: \"no\"})\n",
            "@[true, false]\n",
            "{}\n",
            "@[1.5, 2.5]\n",
            "2\n",
            "true\n",
            "true\n",
            "false\n",
            "true\n",
            "[]"
        ),
        ALL_COMPILED,
    );
}

#[test]
fn failed_lookups_take_the_static_element_default() {
    // `get` / `first` / `last` / `Find` that find nothing, and `get` on a
    // HashMap without the key, yield the default of the element's static
    // type: a pack of defaults, an empty list or map, a typed zero.
    parity(
        "f65_static_miss_defaults",
        r#"Pilot = @(name: Str, age: Int)
names: @[Str] <= @[]
names.first() >=> n
stdout("[" + n + "]")
pilots <= @[@(name <= "Rei", age <= 14)]
pilots.get(5) >=> p
stdout(p)
stdout(p.age)
typed: @[Pilot] <= @[]
typed.last() >=> t
stdout(t.name.length())
xs <= @[@["a"], @["b"]]
xs.get(9) >=> missing
stdout(missing.length())
missing.get(0) >=> m
stdout("[" + m + "]")
ms <= @[hashMap().set("a", 1)]
ms.get(5) >=> hm
stdout(hm)
counts: HashMap[Str, Int] <= hashMap()
counts.get("x") >=> c
stdout(c + 1)
ps <= @[@(a <= 1), @(a <= 2)]
Find[ps, _ q = q.a > 5]() >=> f
stdout(f)
"#,
        "[]\n@(name <= \"\", age <= 0)\n0\n0\n0\n[]\nHashMap({})\n1\n@(a <= 0)",
        ALL_COMPILED,
    );
}

#[test]
fn values_with_no_given_value_take_their_type_default() {
    // A function type's default is a function returning its return type's
    // default; a Result given no error reads, shows and encodes its `throw`
    // as the empty Error.
    parity(
        "f65_function_and_throw_defaults",
        r#"f <= _ x: Int = x + 1
fs <= @[f]
fs.get(3) >=> g
stdout(g)
stdout(g(5))
names <= @[_ s: Str = s + "!"]
names.first() >=> h0
stdout(h0("a"))
empty: @[Int => :Str] <= @[]
empty.last() >=> h1
stdout(h1(1).length())
Greeter = @(name: Str, greet: Str => :Str)
gg <= Greeter(name <= "a")
stdout(gg)
r <= Result[1, _ x: Int = x > 0]()
stdout(r)
stdout(jsonEncode(r))
stdout(r.throw)
t <= r.throw
stdout(t.message.length())
"#,
        "<function <lambda>>\n0\na!\n0\n@(name <= \"a\", greet <= <function <lambda>>, __type <= \"Greeter\")\n@(__value <= 1, __predicate <= <function <lambda>>, throw <= @(type <= \"\", message <= \"\", __type <= \"Error\"), __type <= \"Result\")\n{\"__predicate\":null,\"__value\":1,\"throw\":{\"message\":\"\",\"type\":\"\"}}\n@(type <= \"\", message <= \"\", __type <= \"Error\")\n0",
        ALL_COMPILED,
    );
}
