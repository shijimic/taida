//! Modules on the compiled backends: importing a module runs its top level
//! once (whatever the import graph), its top-level values may refer to each
//! other, re-exports reach the defining module, imports under an alias keep
//! their own slot, and a call omitting arguments gets the defaults of the
//! module that defines the function. The field defaults and methods of an
//! imported type read the names of the module that defines it. A package
//! that is not installed is named the same way by every backend. Fields of
//! the same name in two modules' types keep their own kinds. A versioned
//! import outside packages.tdm stops every build as it stops the
//! interpreter. An imported module runs where its import stands.
#![cfg(feature = "native")]
mod common;
mod f65_support;
use f65_support::{ALL_COMPILED, parity_files};

#[test]
fn imported_module_top_level_runs_once() {
    parity_files(
        "f65_module_init_once",
        &[
            (
                "c.td",
                r#"announce cmsg: Str =
  stdout("c: init " + cmsg)
  100
=> :Int

base <= announce("base")

cval cx: Int =
  cx + base
=> :Int

stdout("c: top level")

<<< @(cval, base)
"#,
            ),
            (
                "a.td",
                r#">>> ./c.td => @(cval)

aval ax: Int =
  cval(ax) + 1
=> :Int

<<< @(aval)
"#,
            ),
            (
                "b.td",
                r#">>> ./c.td => @(cval)

bval bx: Int =
  cval(bx) + 2
=> :Int

<<< @(bval)
"#,
            ),
            (
                "main.td",
                r#">>> ./a.td => @(aval)
>>> ./b.td => @(bval)
>>> ./c.td => @(base)

stdout("main: start")
stdout(aval(1))
stdout(bval(1))
stdout(base)
"#,
            ),
        ],
        "c: init base\nc: top level\nmain: start\n102\n103\n100",
        ALL_COMPILED,
    );
}

#[test]
fn module_top_level_values_refer_to_each_other() {
    parity_files(
        "f65_module_values",
        &[
            (
                "lib.td",
                r#"base <= 10
derived <= base + 1
step <= 10
bump <= _ v: Int = v + step
<<< @(base, derived, bump)
"#,
            ),
            (
                "main.td",
                r#">>> ./lib.td => @(base, derived, bump)

stdout(base)
stdout(derived)
stdout(bump(1))
"#,
            ),
        ],
        "10\n11\n11",
        ALL_COMPILED,
    );
}

#[test]
fn aliased_imports_keep_their_own_values() {
    parity_files(
        "f65_module_alias_slots",
        &[
            ("a.td", "rate <= 10\n<<< @(rate)\n"),
            ("b.td", "rate <= 3\n<<< @(rate)\n"),
            (
                "main.td",
                r#">>> ./a.td => @(rate)
>>> ./b.td => @(rate => rateB)

show n: Int =
  stdout("in func: " + rate.toString() + " " + rateB.toString())
  n
=> :Int

stdout("top: " + rate.toString() + " " + rateB.toString())
show(0)
"#,
            ),
        ],
        "top: 10 3\nin func: 10 3",
        ALL_COMPILED,
    );
}

#[test]
fn re_exported_functions_and_enums_resolve() {
    parity_files(
        "f65_module_reexport",
        &[
            (
                "a.td",
                r#"Enum => Dir = :Up :Down

funcA fa: Int =
  fa + 1
=> :Int

<<< @(funcA, Dir)
"#,
            ),
            (
                "b.td",
                r#">>> ./a.td => @(funcA, Dir)

funcB fb: Int =
  funcA(fb) * 2
=> :Int

<<< @(funcA, funcB, Dir)
"#,
            ),
            (
                "main.td",
                r#">>> ./b.td => @(funcA, funcB, Dir)

stdout(funcA(1))
stdout(funcB(1))
d <= Dir:Down()
stdout(d == Dir:Down())
"#,
            ),
        ],
        "2\n4\ntrue",
        ALL_COMPILED,
    );
}

#[test]
fn omitted_arguments_take_the_defining_module_defaults() {
    parity_files(
        "f65_module_defaults",
        &[
            (
                "lib.td",
                r#"offset <= 100

add3 a: Int b: Int <= 10 c: Int <= a + offset =
  a + b + c
=> :Int

<<< @(add3)
"#,
            ),
            (
                "main.td",
                r#">>> ./lib.td => @(add3)

offset <= 5000

base <= 100

f a: Int b: Int <= base + a =
  a + b
=> :Int

g x: Int =
  base <= 5000
  f(x) + base
=> :Int

outer n: Int =
  k <= 100
  helper a: Int b: Int <= 7 =
    a + b + k
  => :Int
  helper(n)
=> :Int

stdout(add3(1))
stdout(add3(1, 2))
stdout(add3(1, 2, 3))
stdout(f(1))
stdout(g(1))
stdout(outer(1))
"#,
            ),
        ],
        "112\n104\n6\n102\n5102\n108",
        ALL_COMPILED,
    );
}

#[test]
fn imported_types_use_their_module_names() {
    parity_files(
        "f65_module_type_scope",
        &[
            (
                "lib.td",
                r#"rate <= 10
basePort <= 8000
helper n: Int =
  n + 1
=> :Int
Calc = @(
  x: Int
  f =
    helper(x * rate)
  => :Int
)
Config = @(
  name: Str
  port: Int <= basePort + 80
  describe =
    name + ":" + port.toString()
  => :Str
)
<<< @(Calc, Config)
"#,
            ),
            (
                "main.td",
                r#">>> ./lib.td => @(Calc, Config)
rate <= 1000
basePort <= 1
helper n: Int =
  n + 5000
=> :Int
stdout(Calc(x <= 2).f())
stdout(helper(0))
stdout(Config(name <= "api").describe())
stdout(Config(name <= "web", port <= 9).describe())
cs <= @[Config(name <= "a"), Config(name <= "b", port <= 2)]
Map[cs, _ c: Config = c.describe()]() >=> ds
stdout(ds)
"#,
            ),
        ],
        "21\n5000\napi:8080\nweb:9\n@[\"a:8080\", \"b:2\"]",
        ALL_COMPILED,
    );
}

#[test]
fn a_package_that_is_not_installed_is_named_by_every_build() {
    // The compiled backends name the missing package as the interpreter
    // does, rather than a placeholder path.
    let dir = common::unique_temp_dir("f65_missing_package");
    std::fs::write(
        dir.join("main.td"),
        ">>> taida-lang/nonexistent => @(foo)\nstdout(\"after import\")\n",
    )
    .unwrap();
    let expected = "Package 'taida-lang/nonexistent' not found. Run 'taida ingot deps' to install dependencies.";
    let interp = std::process::Command::new(common::taida_bin())
        .arg(dir.join("main.td"))
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&interp.stderr).contains(expected),
        "interpreter: {}",
        String::from_utf8_lossy(&interp.stderr)
    );
    for target in ["native", "wasm-min"] {
        let out = std::process::Command::new(common::taida_bin())
            .args(["build", target])
            .arg(dir.join("main.td"))
            .arg("-o")
            .arg(dir.join(format!("out-{target}")))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !out.status.success() && stderr.contains(expected) && !stderr.contains("<unresolved"),
            "{target}: {stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn same_named_fields_of_two_modules_keep_their_kinds() {
    // `qty` is an Int in one module's type and a Str in another's, and two
    // modules declare an `Item` whose `v` differs the same way. The
    // compiled backends kept one kind per field name, the last one
    // registered: native crashed writing the other type's field as JSON
    // and WASM wrote it as "".
    parity_files(
        "f65_field_kinds_by_module",
        &[
            (
                "a.td",
                "Item = @(label: Str, qty: Int)\n\nmkA an: Int =\n  Item(label <= \"a-item\", qty <= an)\n=> :Item\n\n<<< @(mkA)\n",
            ),
            (
                "b.td",
                "Entry = @(price: Float, qty: Str)\n\nmkB bn: Float =\n  Entry(price <= bn, qty <= \"many\")\n=> :Entry\n\n<<< @(mkB)\n",
            ),
            (
                "c.td",
                "Item = @(v: Int, label: Str <= \"C\")\nmkC n: Int =\n  Item(v <= n)\n=> :Item\n<<< @(mkC)\n",
            ),
            (
                "d.td",
                "Item = @(v: Str, weight: Float <= 2.5)\nmkD s: Str =\n  Item(v <= s)\n=> :Item\n<<< @(mkD)\n",
            ),
            (
                "main.td",
                r#">>> ./a.td => @(mkA)
>>> ./b.td => @(mkB)
>>> ./c.td => @(mkC)
>>> ./d.td => @(mkD)

x <= mkA(3)
stdout(jsonEncode(x))
y <= mkB(3.0)
stdout(jsonEncode(y))
stdout(jsonEncode(x))
c <= mkC(5)
d <= mkD("dee")
stdout(c.v + 1)
stdout(d.v + "!")
stdout(c)
stdout(d)
stdout(jsonEncode(d))
stdout(jsonEncode(c))
"#,
            ),
        ],
        "{\"label\":\"a-item\",\"qty\":3}\n{\"price\":3.0,\"qty\":\"many\"}\n{\"label\":\"a-item\",\"qty\":3}\n6\ndee!\n@(v <= 5, label <= \"C\", __type <= \"Item\")\n@(v <= \"dee\", weight <= 2.5, __type <= \"Item\")\n{\"v\":\"dee\",\"weight\":2.5}\n{\"label\":\"C\",\"v\":5}",
        ALL_COMPILED,
    );
}

#[test]
fn a_versioned_import_in_a_td_file_is_rejected_by_every_build() {
    // Versions belong in packages.tdm. The builds used to resolve the
    // pinned package; they stop as the interpreter does, whether the entry
    // file or a module it imports names the version.
    let expected = "Versioned import '>>> acme/greeter@a.1' is only allowed in packages.tdm. In .td files, use '>>> acme/greeter => @(...)' without a version.";
    for (case, files) in [
        (
            "entry",
            vec![(
                "main.td",
                ">>> acme/greeter@a.1 => @(hello)\nstdout(hello(\"x\"))\n",
            )],
        ),
        (
            "module",
            vec![
                (
                    "lib.td",
                    ">>> acme/greeter@a.1 => @(hello)\n\ngreet n: Str =\n  hello(n)\n=> :Str\n\n<<< @(greet)\n",
                ),
                (
                    "main.td",
                    ">>> ./lib.td => @(greet)\nstdout(greet(\"x\"))\n",
                ),
            ],
        ),
    ] {
        let dir = common::unique_temp_dir(&format!("f65_versioned_import_{case}"));
        for (name, source) in &files {
            std::fs::write(dir.join(name), source).unwrap();
        }
        let interp = std::process::Command::new(common::taida_bin())
            .arg(dir.join("main.td"))
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&interp.stderr).contains(expected),
            "{case} interpreter: {}",
            String::from_utf8_lossy(&interp.stderr)
        );
        for target in ["native", "wasm-min"] {
            let out = std::process::Command::new(common::taida_bin())
                .args(["build", target])
                .arg(dir.join("main.td"))
                .arg("-o")
                .arg(dir.join(format!("out-{target}")))
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(
                !out.status.success() && stderr.contains(expected),
                "{case} {target}: {stderr}"
            );
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn an_imported_module_runs_where_its_import_stands() {
    // The interpreter runs an imported module at its import statement; the
    // compiled backends ran every import before the program's first
    // statement. A module's own imports run where they stand in it, and a
    // module reached twice runs once, at the first import.
    parity_files(
        "f65_import_runs_in_place",
        &[
            ("c.td", "stdout(\"c: runs\")\nbase <= 100\n<<< @(base)\n"),
            (
                "lib.td",
                "stdout(\"lib: before its import\")\n>>> ./c.td => @(base)\nstdout(\"lib: after its import\")\nseven <= base - 93\n<<< @(seven)\n",
            ),
            (
                "main.td",
                r#"stdout("main: before import")
>>> ./lib.td => @(seven)
stdout("main: after import")
>>> ./c.td => @(base)
stdout(seven)
stdout(base)
"#,
            ),
        ],
        "main: before import\nlib: before its import\nc: runs\nlib: after its import\nmain: after import\n7\n100",
        ALL_COMPILED,
    );
}
