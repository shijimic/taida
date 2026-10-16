//! Mold options, method calls and function bodies on the compiled backends:
//! options given as variables or expressions take effect, a method call
//! evaluates its receiver and arguments once, a body ending in a binding
//! yields the bound value, and `CharAt` outside the string fails its Lax.
//! `Clamp`, `Length`, the `default` of `Div` / `Mod`, `Sort`, field defaults,
//! omitted Error fields and the Unicode case mapping of `Upper` / `Lower`
//! give the reference's results. `hashMap(entries)` builds a HashMap from a
//! list of `@(key, value)` packs or from a pack's fields on every backend.
//! The transcendental molds return correctly rounded values everywhere.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use f65_support::{ALL_COMPILED, assert_rejected, parity};

#[test]
fn mold_options_computed_at_run_time_take_effect() {
    parity(
        "f65_dynamic_mold_options",
        r#"no <= false
yes <= true
side <= "end"
stdout("[" + Trim["  hi  "](start <= no) + "]")
stdout("[" + Replace["a-b-c", "-", "+"](all <= yes) + "]")
stdout("[" + Pad["42", 5](side <= side, char <= "0") + "]")
trimEnd s: Str keepEnd: Bool =
  Trim[s](end <= !keepEnd)
=> :Str
stdout("[" + trimEnd("  x  ", true) + "]")
xs <= @[5, 3, 8, 1]
flag <= true
Sort[xs](reverse <= flag) >=> a
stdout(a)
Sort[xs](reverse <= 1 > 0) >=> c
stdout(c)
"#,
        "[  hi]\n[a+b+c]\n[42000]\n[x  ]\n@[8, 5, 3, 1]\n@[8, 5, 3, 1]",
        ALL_COMPILED,
    );
}

#[test]
fn char_at_outside_the_string_fails() {
    parity(
        "f65_char_at_bounds",
        r#"s <= "hello"
stdout(CharAt[s, 10]().has_value)
stdout(CharAt[s, -1]().has_value)
stdout(CharAt[s, 1]().has_value)
stdout("[" + CharAt[s, 10]().getOrDefault("dflt") + "]")
stdout("[" + CharAt[s, 4]().getOrDefault("dflt") + "]")
"#,
        "false\nfalse\ntrue\n[dflt]\n[o]",
        ALL_COMPILED,
    );
}

#[test]
fn method_calls_evaluate_receiver_and_arguments_once() {
    parity(
        "f65_method_single_eval",
        r#"Greeter = @(
  name: Str
  greet =
    "hi " + name
  => :Str
)

mk n: Str =
  stdout("mk " + n)
  Greeter(name <= n)
=> :Greeter

pred tag: Str =
  stdout("pred " + tag)
  _ x: Int = x > 1
=> :Int => :Bool

stdout(mk("a").greet())
xs <= @[1, 2, 3]
stdout(xs.any(pred("any")))
stdout(xs.all(pred("all")))
"#,
        "mk a\nhi a\npred any\ntrue\npred all\nfalse",
        ALL_COMPILED,
    );
}

#[test]
fn bodies_ending_in_a_binding_yield_its_value() {
    parity(
        "f65_tail_binding_bodies",
        r#"Calc = @(
  base: Int
  twice =
    r <= base * 2
  => :Int
)

outer n: Int =
  scale x: Int =
    r <= x * n
  => :Int
  scale(5)
=> :Int

c <= Calc(base <= 21)
stdout(c.twice())
stdout(outer(3))
"#,
        "42\n15",
        ALL_COMPILED,
    );
}

#[test]
fn methods_named_like_builtins_call_the_declared_method() {
    parity(
        "f65_method_builtin_names",
        r#"Counter = @(
  count: Int
  add n: Int =
    count + n
  => :Int
  get key: Str =
    key + "=" + count.toString()
  => :Str
  size =
    count * 2
  => :Int
  has n: Int =
    n == count
  => :Bool
  length =
    count * 100
  => :Int
  first =
    "first-" + count.toString()
  => :Str
)

viaParam c: Counter =
  c.add(5)
=> :Int

c0 <= Counter(count <= 3)
stdout(c0.add(5))
stdout(viaParam(c0))
stdout(c0.get("k"))
stdout(c0.size())
stdout(c0.has(3))
stdout(c0.length())
stdout(c0.first())
Map[@[c0], _ c: Counter = c.length()]() >=> ls
stdout(ls)
"#,
        "8\n8\nk=3\n6\ntrue\n300\nfirst-3\n@[300]",
        ALL_COMPILED,
    );
}

#[test]
fn clamp_length_and_div_defaults() {
    parity(
        "f65_clamp_length_div",
        r#"stdout(Clamp[5, 10, 1]())
stdout(Clamp[20, 10, 1]())
stdout(Clamp[2.5, 3.0, 1.0]())
stdout(Length[@[1, 2, 3]]())
stdout(Length["aあc"]())
Div[1, 0](default <= 7) >=> a
stdout(a)
Mod[1, 0](default <= 3) >=> b
stdout(b)
Div[1.0, 0.0](default <= 2.5) >=> c
stdout(c)
stdout(Div[1, 0](default <= 7).getOrDefault(99))
"#,
        "10\n10\n3.0\n3\n3\n7\n3\n2.5\n99",
        ALL_COMPILED,
    );
}

#[test]
fn a_throw_in_a_mold_default_reaches_the_ceiling() {
    parity(
        "f65_div_default_throw",
        r#"Error => Boom = @()
f =
  Div[1, 0](default <= Boom(message <= "b").throw())
=> :Lax[Int]
g =
  |== e: Error =
    stdout("caught " + e.message)
    0
  => :Int
  f().getOrDefault(5)
=> :Int
stdout(g())
"#,
        "caught b\n0",
        ALL_COMPILED,
    );
}

#[test]
fn sort_orders_like_the_reference() {
    parity(
        "f65_sort_order",
        r#"nan <= Sqrt[-1.0]()
xs <= @[3.0, nan, 1.0, 2.0]
Sort[xs]() >=> a
stdout(a)
Sort[xs](reverse <= true) >=> b
stdout(b)
words <= @["pear", "fig", "apple", "kiwi"]
Sort[words](by <= _ w: Str = w) >=> c
stdout(c)
Sort[words](by <= _ w: Str = w, reverse <= true) >=> d
stdout(d)
nums <= @[1.5, -2.5, 0.5, -0.5]
Sort[nums](by <= _ x: Float = x * 2.0) >=> e
stdout(e)
Sort[words](by <= _ w: Str = w.length(), reverse <= true) >=> f
stdout(f)
"#,
        "@[1.0, 2.0, 3.0, NaN]\n@[3.0, 2.0, 1.0, NaN]\n@[\"apple\", \"fig\", \"kiwi\", \"pear\"]\n@[\"pear\", \"kiwi\", \"fig\", \"apple\"]\n@[-2.5, -0.5, 0.5, 1.5]\n@[\"apple\", \"kiwi\", \"pear\", \"fig\"]",
        ALL_COMPILED,
    );
}

#[test]
fn omitted_fields_take_their_type_defaults() {
    parity(
        "f65_field_defaults",
        r#"Holder = @(
  m: HashMap[Str, Int]
  s: Set[Int]
  r: Result[Int, Str]
  o: Lax[Int]
  n: Int
)
h <= Holder(n <= 1)
stdout(h.m.size())
stdout(h.s.size())
stdout(h.r.isSuccess())
stdout(h.o.has_value)
stdout(h)
"#,
        "0\n0\ntrue\nfalse\n@(m <= HashMap({}), s <= Set({}), r <= @(__value <= 0, __predicate <= @(), throw <= @(type <= \"\", message <= \"\", __type <= \"Error\"), __type <= \"Result\"), o <= @(has_value <= false, __value <= 0, __default <= 0, __type <= \"Lax\"), n <= 1, __type <= \"Holder\")",
        ALL_COMPILED,
    );
}

#[test]
fn omitted_error_type_and_message_read_as_the_reference_does() {
    parity(
        "f65_error_fields",
        r#"Error => AppError = @(code: Int)
e1 <= AppError(code <= 1)
stdout("type=[" + e1.type + "] msg=[" + e1.message + "]")
e2 <= AppError(code <= 2, message <= "m")
stdout("type=[" + e2.type + "] msg=[" + e2.message + "]")
stdout(| e1.message == "" |> "empty" | _ |> "set")
stdout(jsonEncode(e1))
risky n: Int =
  | n > 0 |> AppError(code <= n).throw()
  | _ |> 0
=> :Int
run n: Int =
  |== e: Error =
    stdout("caught type=[" + e.type + "] msg=[" + e.message + "]")
    -1
  => :Int
  risky(n)
=> :Int
stdout(run(1))
"#,
        "type=[AppError] msg=[]\ntype=[AppError] msg=[m]\nempty\n{\"code\":1}\ncaught type=[AppError] msg=[]\n-1",
        ALL_COMPILED,
    );
}

#[test]
fn numeric_and_bytes_molds_at_their_edges() {
    // `Abs` drops the sign of -0.0; `Int[str, base]()` takes one optional
    // sign, accepts the smallest Int and fails past the range; the
    // fixed-width decodes need exactly their width; `Slice` cuts Bytes;
    // Int bounds clamp a Float value in f64.
    parity(
        "f65_numeric_bytes_edges",
        r#"stdout(Abs[-0.0]())
stdout(Int["--5", 10]().has_value)
stdout(Int["-+5", 10]().getOrDefault(-999))
stdout(Int["-8000000000000000", 16]().getOrDefault(0))
stdout(Int["-9223372036854775808", 10]().getOrDefault(0))
stdout(Int["+ff", 16]().getOrDefault(0))
stdout(Int["8000000000000000", 16]().has_value)
stdout(Int["ffffffffffffffff", 16]().getOrDefault(-999))
Bytes[@[1, 2, 3, 4]]() >=> four
stdout(U16BEDecode[four]().has_value)
stdout(U32BEDecode[four]().getOrDefault(0))
Bytes["hello"]() >=> b
stdout(Slice[b](start <= 1, end <= 3))
stdout(Slice[b](start <= 3).length())
stdout(Clamp[5.5, 0, 10]())
stdout(Clamp[12.5, 0, 10]())
"#,
        "0.0\nfalse\n-999\n-9223372036854775808\n-9223372036854775808\n255\nfalse\n-999\nfalse\n16909060\nBytes[@[101, 108]]\n2\n5.5\n10.0",
        ALL_COMPILED,
    );
}

#[test]
fn range_past_its_limit_names_the_saturated_count() {
    parity(
        "f65_range_limit_message",
        r#"safeLen s: Int e: Int =
  |== error: Error =
    stdout(error.type + ": " + error.message)
    -1
  => :Int
  range(s, e).length()
=> :Int
stdout(safeLen(0, 1000000))
stdout(safeLen(-9223372036854775807, 9223372036854775807))
stdout(safeLen(-1, 9223372036854775807))
"#,
        "1000000\nRangeError: range(start, end) would create 9223372036854775807 items; maximum is 1000000\n-1\nRangeError: range(start, end) would create 9223372036854775807 items; maximum is 1000000\n-1",
        ALL_COMPILED,
    );
}

#[test]
fn enum_fields_default_to_the_first_variant() {
    // An Enum value shows as its ordinal; an omitted Enum field holds the
    // first variant.
    parity(
        "f65_enum_field_default",
        r#"Enum => State = :Creating :Running :Stopped
Hive = @(name: Str, state: State)
d <= Hive(name <= "d")
stdout(d)
e <= Hive(name <= "e", state <= State:Running())
stdout(e)
stdout(d.state == State:Creating())
stdout(jsonEncode(d))
"#,
        "@(name <= \"d\", state <= 0, __type <= \"Hive\")\n@(name <= \"e\", state <= 1, __type <= \"Hive\")\ntrue\n{\"name\":\"d\",\"state\":\"Creating\"}",
        ALL_COMPILED,
    );
}

#[test]
fn mold_methods_are_called_on_the_instance() {
    parity(
        "f65_mold_methods",
        r#"Mold[T] => Box[T] = @(
  label: Str <= "box"
  hello =
    "hi"
  => :Str
  describe =
    label + ":" + filling.toString()
  => :Str
)
b <= Box[1]()
stdout(b.hello())
stdout(b.describe())
c <= Box[2](label <= "crate")
stdout(c.describe())
"#,
        "hi\nbox:1\ncrate:2",
        ALL_COMPILED,
    );
}

#[test]
fn a_type_called_without_fields_is_its_defaults() {
    // `Pilot()` is the instance of the type's defaults, as `Pilot(...)` with
    // every field omitted is.
    parity(
        "f65_ctor_without_fields",
        r#"Pilot = @(name: Str, age: Int <= 14)
p <= Pilot()
stdout(p)
stdout(p.age + 1)
"#,
        "@(name <= \"\", age <= 14, __type <= \"Pilot\")\n15",
        ALL_COMPILED,
    );
}

#[test]
fn upper_and_lower_map_every_unicode_letter() {
    // The whole Unicode case mapping, with the expansions (`ß` → `SS`) and
    // the final sigma of `Lower`, as the interpreter has it.
    parity(
        "f65_upper_lower_unicode",
        r#"stdout(Upper["éàü straße σς ａｂｃ ǆ ŉ ﬀ ΐ"]())
stdout(Lower["ÉÀÜ İSTANBUL ＡＢＣ Ǆ"]())
stdout(Lower["ΟΔΟΣ ΣΑΣ Σ AΣ. ΑΣʹΑ"]())
stdout(Lower["ΑΣ"]())
stdout(Upper["plain ascii 123"]())
stdout(Lower["MIXED Case ÄÖÜ"]())
"#,
        "ÉÀÜ STRASSE ΣΣ ＡＢＣ Ǆ ʼN FF \u{0399}\u{0308}\u{0301}\néàü i\u{0307}stanbul ａｂｃ ǆ\nοδος σας σ aς. ασʹα\nας\nPLAIN ASCII 123\nmixed case äöü",
        ALL_COMPILED,
    );
}

#[test]
fn hash_map_is_built_from_entries_or_a_pack() {
    parity(
        "f65_hashmap_from_entries",
        r#"Enum => Color = :Red :Green
m <= hashMap(@[@(key <= "a", value <= 1.5), @(key <= "b", value <= true), @(key <= "a", value <= 2.5)])
stdout(m)
stdout(m.size())
f <= hashMap(@[@(key <= "x", value <= 1.5), @(key <= "y", value <= 2.0)])
f.get("y") >=> y
stdout(y + 0.5)
stdout(hashMap(f.entries()))
named <= hashMap(@[@(value <= "one", key <= 1)])
named.get(1) >=> one
stdout(one)
stdout(hashMap(@(cpu <= 2, memory <= 512, mode <= "fast")))
colors <= hashMap(@[@(key <= "fg", value <= Color:Red()), @(key <= "bg", value <= Color:Green())])
stdout(jsonEncode(colors))
Rec = @(name: Str, color: Color)
stdout(jsonEncode(hashMap(Rec(name <= "r", color <= Color:Green()))))
mkKey i: Int = "k" + i.toString() => :Str
built <= hashMap(@[@(key <= mkKey(1), value <= mkKey(10)), @(key <= mkKey(2), value <= mkKey(20))])
stdout(built.remove("k1"))
stdout(hashMap(@[]).size())
"#,
        "HashMap({\"a\": 2.5, \"b\": true})\n2\n2.5\nHashMap({\"x\": 1.5, \"y\": 2.0})\none\nHashMap({\"cpu\": 2, \"memory\": 512, \"mode\": \"fast\"})\n{\"fg\":\"Red\",\"bg\":\"Green\"}\n{\"name\":\"r\",\"color\":\"Green\"}\nHashMap({\"k2\": \"k20\"})\n0",
        ALL_COMPILED,
    );
    assert_rejected(
        "f65_hashmap_from_scalar",
        "m <= hashMap(5)\nstdout(m)\n",
        "[E1506]",
    );
    assert_rejected(
        "f65_hashmap_from_ints",
        "m <= hashMap(@[1, 2])\nstdout(m)\n",
        "[E1506]",
    );
}

#[test]
fn transcendental_molds_return_correctly_rounded_values() {
    // Every value is the double nearest to the exact one (checked with
    // mpmath); the host libm returns a neighbour for each of the first
    // thirteen, so the backends agree because they share the kernel, not
    // the platform's libm.
    parity(
        "f65_correctly_rounded_math",
        r#"stdout(Log10[0.17]())
stdout(Log10[0.52]())
stdout(Sinh[0.12]())
stdout(Sinh[2.0]())
stdout(Cosh[0.4]())
stdout(Cosh[10.0]())
stdout(Tanh[0.02]())
stdout(Tanh[0.06]())
stdout(Exp[5.66]())
stdout(Exp[13.08]())
stdout(Sin[8.85]())
stdout(Sin[29.2]())
stdout(Atan[7.58]())
stdout(Sin[1e22]())
stdout(Cos[1e300]())
stdout(Pow[2.0, 0.5]())
stdout(Pow[-2.0, 3.0]())
stdout(Pow[10.0, -3.0]())
stdout(Atan2[1.0, -1.0]())
stdout(Asin[0.5]())
stdout(Acos[-0.999]())
stdout(Log[8.0, 2.0]())
stdout(Ln[0.0]())
"#,
        "-0.769551078621726\n-0.28399665636520083\n0.12028820743110907\n3.6268604078470186\n1.0810723718384547\n11013.232920103323\n0.019997333759930933\n0.059928103529143496\n287.1486425560543\n479260.70612947544\n0.5436484436660883\n-0.7990214786596138\n1.4396276803928358\n-0.8522008497671888\n-0.5753861119575491\n1.4142135623730951\n-8.0\n0.001\n2.356194490192345\n0.5235987755982989\n3.09686756642106\n3.0\n-inf",
        ALL_COMPILED,
    );
}
