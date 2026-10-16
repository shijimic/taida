//! Native runtime memory safety, the JSON reader, and native runtime
//! value parity: each program must print the interpreter's output on the
//! compiled backends (WASM runs only when wasmtime is installed).
#![cfg(feature = "native")]
mod common;
use common::{run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn build(profile: &str, td: &Path, out: &Path) -> Output {
    Command::new(taida_bin())
        .args(["build", profile])
        .arg(td)
        .arg("-o")
        .arg(out)
        .output()
        .unwrap()
}

fn run_built(profile: &str, bin: &Path, cwd: &Path) -> Output {
    if profile == "native" {
        Command::new(bin).current_dir(cwd).output().unwrap()
    } else {
        Command::new(wasmtime_bin().unwrap())
            .current_dir(cwd)
            .args(["run", "--dir=."])
            .arg(bin)
            .output()
            .unwrap()
    }
}

/// Runs `source` on the interpreter and on each profile; every backend
/// must succeed and print `expected`.
fn parity_on(profiles: &[&str], source: &str, expected: &str) {
    let dir = unique_temp_dir("f65_nrt");
    let td = dir.join("main.td");
    std::fs::write(&td, source).unwrap();
    assert_eq!(
        run_interpreter(&td).expect("interpreter run").trim_end(),
        expected,
        "interpreter"
    );
    for profile in profiles {
        if *profile != "native" && wasmtime_bin().is_none() {
            continue;
        }
        let bin = dir.join(profile);
        let built = build(profile, &td, &bin);
        assert!(
            built.status.success(),
            "{profile} build: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        let out = run_built(profile, &bin, &dir);
        assert!(
            out.status.success(),
            "{profile}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            expected,
            "{profile}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn parity(source: &str, expected: &str) {
    parity_on(&["native", "wasm-min", "wasm-wasi"], source, expected);
}

/// Compiles the native runtime C with a test `main` (`body`) and runs it.
/// `sanitize` adds AddressSanitizer + UBSan.
fn native_harness(body: &str, sanitize: bool) -> Output {
    let dir = unique_temp_dir("f65_nrt_harness");
    let src = dir.join("check.c");
    let bin = dir.join("check");
    let runtime = *taida::codegen::native_runtime::NATIVE_RUNTIME_C;
    std::fs::write(
        &src,
        format!(
            "#define main embedded_main\n{runtime}\n#undef main\nint64_t _taida_main(void) {{ return 0; }}\n{body}"
        ),
    )
    .unwrap();
    let mut cmd = Command::new("cc");
    cmd.args(["-O1", "-ffunction-sections", "-fdata-sections"]);
    if sanitize {
        cmd.args(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"]);
    }
    let built = cmd
        .arg(&src)
        .args(["-Wl,--gc-sections", "-lm", "-lpthread", "-ldl", "-o"])
        .arg(&bin)
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let out = Command::new(&bin)
        .env("ASAN_OPTIONS", "detect_leaks=0:halt_on_error=1")
        .env("UBSAN_OPTIONS", "halt_on_error=1:print_stacktrace=1")
        .output()
        .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    out
}

// ── HashMap ──────────────────────────────────────────────────────────

#[test]
fn hashmap_set_remove_churn_keeps_the_order_index_in_bounds() {
    // Every insertion takes an ordinal in the insertion-order index; a
    // map whose size stays small never grew, so churn wrote past the
    // index. Native only: the WASM HashMap is fixed separately.
    parity_on(
        &["native"],
        r#"churn m: HashMap[Str, Int] i: Int =
  | i == 0 |> m
  | _ |> churn(m.set("k" + i.toString(), i).remove("k" + i.toString()), i - 1)
=> :HashMap[Str, Int]
m <= churn(hashMap().set("keep", 1), 200)
stdout(m.size())
stdout(m.set("after", 2))
stdout(m.set("after", 2).keys())
churnInt m: HashMap[Int, Int] i: Int =
  | i == 0 |> m
  | _ |> churnInt(m.set(i, i).remove(i), i - 1)
=> :HashMap[Int, Int]
stdout(churnInt(hashMap().set(0, 0), 100).set(99, 1))
"#,
        "1\nHashMap({\"keep\": 1, \"after\": 2})\n@[\"keep\", \"after\"]\nHashMap({0: 0, 99: 1})",
    );
}

#[test]
fn short_string_keys_keep_matching_their_entries() {
    // Each `.set` copies the map, so memory fills with maps and Lax
    // values. A two-character key read as a pack header pointed far ahead,
    // where another pack's tail sentinel could sit; the WASM key
    // comparison then treated the key as a pack and added "k8" twice.
    parity(
        r#"loop i: Int m: HashMap[Str, Int] =
  | i == 0 |> m.size()
  | _ |> loop(i - 1, m.set("k" + Mod[i, 50]().getOrDefault(0).toString(), i))
=> :Int
stdout(loop(50000, hashMap()))
"#,
        "50",
    );
}

#[test]
fn packs_may_use_any_number_of_field_names() {
    // 300 field names, then the runtime's own (a Lax, an Enumerate pair).
    // The compiled backends kept 256 names and left the fields past them
    // out of display and jsonEncode.
    let mut src = String::new();
    for p in 0..30 {
        let fields: Vec<String> = (0..10)
            .map(|i| {
                let n = p * 10 + i;
                format!("fld{n:04} <= {n}")
            })
            .collect();
        src.push_str(&format!("p{p} <= @({})\n", fields.join(", ")));
    }
    src.push_str("stdout(p29)\nstdout(jsonEncode(p29))\nstdout(Div[1, 0]())\n");
    src.push_str("Enumerate[@[\"a\"]]() >=> en\nstdout(en)\n");
    let shown: Vec<String> = (290..300).map(|n| format!("fld{n:04} <= {n}")).collect();
    let json: Vec<String> = (290..300).map(|n| format!("\"fld{n:04}\":{n}")).collect();
    let expected = format!(
        "@({})\n{{{}}}\n@(has_value <= false, __value <= 0, __default <= 0, __type <= \"Lax\")\n@[@(index <= 0, value <= \"a\")]",
        shown.join(", "),
        json.join(",")
    );
    parity(&src, &expected);
}

#[test]
fn functions_read_any_number_of_top_level_values() {
    // Native kept top-level values in a fixed table of 256 slots and
    // dropped the rest, so functions read 0 for them.
    let mut src = String::new();
    for i in 0..300 {
        src.push_str(&format!("v{i} <= {i}\n"));
    }
    let sum: Vec<String> = (0..300).map(|i| format!("v{i}")).collect();
    src.push_str(&format!(
        "total =\n  {}\n=> :Int\nstdout(total())\nlast =\n  v299\n=> :Int\nstdout(last())\n",
        sum.join(" + ")
    ));
    parity(&src, "44850\n299");
}

#[test]
fn todo_stream_and_molten_values_show_as_the_interpreter_shows_them() {
    // `toString()` read a TODO value's `__type` and a Stream as numbers on
    // native; a Molten value showed its internal pack on both compiled
    // backends.
    parity(
        r#"t <= TODO[Int](id <= "T-1", task <= "later", sol <= 5)
stdout(t.toString())
stdout(`in a template: ${t.toString()}`)
s <= Stream[42]()
stdout(s.toString())
v <= Stub["inner"]()
stdout(v)
stdout(`v=${v}`)
stdout(@[v])
stdout(@(k <= v))
"#,
        "@(id <= \"T-1\", task <= \"later\", sol <= 5, unm <= 0, __value <= 5, __default <= 0, __type <= \"TODO\")\nin a template: @(id <= \"T-1\", task <= \"later\", sol <= 5, unm <= 0, __value <= 5, __default <= 0, __type <= \"TODO\")\nStream[completed: 1 items]\nMolten\nv=Molten\n@[Molten]\n@(k <= Molten)",
    );
}

#[test]
fn ints_just_below_two_to_the_32_are_written_as_numbers() {
    // WASM's pointer check added the probe size to the value in 32 bits,
    // so an Int just below 2^32 wrapped around, passed as a pointer and
    // was read outside memory.
    parity(
        r#"stdout(jsonEncode(@(l <= 4294967295, m <= 4294967290, n <= 4294967296)))
stdout(jsonEncode(@[4294967295, 4294967288, 4294967280]))
"#,
        "{\"l\":4294967295,\"m\":4294967290,\"n\":4294967296}\n[4294967295,4294967288,4294967280]",
    );
}

#[test]
fn random_bytes_throws_a_catchable_crypto_error_for_a_bad_count() {
    // The reference throws, as the docs say; the interpreter used to stop
    // with an error no handler could catch, and had no 256 MiB limit.
    parity_on(
        &["native", "wasm-wasi"],
        r#">>> taida-lang/crypto => @(randomBytes)

take n: Int =
  |== error: Error =
    stdout(error.type + ": " + error.message)
    -1
  => :Int
  randomBytes(n).length()
=> :Int

stdout(take(-1))
stdout(take(268435457))
stdout(take(3))
"#,
        "CryptoError: randomBytes: count must be non-negative\n-1\nCryptoError: randomBytes: count exceeds 256 MiB limit\n-1\n3",
    );
}

// ── Int values that equal addresses ──────────────────────────────────

#[test]
fn int_sets_keep_values_that_equal_code_addresses() {
    // A non-PIE native executable is mapped at 0x400000, and its machine
    // code holds the runtime's magic words as instruction immediates.
    // These Ints cover every 8-aligned address of that range; reading one
    // of them as an object crashed `setOf`.
    parity(
        r#"xs <= Map[range(0, 65536), _ i = 4194304 + i * 8]()
s <= setOf(xs)
stdout(s.size())
stdout(Unique[xs]().length())
evens <= setOf(Filter[xs, _ x = Mod[x, 16]().getOrDefault(1) == 0]())
stdout(s.union(evens).size())
stdout(s.intersect(evens).size())
stdout(s.diff(evens).size())
"#,
        "65536\n65536\n65536\n32768\n32768",
    );
}

#[test]
fn int_sets_keep_values_that_equal_heap_addresses() {
    // WASM linear memory starts its heap after a 256 KiB stack, so these
    // Ints point at the 2000 equal lists made first. Ints are compared as
    // numbers: two of them are never equal because the lists they happen
    // to point at are.
    parity(
        r#"copies <= Map[range(0, 2000), _ i = @[1, 2]]()
xs <= Map[range(0, 65536), _ i = 262144 + i * 8]()
stdout(copies.length())
stdout(setOf(xs).size())
stdout(Unique[xs]().length())
"#,
        "2000\n65536\n65536",
    );
}

#[test]
#[cfg(target_os = "linux")]
fn hashmap_churn_stays_in_bounds_under_asan() {
    let out = native_harness(
        r#"
int main(void) {
    taida_val m = taida_hashmap_new();
    const char *keep = "keep";
    m = taida_hashmap_set(m, taida_str_hash((taida_val)keep), (taida_val)keep, 1);
    for (int i = 0; i < 500; i++) {
        char *k = taida_str_alloc(8);
        snprintf(k, 9, "k%07d", i);
        taida_val h = taida_str_hash((taida_val)k);
        m = taida_hashmap_set(m, h, (taida_val)k, i);
        m = taida_hashmap_remove(m, h, (taida_val)k);
    }
    if (taida_hashmap_length(m) != 1) return 2;
    if (taida_hashmap_get(m, taida_str_hash((taida_val)keep), (taida_val)keep) != 1) return 3;
    return 0;
}
"#,
        true,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ── JSON reader ──────────────────────────────────────────────────────

#[test]
fn json_reader_accepts_and_rejects_like_the_interpreter() {
    let cases = [
        r#"{"a":1,"s":"x"}"#,
        r#"{"a":1,"s":"x",}"#,
        r#"{"a":1,,"s":"x"}"#,
        r#"{"a":1 "s":"x"}"#,
        r#"{"a" 1}"#,
        r#"{"a":}"#,
        r#"{a:1}"#,
        r#"{"a":1"#,
        r#"{"a":01}"#,
        r#"{"a":0x10}"#,
        r#"{"a":+1}"#,
        r#"{"a":.5}"#,
        r#"{"a":1.}"#,
        r#"{"a":1e}"#,
        r#"{"a":1,"s":"abc"#,
        r#"{"a":1,"s":"abc\"#,
        r#"{"s":"\x"}"#,
        r#"{"s":"\u12"}"#,
        r#"{"s":"\ud83d"}"#,
        r#"{"s":"\ude00"}"#,
        r#"{"a":1} x"#,
        r#"{"a":1}}"#,
        r#"  {"a" : 7 , "s" : "y" }  "#,
        r#"[1]"#,
        "",
        r#"{"a":1,"a":2,"s":"p","s":"q"}"#,
        r#"{"a":null,"s":null}"#,
        r#"{"a":"1","s":5}"#,
        r#"{"a":1.0}"#,
        r#"{"a":1e2}"#,
        r#"{"a":-0}"#,
        r#"{"a":9223372036854775807}"#,
        r#"{"a":-9223372036854775808}"#,
        r#"{"a":9223372036854775808}"#,
        r#"{"a":1e400}"#,
    ];
    let mut source = String::from("S = @(a: Int, s: Str)\n");
    let mut expected = Vec::new();
    for (i, raw) in cases.iter().enumerate() {
        let lit = raw.replace('\\', "\\\\").replace('\'', "\\'");
        source.push_str(&format!(
            "r{i} <= JSON['{lit}', S]()\nr{i} >=> v{i}\nstdout(\"{i} \" + r{i}.hasValue().toString() + \" \" + v{i}.a.toString() + \" [\" + v{i}.s + \"]\")\n"
        ));
        expected.push(i);
    }
    let dir = unique_temp_dir("f65_nrt_json_cases");
    let td = dir.join("main.td");
    std::fs::write(&td, &source).unwrap();
    let reference = run_interpreter(&td).expect("interpreter run");
    std::fs::remove_dir_all(dir).unwrap();
    // The interpreter (serde_json) is the reference; pin a few rows so the
    // reference itself cannot drift unnoticed.
    let lines: Vec<&str> = reference.lines().collect();
    assert_eq!(lines.len(), expected.len());
    assert_eq!(lines[0], "0 true 1 [x]");
    assert_eq!(lines[1], "1 false 0 []");
    assert_eq!(lines[15], "15 false 0 []");
    assert_eq!(lines[22], "22 true 7 [y]");
    assert_eq!(lines[25], "25 true 2 [q]");
    assert_eq!(lines[27], "27 false 0 []");
    assert_eq!(lines[31], "31 true 9223372036854775807 []");
    assert_eq!(lines[33], "33 false 0 []");
    parity(&source, &reference);
}

#[test]
fn json_reader_decodes_unicode_escapes() {
    parity(
        r#"S = @(s: Str)
JSON['{"s":"\\u0041\\u3042"}', S]() >=> a
stdout(a.s)
stdout(a.s.length())
JSON['{"s":"caf\\u00e9 \\ud83d\\ude00"}', S]() >=> b
stdout(b.s)
stdout(b.s.length())
JSON['{"s":"tab\\there\\n\\"q\\"\\\\\\/"}', S]() >=> c
stdout(c.s)
JSON['{"s":"é😀"}', S]() >=> d
stdout(d.s.length())
"#,
        "Aあ\n2\ncafé 😀\n6\ntab\there\n\"q\"\\/\n2",
    );
}

#[test]
fn json_reader_keeps_every_int_and_the_interpreters_floats() {
    parity(
        r#"S = @(id: Int)
F = @(f: Float)
show raw: Str =
  r <= JSON['{"id":' + raw + '}', S]()
  r >=> v
  r.hasValue().toString() + " " + v.id.toString()
=> :Str
stdout(show("1234567890123456789"))
stdout(show("9007199254740993"))
stdout(show("-9223372036854775808"))
stdout(show("9223372036854775807"))
stdout(show("9223372036854775808"))
stdout(show("1e3"))
stdout(JSON['0.1', Float]().getOrDefault(1.0) == 0.1)
stdout(JSON['1.7976931348623157e308', Float]().getOrDefault(1.0) == 1.7976931348623157e308)
stdout(JSON['123.456e-2', Float]().getOrDefault(1.0) == 1.23456)
f4 <= JSON['{"f":9223372036854775808}', F]()
stdout(f4.hasValue())
"#,
        "true 1234567890123456789\ntrue 9007199254740993\ntrue -9223372036854775808\ntrue 9223372036854775807\nfalse 0\nfalse 0\ntrue\ntrue\ntrue\ntrue",
    );
}

#[test]
fn json_reader_fills_defaults_with_real_strings() {
    // Missing, null and wrong-kind Str slots are "" (never a raw pointer
    // printed as a number); a wrong-kind element keeps the other elements.
    parity(
        r#"S = @(o: @(k: Str, n: Int))
JSON['{"o":{"n":5}}', S]() >=> w
stdout("k=[" + w.o.k + "] n=" + w.o.n.toString())
L = @(xs: @[Str])
r <= JSON['{"xs":["a",null,"b",5,""]}', L]()
stdout(r.hasValue())
r >=> v
stdout(v.xs.length())
stdout(Join[v.xs, "|"]())
"#,
        "k=[] n=5\nfalse\n5\na||b||",
    );
}

#[test]
fn json_reader_matches_enum_variants() {
    // Top-level Enum schemas (Enum fields are covered on native only: the
    // WASM backends do not build types with Enum fields yet).
    parity(
        r#"Enum => Status = :Active :Inactive
stdout(JSON['"Inactive"', Status]().hasValue())
stdout(JSON['"Nope"', Status]().hasValue())
stdout(JSON['5', Status]().hasValue())
stdout(JSON['["Active","Inactive"]', @[Status]]().hasValue())
stdout(JSON['["Active","x"]', @[Status]]().hasValue())
"#,
        "true\nfalse\nfalse\ntrue\nfalse",
    );
    parity_on(
        &["native"],
        r#"Enum => Status = :Active :Inactive
U = @(name: Str, status: Status)
stdout(JSON['{"name":"a","status":"Active"}', U]().hasValue())
stdout(JSON['{"name":"a","status":"Nope"}', U]().hasValue())
stdout(JSON['{"name":"a"}', U]().hasValue())
"#,
        "true\nfalse\ntrue",
    );
}

#[test]
fn json_reader_handles_self_referential_types() {
    parity(
        r#"Tree = @(value: Int, children: @[Tree])
Node = @(value: Int, next: Node)
raw <= '{"value":1,"children":[{"value":2,"children":[{"value":3,"children":[]}]},{"value":4}]}'
JSON[raw, Tree]() >=> t
stdout(t.value)
stdout(t.children.length())
t.children.get(0) >=> c0
c0.children.get(0) >=> c00
stdout(c00.value)
t.children.get(1) >=> c1
stdout(c1.children.length())
r <= JSON['{"value":1,"next":{"value":2}}', Node]()
stdout(r.hasValue())
r >=> n
stdout(n.next.value)
stdout(n.next.next.value)
bad <= JSON['{"value":1,"next":7}', Node]()
stdout(bad.hasValue())
"#,
        "1\n2\n3\n0\ntrue\n2\n0\nfalse",
    );
}

#[test]
fn json_schema_types_keep_their_methods() {
    parity(
        r#"greeting <= "Hi"
Pilot = @(
  name: Str
  age: Int
  intro =
    `${greeting}, I'm ${name}, age ${age.toString()}.`
  => :Str
  older n: Int =
    age + n
  => :Int
)
Crew = @(lead: Pilot, members: @[Pilot])
JSON['{"name":"Rei","age":14}', Pilot]() >=> p
stdout(p.intro())
stdout(p.older(3))
JSON['{"lead":{"name":"Asuka","age":14},"members":[{"name":"Shinji"}]}', Crew]() >=> c
stdout(c.lead.intro())
c.members.get(0) >=> m
stdout(m.intro())
JSON['broken', Pilot]() >=> d
stdout(d.intro())
load raw: Str =
  JSON[raw, Pilot]() >=> q
  q.intro()
=> :Str
stdout(load('{"name":"Mari","age":15}'))
"#,
        "Hi, I'm Rei, age 14.\n17\nHi, I'm Asuka, age 14.\nHi, I'm Shinji, age 0.\nHi, I'm , age 0.\nHi, I'm Mari, age 15.",
    );
}

#[test]
#[cfg(target_os = "linux")]
fn json_reader_stays_inside_the_input_under_asan() {
    // Each document is a Str allocated at exactly its length (header +
    // bytes + NUL), so any read or write past its end trips ASan.
    let out = native_harness(
        r#"
static taida_val exact_str(const char *text) {
    size_t n = strlen(text);
    taida_val *hdr = (taida_val*)malloc(16 + n + 1);
    hdr[0] = TAIDA_STR_MAGIC | 1;
    hdr[1] = (taida_val)n;
    memcpy((char*)(hdr + 2), text, n + 1);
    return (taida_val)(hdr + 2);
}
int main(void) {
    static const char *bad[] = {
        "{\"a\":\"abc\\", "{\"\\", "\"\\", "\"\\u12", "\"\\ud83d\\u", "[1,2", "{\"a\":1,",
        "{\"a\":", "\"abc", "-", "1e", "1.", "[", "{", "tru", "nul", "\"\\ud83d",
        "{\"a\":\"\\u0000\\", "[\"\xe3\x81\"]", "[\"\xff\"]",
    };
    for (size_t i = 0; i < sizeof(bad) / sizeof(bad[0]); i++) {
        taida_val lax = taida_json_schema_cast(exact_str(bad[i]), (taida_val)"T{S|a:s}");
        if (taida_lax_has_value(lax)) return 10 + (int)i;
    }
    static const char *good[] = {
        "{\"a\":\"x\\u00e9\\ud83d\\ude00\"}", "{\"a\":\"\\\\\"}", " {\"a\" : \"\" } ",
    };
    for (size_t i = 0; i < sizeof(good) / sizeof(good[0]); i++) {
        taida_val lax = taida_json_schema_cast(exact_str(good[i]), (taida_val)"T{S|a:s}");
        if (!taida_lax_has_value(lax)) return 40 + (int)i;
    }
    return 0;
}
"#,
        true,
    );
    assert!(
        out.status.success(),
        "exit {:?}: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn large_json_int_list_sums_on_native() {
    // A large JSON document makes an mmap'd allocation; the heap-range
    // fast path used to cover the gap up to it, so the Int sum read as a
    // pointer and faulted.
    parity_on(
        &["native"],
        r#"raw <= jsonEncode(@(xs <= range(0, 60000)))
S = @(xs: @[Int])
JSON[raw, S]() >=> v
stdout(v.xs.length())
Sum[v.xs]() >=> t
stdout(t)
xs <= range(0, 20000)
s <= Join[xs, ","]()
stdout(s.length())
n <= 4000000000
stdout(n)
"#,
        "60000\n1799970000\n108889\n4000000000",
    );
}

#[test]
fn failed_get_of_a_pack_element_does_not_crash_native() {
    // The element default itself is decided elsewhere; reading a field of
    // it must not dereference the null page.
    let dir = unique_temp_dir("f65_nrt_get_default");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        "ps <= @[@(a <= 1, b <= \"x\")]\nps.get(99) >=> p\nstdout(\"a=\" + p.a.toString())\n",
    )
    .unwrap();
    let bin = dir.join("native");
    let built = build("native", &td, &bin);
    assert!(built.status.success());
    let out = run_built("native", &bin, &dir);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim_end(), "a=0");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn byte_molds_read_bytes_values() {
    // `ByteLength` / `ByteAt` / `ByteSlice` take a Bytes value as well as a
    // Str, on every backend; a slice that cuts a UTF-8 sequence reads it as
    // U+FFFD.
    parity(
        r#"Bytes["hello world"]() >=> b
stdout(ByteLength[b]())
ByteAt[b, 0]() >=> x
stdout(x)
stdout(ByteSlice[b, 0, 5]())
ByteAt[b, 99]() >=> y
stdout(y)
cut <= ByteSlice["\u{3042}\u{3044}", 0, 2]()
stdout(ByteLength[cut]())
"#,
        "11\n104\nhello\n0\n3",
    );
}

// ── Numbers and strings ──────────────────────────────────────────────

#[test]
fn smallest_int_divided_by_minus_one_is_a_failed_lax() {
    parity(
        r#"lo <= 0 - 9223372036854775807
mn <= lo - 1
d <= 0 - 1
Div[mn, -1]() >=> q
stdout(q)
Mod[mn, -1]() >=> r
stdout(r)
stdout(Div[mn, d]().hasValue())
stdout(Mod[mn, d]().hasValue())
Div[mn, 2]() >=> h
stdout(h)
"#,
        "0\n0\nfalse\nfalse\n-4611686018427387904",
    );
}

#[test]
fn to_fixed_prints_the_exact_value() {
    parity(
        r#"stdout(ToFixed[1e300, 2]())
stdout(ToFixed[1.5, 100]())
stdout(ToFixed[2.5, -3]())
stdout(ToFixed[0.125, 2]())
stdout(ToFixed[0.1, 20]())
stdout(ToFixed[9.995, 2]())
stdout(ToFixed[9.9999, 2]())
stdout(ToFixed[0.0 - 0.0001, 2]())
stdout(ToFixed[Sqrt[-1.0](), 2]())
"#,
        "1000000000000000052504760255204420248704468581108159154915854115511802457988908195786371375080447864043704443832883878176942523235360430575644792184786706982848387200926575803737830233794788090059368953234970799945081119038967640880074652742780142494579258788820056842838115669472196386865459400540160.00\n1.50000000000000000000\n2\n0.12\n0.10000000000000000555\n9.99\n10.00\n-0.00\nNaN",
    );
}

#[test]
fn float_to_int_saturates_like_the_interpreter() {
    parity(
        r#"Floor[1e300]() >=> a
stdout(a)
Truncate[9.3e18]() >=> b
stdout(b)
Floor[Sqrt[-1.0]()]() >=> c
stdout(c)
Ceil[0.0 - 9.3e18]() >=> d
stdout(d)
Round[0.49999999999999994]() >=> e
stdout(e)
Round[0.0 - 2.5]() >=> f
stdout(f)
Int[1e300]() >=> g
stdout(g)
"#,
        "9223372036854775807\n9223372036854775807\n0\n-9223372036854775808\n0\n-3\n9223372036854775807",
    );
}

#[test]
fn json_encode_writes_floats_like_the_interpreter() {
    // Native only: the WASM jsonEncode Float form is fixed separately.
    parity_on(
        &["native"],
        r#"n <= Sqrt[-1.0]()
inf <= 1.0e308 * 10.0
stdout(jsonEncode(@(a <= 1.0e21, b <= 0.00000015, c <= 100.0, d <= 0.00015, e <= 1.0e16, f <= 1.0e15, g <= 5.0e-324, n <= n, p <= inf)))
stdout(jsonEncode(@[1.5, 0.000001, 3.0e100]))
"#,
        r#"{"a":1e+21,"b":1.5e-7,"c":100.0,"d":0.00015,"e":1e+16,"f":1000000000000000.0,"g":5e-324,"n":null,"p":null}
[1.5,1e-6,3e+100]"#,
    );
}

#[test]
fn sort_is_stable_and_not_quadratic() {
    parity_on(
        &["native"],
        r#"P = @(n: Int, s: Str)
ps <= @[P(n <= 3, s <= "c"), P(n <= 1, s <= "a"), P(n <= 3, s <= "b"), P(n <= 2, s <= "d")]
stdout(Map[Sort[ps](by <= _ p: P = p.n), _ p: P = p.s]())
stdout(Sort[@["pear", "apple", "fig", "apple"]]())
stdout(Sort[@[2.5, 0.0 - 1.5, 3.25]](reverse <= true))
"#,
        "@[\"a\", \"d\", \"c\", \"b\"]\n@[\"apple\", \"apple\", \"fig\", \"pear\"]\n@[3.25, 2.5, -1.5]",
    );
    // 60,000 descending elements took minutes with the insertion sort.
    let dir = unique_temp_dir("f65_nrt_sort_speed");
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        "xs <= range(0, 60000)\nSort[xs](reverse <= true) >=> s\nstdout(s.length())\nSort[s]() >=> t\nstdout(t.length())\n",
    )
    .unwrap();
    let bin = dir.join("native");
    assert!(build("native", &td, &bin).status.success());
    let start = Instant::now();
    let out = run_built("native", &bin, &dir);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "60000\n60000\n");
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "sorting took {:?}",
        start.elapsed()
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn trim_strips_unicode_whitespace() {
    parity(
        r#"stdout(Trim["\u{3000}x\u{a0}"]().length())
stdout(Trim["\u{b}x\u{c}"]().length())
stdout(Trim["\u{2003}x"](end <= false).length())
stdout("[" + Trim[" \t\u{85}\u{2028}x y\u{202f}\u{205f}\u{1680}\u{3000}"]() + "]")
stdout("[" + Trim["\u{200b}x\u{200b}"]() + "]")
stdout("[" + Trim["\u{3000}日本\u{3000}"](start <= false) + "]")
"#,
        "1\n1\n1\n[x y]\n[\u{200b}x\u{200b}]\n[\u{3000}日本]",
    );
}

#[test]
fn bytes_cursor_take_with_a_huge_size_is_a_failed_lax() {
    parity_on(
        &["native", "wasm-wasi"],
        r#"Bytes["hello world"]() >=> b
stdout(BytesCursorTake[BytesCursor[b](offset <= 5), 9223372036854775807]().has_value)
stdout(BytesCursorTake[BytesCursor[b](offset <= 5), 6]().has_value)
stdout("after")
"#,
        "false\ntrue\nafter",
    );
}

#[test]
fn read_fails_on_non_utf8_files_and_directories() {
    let dir = unique_temp_dir("f65_nrt_read");
    std::fs::write(dir.join("bad.txt"), [0x61u8, 0x62, 0xff, 0xfe, 0x63, 0x64]).unwrap();
    std::fs::write(dir.join("good.txt"), "héllo").unwrap();
    std::fs::create_dir(dir.join("d")).unwrap();
    let td = dir.join("main.td");
    std::fs::write(
        &td,
        r#"r1 <= Read["bad.txt"]()
stdout(r1.has_value)
stdout(r1.getOrDefault("DEF").length())
r2 <= Read["d"]()
stdout(r2.has_value)
r3 <= Read["good.txt"]()
stdout(r3.getOrDefault("").length())
"#,
    )
    .unwrap();
    let interp = Command::new(taida_bin())
        .current_dir(&dir)
        .arg(&td)
        .output()
        .unwrap();
    let expected = "false\n3\nfalse\n5\n";
    assert_eq!(String::from_utf8_lossy(&interp.stdout), expected);
    for profile in ["native", "wasm-wasi"] {
        if profile != "native" && wasmtime_bin().is_none() {
            continue;
        }
        let bin = dir.join(profile);
        assert!(build(profile, &td, &bin).status.success(), "{profile}");
        let out = run_built(profile, &bin, &dir);
        assert!(out.status.success(), "{profile}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{profile}");
    }
    #[cfg(target_os = "linux")]
    {
        // Files that report a size of 0 still have contents.
        let proc_td = dir.join("proc.td");
        std::fs::write(
            &proc_td,
            "stdout(Read[\"/proc/self/stat\"]().getOrDefault(\"\").length() > 0)\n",
        )
        .unwrap();
        let bin = dir.join("proc_native");
        assert!(build("native", &proc_td, &bin).status.success());
        let out = run_built("native", &bin, &dir);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "true\n");
    }
    std::fs::remove_dir_all(dir).unwrap();
}
