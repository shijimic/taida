//! Native reference counting keeps values that are still in use alive, and
//! a tail-recursive loop does not keep its earlier iterations' values.
//!
//! Every program runs on the interpreter (the reference), native, and the
//! WASM profiles when wasmtime is available; all must print the same
//! output. The memory tests measure native peak RSS with `/usr/bin/time`.
#![cfg(feature = "native")]
mod common;

use common::{run_interpreter, taida_bin, unique_temp_dir, wasmtime_bin};
use std::path::Path;
use std::process::{Command, Output};

fn build(profile: &str, td: &Path, bin: &Path) -> Output {
    Command::new(taida_bin())
        .args(["build", profile])
        .arg(td)
        .arg("-o")
        .arg(bin)
        .output()
        .unwrap()
}

fn parity(source: &str, expected: &str) {
    let dir = unique_temp_dir("f65_life");
    let td = dir.join("main.td");
    std::fs::write(&td, source).unwrap();
    assert_eq!(
        run_interpreter(&td).unwrap().trim_end(),
        expected,
        "interpreter"
    );
    for profile in ["native", "wasm-min", "wasm-wasi"] {
        if profile != "native" && wasmtime_bin().is_none() {
            continue;
        }
        let bin = dir.join(profile);
        let output = build(profile, &td, &bin);
        assert!(
            output.status.success(),
            "{profile}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = if profile == "native" {
            Command::new(&bin).output()
        } else {
            Command::new(wasmtime_bin().unwrap()).arg(&bin).output()
        }
        .unwrap();
        assert!(
            output.status.success(),
            "{profile}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim_end(),
            expected,
            "{profile}"
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// Build `source` natively, run it under `/usr/bin/time`, and return its
/// stdout and peak RSS in KiB (`None` when `/usr/bin/time` is missing).
#[cfg(target_os = "linux")]
fn native_peak_rss(source: &str) -> Option<(String, u64)> {
    if !Path::new("/usr/bin/time").exists() {
        return None;
    }
    let dir = unique_temp_dir("f65_life_rss");
    let td = dir.join("main.td");
    std::fs::write(&td, source).unwrap();
    let bin = dir.join("main");
    let built = build("native", &td, &bin);
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let rss = dir.join("rss");
    let out = Command::new("/usr/bin/time")
        .args(["-f", "%M", "-o"])
        .arg(&rss)
        .arg(&bin)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let peak: u64 = std::fs::read_to_string(&rss)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    std::fs::remove_dir_all(dir).unwrap();
    Some((stdout, peak))
}

// ── Borrowed values are not released ────────────────────────────────

#[test]
fn unused_alias_keeps_the_original_value() {
    parity(
        r#"xs <= @[1, 2, 3]
unused <= xs
ys <= @[4, 5, 6]
greet <= "hello-" + "world"
copy <= greet
other <= "zzzzz-" + "yyyyy"
stdout(xs)
stdout(ys)
stdout(greet)
stdout(other)
"#,
        "@[1, 2, 3]\n@[4, 5, 6]\nhello-world\nzzzzz-yyyyy",
    );
}

#[test]
fn unused_alias_of_a_parameter_keeps_the_callers_value() {
    parity(
        r#"f xs: @[Int] =
  tmp <= xs
  xs.length()
=> :Int

id xs: @[Int] =
  xs
=> :@[Int]

data <= @[1, 2, 3]
stdout(f(data).toString())
unused <= id(data)
more <= @[7, 8, 9]
stdout(data)
stdout(more)
"#,
        "3\n@[1, 2, 3]\n@[7, 8, 9]",
    );
}

#[test]
fn unused_field_reads_keep_the_pack_intact() {
    parity(
        r#"p <= @(name <= "Rei" + "-san", tags <= @["a", "b"])
unusedName <= p.name
unusedTags <= p.tags
other <= "Asuka" + "-san"
junk <= @["x", "y"]
stdout(p.name)
stdout(p.tags)
stdout(other)
stdout(junk)
"#,
        "Rei-san\n@[\"a\", \"b\"]\nAsuka-san\n@[\"x\", \"y\"]",
    );
}

#[test]
fn error_ceiling_body_does_not_release_captured_values() {
    parity(
        r#"Error => Boom = @(code: Int)

safe xs: @[Int] n: Int =
  |== error: Error =
    0 - 1
  => :Int
  n
=> :Int

risky n: Int =
  | n > 2 |> Boom(type <= "Boom", message <= "too big", code <= n).throw()
  | _ |> n
=> :Int

total xs: @[Int] limit: Int =
  count <= xs.length()
  |== error: Error =
    0 - 1
  => :Int
  risky(count + limit)
=> :Int

data <= @[10, 20, 30]
r1 <= safe(data, 1)
r2 <= total(data, 0 - 2)
more <= @[40, 50, 60]
stdout(r1.toString() + " " + r2.toString())
stdout(data)
stdout(more)
"#,
        "1 1\n@[10, 20, 30]\n@[40, 50, 60]",
    );
}

#[test]
fn type_method_does_not_release_fields_it_does_not_use() {
    parity(
        r#"Pilot = @(
  name: Str
  age: Int
  isAdult =
    age >= 18
  => :Bool
)

Bag = @(
  items: @[Int]
  label: Str
  count =
    items.length()
  => :Int
)

p <= Pilot(name <= "Rei" + "-san", age <= 14)
a <= p.isAdult()
b <= p.isAdult()
bag <= Bag(items <= @[1, 2, 3], label <= "b" + "ag")
c1 <= bag.count()
c2 <= bag.count()
c3 <= bag.count()
other <= "Asuka" + "-san"
stdout(p.name)
stdout(other)
stdout(a.toString() + b.toString())
stdout(bag.label)
stdout(bag.items)
stdout(c1 + c2 + c3)
"#,
        "Rei-san\nAsuka-san\nfalsefalse\nbag\n@[1, 2, 3]\n9",
    );
}

#[test]
fn returned_closures_keep_their_captured_values() {
    parity(
        r#"makeGetter n: Int =
  xs <= @[n, n + 1, n + 2]
  _ i = xs.get(i).unmold()
=> :Int => :Int

makeCounter start: Int =
  steps <= @[start, start + 1, start + 2]
  at i: Int =
    steps.get(i).unmold()
  => :Int
  at
=> :Int => :Int

makePack n: Int =
  xs <= @[n, n + 1, n + 2]
  @(get <= _ i: Int = xs.get(i).unmold())
=> :@(get: Int => :Int)

wrap f: Int => :Int =
  f
=> :Int => :Int

makeWrapped n: Int =
  xs <= @[n, n + 1, n + 2]
  g <= _ i: Int = xs.get(i).unmold()
  wrap(g)
=> :Int => :Int

g <= makeGetter(10)
f <= makeCounter(20)
p <= makePack(30)
h <= makeWrapped(40)
junk <= @[100, 200, 300]
get <= p.get
stdout(g(1).toString())
stdout(f(1).toString())
stdout(get(1).toString())
stdout(h(2).toString())
stdout(junk)
"#,
        "11\n21\n31\n42\n@[100, 200, 300]",
    );
}

#[test]
fn mapping_an_error_result_keeps_the_callers_result() {
    // `.map` on an error Result hands back the Result it was given; the
    // binding does not own it.
    parity(
        r#"bump r: Result[Int, Error] =
  r2 <= r.map(_ x: Int = x + 1)
  stdout(r2.isError())
  7
=> :Int

failed <= Result[0, _ = false](throw <= Error(type <= "Error", message <= "oo" + "ps"))
e1 <= bump(failed)
e2 <= bump(failed)
junk <= @(a <= "q" + "q", b <= 1, c <= 2, d <= 3)
junk2 <= @(a <= "w" + "w", b <= 3, c <= 4, d <= 5)
stdout(e1 + e2)
stdout(failed.isError())
stdout(failed.throw.message)
"#,
        "true\ntrue\n14\ntrue\noops",
    );
}

#[test]
fn appended_item_survives_its_binding() {
    parity(
        r#"Todo = @(title: Str, done: Bool)

addTodo list: @[Todo] title: Str =
  item <= @(title <= title, done <= false)
  Append[list, item]()
=> :@[Todo]

list1 <= addTodo(@[], "Learn Taida basics")
list2 <= addTodo(list1, "Build a TODO app")
list3 <= addTodo(list2, "Write tests")
junk <= @(title <= "x" + "y", done <= true)
stdout(list3.get(0).unmold().title)
stdout(list3.get(1).unmold().title)
stdout(list3.get(2).unmold().title)
stdout(junk.title)
"#,
        "Learn Taida basics\nBuild a TODO app\nWrite tests\nxy",
    );
}

// ── Values bound inside condition arms ──────────────────────────────

#[test]
fn arm_tail_binding_is_the_arm_result() {
    parity(
        r#"describe n: Int =
  | n > 0 |>
    label <= "pos-" + n.toString()
  | _ |> "neg"
=> :Str

mk n: Int =
  | n > 0 |>
    p <= @(v <= n, s <= "n" + n.toString())
  | _ |> @(v <= 0, s <= "zero")
=> :@(v: Int, s: Str)

a <= describe(5)
b <= describe(7)
c <= mk(3)
d <= mk(4)
stdout(a)
stdout(b)
stdout(c.s)
stdout(d.s)
"#,
        "pos-5\npos-7\nn3\nn4",
    );
}

#[test]
fn heap_bindings_inside_expression_arms_build_and_run() {
    parity(
        r#"report n: Int =
  logged <= (
    | n > 0 |>
      xs <= @[n, n]
      stdout(xs)
    | _ |> 0
  )
  n + 1
=> :Int

mk n: Int =
  | n > 0 |>
    xs <= @[n, n * 2, n * 3]
  | _ |> @[0]
=> :@[Int]

stdout(report(2).toString())
stdout(report(0).toString())
a <= mk(5)
b <= mk(7)
stdout(a)
stdout(b)
n <= 5
msg <= (
  | n > 0 |>
    parts <= @["a", "b"]
    Join[parts, ","]()
  | _ |> "none"
)
stdout(msg)
"#,
        "@[2, 2]\n3\n1\n@[5, 10, 15]\n@[7, 14, 21]\na,b",
    );
}

#[test]
fn error_ceiling_after_an_arm_with_local_bindings_builds() {
    parity(
        r#"Error => Boom = @(code: Int)

risky n: Int =
  | n > 2 |> Boom(type <= "Boom", message <= "too big", code <= n).throw()
  | _ |> n
=> :Int

f x: Int =
  base <= @[x, x + 1]
  y <= (
    | x > 0 |>
      xs <= @[x, x * 2]
      s <= "v" + x.toString()
      xs.length() + s.length()
    | _ |> 0
  )
  |== error: Error =
    0 - 1
  => :Int
  risky(y + base.length())
=> :Int

stdout(f(0).toString())
stdout(f(1).toString())
stdout(f(5).toString())
"#,
        "2\n-1\n-1",
    );
}

// ── Loops ────────────────────────────────────────────────────────────

#[test]
fn loop_results_match_across_backends() {
    parity(
        r#"loop i: Int acc: Int =
  | i == 0 |> acc
  | _ |>
    s <= "item-" + i.toString()
    xs <= @[i, i + 1, i + 2]
    p <= @(name <= s, vals <= xs)
    loop(i - 1, acc + p.name.length() + p.vals.length())
=> :Int

fill m: HashMap[Int, Int] i: Int n: Int =
  | i >= n |> m
  | _ |> fill(m.set(i + 10, i * 2), i + 1, n)
=> :HashMap[Int, Int]

sizes m: HashMap[Int, Int] i: Int acc: @[Int] =
  | i == 0 |> acc
  | _ |> sizes(m.set(i + 100, i), i - 1, Append[acc, m.size()]())
=> :@[Int]

strs i: Int acc: Str =
  | i == 0 |> acc
  | _ |> strs(i - 1, Slice[acc + i.toString()](start <= 1, end <= 6))
=> :Str

base <= hashMap().set(100, 1)
m <= fill(base, 0, 50)
stdout(loop(1000, 0))
stdout(m.size())
stdout(m.get(59).getOrDefault(0))
stdout(base.size())
stdout(sizes(base, 3, @[]))
stdout(strs(12, "abcde"))
"#,
        "10893\n51\n98\n1\n@[1, 2, 3]\n54321",
    );
}

/// A loop may update its own map in place, but never a map somebody else
/// still holds: the caller's map, an older version passed along, or one
/// kept in a pack.
#[test]
fn map_updates_in_loops_leave_other_holders_unchanged() {
    parity(
        r#"keep m: HashMap[Int, Int] i: Int olds: @[Int] =
  | i == 0 |> olds
  | _ |> keep(m.set(i + 10, i), i - 1, Append[olds, m.size()]())
=> :@[Int]

pair a: HashMap[Int, Int] b: HashMap[Int, Int] i: Int =
  | i == 0 |> a.size() * 100 + b.size()
  | _ |> pair(a.set(i + 10, i), a, i - 1)
=> :Int

boxed m: HashMap[Int, Int] i: Int acc: @(prev: HashMap[Int, Int]) =
  | i == 0 |> acc.prev.size() * 100 + m.size()
  | _ |> boxed(m.set(i + 10, i), i - 1, @(prev <= m))
=> :Int

grow m: HashMap[Int, Int] i: Int =
  | i == 0 |> m
  | _ |> grow(m.set(i + 10, i * 3), i - 1)
=> :HashMap[Int, Int]

regrow m: HashMap[Int, Int] i: Int =
  | i == 0 |> m
  | _ |> regrow(m.set(i + 10, 0 - i), i - 1)
=> :HashMap[Int, Int]

base <= hashMap().set(100, 1)
stdout(keep(base, 3, @[]))
stdout(pair(base, base, 4))
stdout(boxed(base, 5, @(prev <= base)))
g <= grow(base, 30)
stdout(g.size())
stdout(g.get(40).getOrDefault(0))
stdout(base.size())
g2 <= regrow(g, 5)
stdout(g.size().toString() + " " + g2.size().toString())
stdout(g.get(15).getOrDefault(0).toString() + " " + g2.get(15).getOrDefault(0).toString())
"#,
        "@[1, 2, 3]\n504\n506\n31\n90\n1\n31 31\n15 -5",
    );
}

#[test]
#[cfg(target_os = "linux")]
fn loop_iterations_do_not_accumulate_their_values() {
    let Some((stdout, peak_kib)) = native_peak_rss(
        r#"loop i: Int acc: Int =
  | i == 0 |> acc
  | _ |>
    s <= "item-" + i.toString()
    xs <= @[i, i + 1, i + 2]
    p <= @(name <= s, vals <= xs)
    loop(i - 1, acc + p.name.length() + p.vals.length())
=> :Int
stdout(loop(1000000, 0))
"#,
    ) else {
        return;
    };
    assert_eq!(stdout, "13888896");
    // Each iteration used to leave a string, a list and a pack behind
    // (about 290 bytes, 115 MB for 400,000 iterations).
    assert!(peak_kib < 32 * 1024, "peak RSS {peak_kib} KiB");
}

#[test]
#[cfg(target_os = "linux")]
fn loop_replacing_its_map_parameter_stays_flat() {
    let Some((stdout, peak_kib)) = native_peak_rss(
        r#"loop i: Int m: HashMap[Str, Int] =
  | i == 0 |> m.size()
  | _ |> loop(i - 1, m.set("k" + Mod[i, 50]().getOrDefault(0).toString(), i))
=> :Int
stdout(loop(400000, hashMap()))
"#,
    ) else {
        return;
    };
    assert_eq!(stdout, "50");
    // Every iteration used to keep the previous map (1.7 GB here).
    assert!(peak_kib < 32 * 1024, "peak RSS {peak_kib} KiB");
}

#[test]
#[cfg(target_os = "linux")]
fn loop_replacing_its_string_parameter_stays_flat() {
    let Some((stdout, peak_kib)) = native_peak_rss(
        r#"loop i: Int acc: Str =
  | i == 0 |> acc.length()
  | _ |> loop(i - 1, Slice[acc + "x"](start <= 1, end <= 101))
=> :Int
stdout(loop(400000, Repeat["a", 100]()))
"#,
    ) else {
        return;
    };
    assert_eq!(stdout, "100");
    assert!(peak_kib < 32 * 1024, "peak RSS {peak_kib} KiB");
}

#[test]
#[cfg(target_os = "linux")]
fn building_a_large_map_by_recursion_is_linear() {
    let started = std::time::Instant::now();
    let Some((stdout, peak_kib)) = native_peak_rss(
        r#"fill m: HashMap[Int, Int] i: Int n: Int =
  | i >= n |> m
  | _ |> fill(m.set(i, i), i + 1, n)
=> :HashMap[Int, Int]

m <= fill(hashMap(), 0, 100000)
stdout(m.size())
stdout(m.get(99999).getOrDefault(0))
"#,
    ) else {
        return;
    };
    assert_eq!(stdout, "100000\n99999");
    // Cloning the whole map on every `.set` made this O(N^2): 20,000
    // entries took 12 GB and 100,000 were killed for running out of memory.
    assert!(peak_kib < 128 * 1024, "peak RSS {peak_kib} KiB");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(60),
        "took {:?}",
        started.elapsed()
    );
}
