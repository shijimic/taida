/// JS execution parity tests: transpile Taida source to JS, execute with node,
/// and verify the output matches the interpreter (reference implementation).
///
/// These tests validate JS transpiler correctness by comparing actual execution
/// results against the interpreter, rather than checking generated JS strings.
///
/// Taida syntax reminder:
///   - `<=` for variable assignment
///   - `=` for function definition and BuchiPack field initialization inside @()
///   - `=>` for pipeline / return type
///   - `]=>` for unmold
///   - `| cond |> value` for conditions (NOT `| cond = value`)
mod common;

use common::taida_bin;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter to generate unique temp file names across parallel tests.
/// Combined with PID to remain unique under cargo-nextest (each test = separate process).
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_id() -> String {
    let seq = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}_{}", std::process::id(), seq)
}

/// Run Taida source with the interpreter.
fn interp(source: &str) -> Option<String> {
    let id = unique_id();
    let tmp = std::env::temp_dir().join(format!("taida_jstest_interp_{}.td", id));
    std::fs::write(&tmp, source.trim_start()).ok()?;
    let output = Command::new(taida_bin()).arg(&tmp).output().ok()?;
    let _ = std::fs::remove_file(&tmp);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("Interpreter stderr: {}", stderr);
        return None;
    }
    Some(normalize(&String::from_utf8_lossy(&output.stdout)))
}

/// Transpile Taida source to JS and execute with node.
fn js(source: &str) -> Option<String> {
    let id = unique_id();
    let tmp_td = std::env::temp_dir().join(format!("taida_jstest_src_{}.td", id));
    let tmp_js = std::env::temp_dir().join(format!("taida_jstest_out_{}.mjs", id));
    std::fs::write(&tmp_td, source.trim_start()).ok()?;

    let transpile = Command::new(taida_bin())
        .arg("build")
        .arg("--target")
        .arg("js")
        .arg(&tmp_td)
        .arg("-o")
        .arg(&tmp_js)
        .output()
        .ok()?;

    let _ = std::fs::remove_file(&tmp_td);

    if !transpile.status.success() {
        let stderr = String::from_utf8_lossy(&transpile.stderr);
        eprintln!("Transpile stderr: {}", stderr);
        let _ = std::fs::remove_file(&tmp_js);
        return None;
    }

    let run = Command::new("node").arg(&tmp_js).output().ok()?;

    let _ = std::fs::remove_file(&tmp_js);

    if !run.status.success() {
        let stderr = String::from_utf8_lossy(&run.stderr);
        eprintln!("Node stderr: {}", stderr);
        return None;
    }

    Some(normalize(&String::from_utf8_lossy(&run.stdout)))
}

/// Normalize output for comparison: strip trailing whitespace per line and at end.
///
/// LIMITATION (AT-1): This hides trailing-space differences between backends.
/// See tests/parity.rs normalize() for full documentation.
fn normalize(s: &str) -> String {
    s.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Assert that interpreter and JS produce the same output for a given source.
fn assert_parity(label: &str, source: &str) {
    let interp_out =
        interp(source).unwrap_or_else(|| panic!("{}: interpreter execution failed", label));
    let js_out = js(source).unwrap_or_else(|| panic!("{}: JS transpile/execution failed", label));
    assert_eq!(
        interp_out, js_out,
        "{}: output mismatch\n  interpreter: {:?}\n  JS:          {:?}",
        label, interp_out, js_out,
    );
}

// =========================================================================
// 1. Core language features
// =========================================================================

#[test]
fn test_js_exec_hello() {
    if !node_available() {
        return;
    }
    assert_parity("hello", r#"stdout("Hello, Taida Lang!")"#);
}

#[test]
fn test_js_exec_variables() {
    if !node_available() {
        return;
    }
    assert_parity(
        "variables",
        r#"
x <= 42
y <= "hello"
stdout(x.toString())
stdout(y)
"#,
    );
}

#[test]
fn test_js_exec_arithmetic() {
    if !node_available() {
        return;
    }
    assert_parity(
        "arithmetic",
        r#"
a <= 10 + 5
b <= 10 - 3
c <= 4 * 3
stdout(a.toString())
stdout(b.toString())
stdout(c.toString())
"#,
    );
}

#[test]
fn test_js_exec_functions() {
    if !node_available() {
        return;
    }
    assert_parity(
        "functions",
        r#"
add x y = x + y => :Int
result <= add(3, 5)
stdout(result.toString())
"#,
    );
}

#[test]
fn test_js_exec_closures() {
    if !node_available() {
        return;
    }
    assert_parity(
        "closures",
        r#"
makeAdder n = _ x = n + x => :Int
add5 <= makeAdder(5)
stdout(add5(3).toString())
"#,
    );
}

#[test]
fn test_js_exec_conditions() {
    if !node_available() {
        return;
    }
    assert_parity(
        "conditions",
        r#"
x <= 10
result <= (
  | x > 5 |> "big"
  | x > 0 |> "small"
  | _ |> "zero"
)
stdout(result)
"#,
    );
}

#[test]
fn test_js_exec_buchi_pack() {
    if !node_available() {
        return;
    }
    assert_parity(
        "buchi_pack",
        r#"
person <= @(name <= "Alice", age <= 30)
stdout(person.name)
stdout(person.age.toString())
"#,
    );
}

#[test]
fn test_js_exec_template_string() {
    if !node_available() {
        return;
    }
    assert_parity(
        "template_string",
        r#"
name <= "world"
msg <= `hello ${name}!`
stdout(msg)
"#,
    );
}

#[test]
fn test_js_exec_pipeline() {
    if !node_available() {
        return;
    }
    assert_parity(
        "pipeline",
        r#"
double x = x * 2 => :Int
inc x = x + 1 => :Int
5 => double(_) => inc(_) => result
stdout(result.toString())
"#,
    );
}

// =========================================================================
// 2. Mold operations
// =========================================================================

#[test]
fn test_js_exec_string_molds() {
    if !node_available() {
        return;
    }
    assert_parity(
        "string_molds",
        r#"
stdout(Upper["hello"]())
stdout(Lower["WORLD"]())
stdout(Trim["  hi  "]())
stdout(Reverse["abc"]())
stdout(Repeat["ha", 3]())
stdout(CharAt["hello", 1]())
"#,
    );
}

#[test]
fn test_js_exec_custom_mold_solidify_override() {
    if !node_available() {
        return;
    }
    assert_parity(
        "custom_mold_solidify",
        r#"
Mold[T] => PlusOne[T] = @(
  solidify =
    filling + 1
  => :Int
)
stdout(PlusOne[41]().toString())
"#,
    );
}

#[test]
fn test_js_exec_custom_mold_required_positional_binding() {
    if !node_available() {
        return;
    }
    assert_parity(
        "custom_mold_required_positional",
        r#"
Mold[T] => Pair[T, U] = @(
  second: U
  solidify =
    filling + second
  => :Int
)
stdout(Pair[40, 2]().toString())
"#,
    );
}

#[test]
fn test_js_exec_inherited_custom_mold_required_positional_binding() {
    if !node_available() {
        return;
    }
    assert_parity(
        "inherited_custom_mold_required_positional",
        r#"
Mold[T] => PairBase[T] = @()
PairBase[T] => Pair[T, U] = @(
  second: U
  solidify =
    filling + second
  => :Int
)
stdout(Pair[40, 2]().toString())
"#,
    );
}

#[test]
fn test_js_exec_inherited_custom_mold_override_parent_field() {
    if !node_available() {
        return;
    }
    assert_parity(
        "inherited_custom_mold_override_parent_field",
        r#"
Mold[T] => Base[T] = @(
  bonus: Int <= 0
  solidify =
    filling + bonus
  => :Int
)
Base[T] => Child[T] = @(
  bonus: Int
)
stdout(Child[40, 2]().toString())
"#,
    );
}

#[test]
fn test_js_exec_number_molds() {
    if !node_available() {
        return;
    }
    assert_parity(
        "number_molds",
        r#"
stdout(Abs[-5]().toString())
stdout(Floor[3.7]().toString())
stdout(Ceil[3.2]().toString())
stdout(Round[3.5]().toString())
stdout(Clamp[10, 0, 5]().toString())
"#,
    );
}

#[test]
fn test_js_exec_list_molds() {
    if !node_available() {
        return;
    }
    assert_parity(
        "list_molds",
        r#"
items <= @[3, 1, 2]
stdout(Join[Sort[items](), ", "]())
stdout(Sum[items]().toString())
reversed <= Reverse[items]()
stdout(Join[reversed, ", "]())
"#,
    );
}

#[test]
fn test_js_exec_hof_molds() {
    if !node_available() {
        return;
    }
    assert_parity(
        "hof_molds",
        r#"
items <= @[1, 2, 3, 4, 5]
doubled <= Map[items, _ x = x * 2]()
stdout(Join[doubled, ", "]())
evens <= Filter[items, _ x = x > 2]()
stdout(Join[evens, ", "]())
total <= Fold[items, 0, _ acc x = acc + x]()
stdout(total.toString())
"#,
    );
}

// =========================================================================
// 3. Type system
// =========================================================================

#[test]
fn test_js_exec_optional_abolished() {
    // Optional is abolished in v0.8.0 — verify interpreter errors
    // JS backend parity for this error will be handled by parity-driver
    if !node_available() {
        return;
    }
    // Use Lax instead of Optional for the parity test
    assert_parity(
        "lax_replaces_optional",
        r#"
opt <= Lax[42]()
stdout(opt.hasValue().toString())
stdout(opt.toString())
"#,
    );
}

#[test]
fn test_js_exec_result() {
    if !node_available() {
        return;
    }
    assert_parity(
        "result",
        r#"
ok <= Result[42]()
stdout(ok.isSuccess().toString())
stdout(ok.toString())
Error => Fail = @(message: Str)
err <= Result[0](throw <= Fail(message <= "fail"))
stdout(err.isError().toString())
stdout(err.toString())
"#,
    );
}

#[test]
fn test_js_exec_lax() {
    if !node_available() {
        return;
    }
    assert_parity(
        "lax",
        r#"
d <= Div[10, 3]()
d ]=> val
stdout(val.toString())
d2 <= Div[10, 0]()
stdout(d2.hasValue.toString())
d2 ]=> val2
stdout(val2.toString())
"#,
    );
}

#[test]
fn test_js_exec_type_conversion() {
    if !node_available() {
        return;
    }
    assert_parity(
        "type_conversion",
        r#"
Int["123"]() ]=> n
stdout(n.toString())
Int["abc"]() ]=> n2
stdout(n2.toString())
Float["3.14"]() ]=> f
stdout(f.toString())
Bool["true"]() ]=> b
stdout(b.toString())
Str[42]() ]=> s
stdout(s)
"#,
    );
}

#[test]
fn test_js_exec_typedef_implicit_type_default_injection() {
    if !node_available() {
        return;
    }
    assert_parity(
        "typedef_implicit_type_default_injection",
        r#"
Pilot = @(
  name: Str
  age: Int
  callSign: Str
)

pilot <= Pilot(name <= "Rei")
stdout(pilot.name)
stdout(pilot.age.toString())
stdout(pilot.callSign.length().toString())
"#,
    );
}

#[test]
fn test_js_exec_inherit_implicit_type_default_injection() {
    if !node_available() {
        return;
    }
    assert_parity(
        "inherit_implicit_type_default_injection",
        r#"
Pilot = @(
  name: Str
  age: Int
)

Pilot => Officer = @(
  rank: Int
  department: Str
)

officer <= Officer(name <= "Asuka")
stdout(officer.name)
stdout(officer.age.toString())
stdout(officer.rank.toString())
stdout(officer.department)
"#,
    );
}

// =========================================================================
// 4. Collections
// =========================================================================

#[test]
fn test_js_exec_hashmap() {
    if !node_available() {
        return;
    }
    assert_parity(
        "hashmap",
        r#"
m <= hashMap()
m2 <= m.set("a", 1).set("b", 2)
stdout(m2.size().toString())
stdout(m2.has("a").toString())
stdout(m2.has("c").toString())
"#,
    );
}

#[test]
fn test_js_exec_set() {
    if !node_available() {
        return;
    }
    assert_parity(
        "set",
        r#"
s <= setOf(@[1, 2, 3, 2, 1])
stdout(s.size().toString())
stdout(s.has(2).toString())
stdout(s.has(5).toString())
"#,
    );
}

// =========================================================================
// 5. List methods (state checks)
// =========================================================================

#[test]
fn test_js_exec_list_methods() {
    if !node_available() {
        return;
    }
    assert_parity(
        "list_methods",
        r#"
items <= @[3, 1, 4, 1, 5]
stdout(items.length().toString())
stdout(items.isEmpty().toString())
stdout(items.contains(4).toString())
stdout(items.indexOf(1).toString())
items.first() ]=> f
stdout(f.toString())
items.last() ]=> l
stdout(l.toString())
items.max() ]=> mx
stdout(mx.toString())
items.min() ]=> mn
stdout(mn.toString())
"#,
    );
}

#[test]
fn test_js_exec_string_methods() {
    if !node_available() {
        return;
    }
    assert_parity(
        "string_methods",
        r#"
s <= "hello world"
stdout(s.length().toString())
stdout(s.contains("world").toString())
stdout(s.startsWith("hello").toString())
stdout(s.endsWith("world").toString())
stdout(s.indexOf("world").toString())
"#,
    );
}

#[test]
fn test_js_exec_number_methods() {
    if !node_available() {
        return;
    }
    assert_parity(
        "number_methods",
        r#"
x <= 42
stdout(x.isNaN().toString())
stdout(x.isPositive().toString())
stdout(x.isZero().toString())
"#,
    );
}

// =========================================================================
// 6. Error handling
// =========================================================================

#[test]
fn test_js_exec_error_ceiling() {
    if !node_available() {
        return;
    }
    assert_parity(
        "error_ceiling",
        r#"
safeOp =
  |== e: Error =
    "caught"
  => :Str
  "ok"
=> :Str

stdout(safeOp())
"#,
    );
}

// =========================================================================
// 7. Partial application
// =========================================================================

#[test]
fn test_js_exec_partial_application() {
    if !node_available() {
        return;
    }
    assert_parity(
        "partial_application",
        r#"
add x y = x + y => :Int
add5 <= add(5, )
stdout(add5(3).toString())
mul x y = x * 2 + y => :Int
double <= mul(, 0)
stdout(double(7).toString())
"#,
    );
}

// =========================================================================
// 8. Tail recursion
// =========================================================================

#[test]
fn test_js_exec_tail_recursion() {
    if !node_available() {
        return;
    }
    assert_parity(
        "tail_recursion",
        r#"
factorial n acc =
  | n < 2 |> acc
  | _ |> factorial(n - 1, n * acc)
=> :Int
stdout(factorial(10, 1).toString())
"#,
    );
}

// =========================================================================
// 9. JSON encode
// =========================================================================

#[test]
fn test_js_exec_json_encode() {
    if !node_available() {
        return;
    }
    assert_parity(
        "json_encode",
        r#"
data <= @(name <= "Alice", age <= 30)
stdout(jsonEncode(data))
"#,
    );
}

// ── Mutual Recursion TCO ─────────────────────────────────────────────────

#[test]
fn test_js_exec_mutual_recursion_basic() {
    if !node_available() {
        return;
    }
    assert_parity(
        "mutual_recursion_basic",
        r#"
isEven n =
  | n == 0 |> true
  | _ |> isOdd(n - 1)

isOdd n =
  | n == 0 |> false
  | _ |> isEven(n - 1)

stdout(isEven(0))
stdout(isEven(1))
stdout(isEven(4))
stdout(isOdd(3))
"#,
    );
}

#[test]
fn test_js_exec_mutual_recursion_deep() {
    if !node_available() {
        return;
    }
    assert_parity(
        "mutual_recursion_deep",
        r#"
isEven n =
  | n == 0 |> true
  | _ |> isOdd(n - 1)

isOdd n =
  | n == 0 |> false
  | _ |> isEven(n - 1)

stdout(isEven(100000))
"#,
    );
}

// =========================================================================
// FL-12: Template literal backtick escaping regression
// =========================================================================

#[test]
fn test_js_exec_template_lit_backtick_escape() {
    if !node_available() {
        return;
    }
    // Backtick inside a template literal must be escaped in JS output
    assert_parity(
        "template_lit_backtick",
        "msg <= `has backtick: \\``\nstdout(msg)",
    );
}

#[test]
fn test_js_exec_template_lit_backslash_escape() {
    if !node_available() {
        return;
    }
    // Backslash inside a template literal must be escaped in JS output
    assert_parity(
        "template_lit_backslash",
        "msg <= `has backslash: \\\\`\nstdout(msg)",
    );
}

#[test]
fn test_js_exec_template_lit_interpolation_preserved() {
    if !node_available() {
        return;
    }
    // ${...} interpolation must still work after escaping
    assert_parity(
        "template_lit_interpolation",
        "name <= \"Taida\"\nmsg <= `Hello ${name}!`\nstdout(msg)",
    );
}

// =========================================================================
// FL-13: HashMap/Set toString string escaping regression
// =========================================================================

#[test]
fn test_js_exec_hashmap_tostring_basic() {
    if !node_available() {
        return;
    }
    // Basic HashMap toString parity
    assert_parity(
        "hashmap_tostring_basic",
        r#"
h <= hashMap(@[
  @(key <= "a", value <= "b")
])
stdout(h.toString())
"#,
    );
}

#[test]
fn test_js_exec_set_tostring_basic() {
    if !node_available() {
        return;
    }
    // Basic Set toString parity
    assert_parity(
        "set_tostring_basic",
        r#"
s <= setOf(@["hello", "world"])
stdout(s.toString())
"#,
    );
}

// FL-13: HashMap/Set toString with escape characters
// =========================================================================

#[test]
fn test_js_exec_hashmap_tostring_special_chars() {
    if !node_available() {
        return;
    }
    // HashMap toString with special characters in keys/values
    assert_parity(
        "hashmap_tostring_special_chars",
        r#"
h <= hashMap(@[
  @(key <= "greeting", value <= "hello world"),
  @(key <= "symbol", value <= "a+b=c"),
  @(key <= "empty", value <= "")
])
stdout(h.toString())
"#,
    );
}

#[test]
fn test_js_exec_hashmap_tostring_numeric_values() {
    if !node_available() {
        return;
    }
    // HashMap toString with numeric values
    assert_parity(
        "hashmap_tostring_numeric_values",
        r#"
h <= hashMap(@[
  @(key <= "x", value <= 42),
  @(key <= "y", value <= 0)
])
stdout(h.toString())
"#,
    );
}

#[test]
fn test_js_exec_set_tostring_special_chars() {
    if !node_available() {
        return;
    }
    // Set toString with various string values
    assert_parity(
        "set_tostring_special_chars",
        r#"
s <= setOf(@["alpha", "beta", "gamma"])
stdout(s.toString())
"#,
    );
}

// =========================================================================
// FL-14: Div/Mod Number.isInteger regression
// =========================================================================

#[test]
fn test_js_exec_div_integer() {
    if !node_available() {
        return;
    }
    // Integer division should use truncation
    assert_parity(
        "div_integer",
        r#"
d <= Div[7, 2]()
d ]=> v
stdout(v.toString())
"#,
    );
}

#[test]
fn test_js_exec_div_by_zero() {
    if !node_available() {
        return;
    }
    // Division by zero returns Lax default
    assert_parity(
        "div_by_zero",
        r#"
d <= Div[10, 0]()
d ]=> v
stdout(v.toString())
"#,
    );
}

#[test]
fn test_js_exec_div_float() {
    if !node_available() {
        return;
    }
    // Float division with non-integer result
    assert_parity(
        "div_float",
        r#"
d <= Div[7.5, 2.0]()
d ]=> v
stdout(v.toString())
"#,
    );
}

#[test]
fn test_js_exec_div_mixed_int_float() {
    if !node_available() {
        return;
    }
    // Mixed integer and float division with non-integer result
    assert_parity(
        "div_mixed_int_float",
        r#"
d <= Div[7, 2.0]()
d ]=> v
stdout(v.toString())
"#,
    );
}

#[test]
fn test_js_exec_mod_integer() {
    if !node_available() {
        return;
    }
    // Integer modulo
    assert_parity(
        "mod_integer",
        r#"
m <= Mod[7, 3]()
m ]=> v
stdout(v.toString())
"#,
    );
}
