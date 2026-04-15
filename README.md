# Taida Lang

Taida Lang is a programming language designed for the AI collaboration era.

It limits itself to 10 operators, has no `null` or `undefined`, and does not allow implicit type conversion. The language is shaped around a simple premise: AI writes it, AI reads it, and humans inspect the structure. That is why Taida is built to make code easy to analyze as graphs.

## What makes it different

- Only 10 operators (`=`, `=>`, `<=`, `]=>`, `<=[`, `|==`, `|`, `|>`, `>>>`, `<<<`)
- Default values for every type — no null, no undefined
- Structured data through Buchi Pack `@(...)`
- Parametric typing through molding types `Mold[T]`
- Enum types with ordinal values
- Three backends: Interpreter, JS, and Native (+ WASM profiles)
- Structural tooling through `check`, `graph`, `verify`, and `inspect`
- Rust addon system for extending the language with native code

## Hello World

```taida
greet name: Str =
  "Hello, " + name + "!"
=> :Str

message <= greet("World")
stdout(message)
```

`stdout` is part of the prelude, so no import is required.

## Example: Enum and Buchi Pack

```taida
Enum => Status = :Active :Inactive :Pending

Pilot = @(
  name: Str
  sync_rate: Int
)

pilot <= Pilot(name <= "Rei", sync_rate <= 78)
stdout(pilot.name)
stdout(Status:Active() == Status:Active())   // true
```

## Backends

| Backend | Command | Output |
|---------|---------|--------|
| Interpreter | `taida file.td` | Direct execution |
| Native | `taida build file.td` | Binary executable (default) |
| JS | `taida build --target js file.td` | `.mjs` file |
| WASM | `taida build --target wasm-wasi file.td` | `.wasm` file |

All backends produce identical output for the same source code (3-way parity).

WASM profiles: `wasm-min`, `wasm-wasi`, `wasm-edge`, `wasm-full`.

## Installation

The official distribution channel is `install.sh` from `taida.dev`. `crates.io` is not used as a release channel.

Tagged releases are packaged as GitHub Release artifacts for Linux, macOS, and Windows, with a shared `SHA256SUMS` file.

To build from source:

```bash
cargo build --release
./target/release/taida examples/01_hello.td
```

Tests:

```bash
cargo test
```

## Core Packages

| Package | Description |
|---------|-------------|
| `taida-lang/net` | HTTP server/client (H1/H2/H3), WebSocket, SSE |
| `taida-lang/os` | DNS, TCP, UDP, socket operations |
| `taida-lang/crypto` | Cryptographic operations |
| `taida-lang/js` | JS interop (Molten boundary) |
| `taida-lang/pool` | Connection pooling |

## Addons

Taida supports Rust-backed native addons via `cdylib` crates:

```bash
taida init --target rust-addon my-addon   # Scaffold
taida publish --target rust-addon         # Build and release
taida install                             # Download prebuilds
```

See [Creating Addons](docs/guide/13_creating_addons.md) for details.

## Documentation

### Guide

| # | Document | Content |
|---|----------|---------|
| 00 | [Overview](docs/guide/00_overview.md) | Language overview |
| 01 | [Types](docs/guide/01_types.md) | Primitives, Enum, Collections, Molding types |
| 02 | [Strict Typing](docs/guide/02_strict_typing.md) | No implicit conversion, Lax safety |
| 03 | [JSON](docs/guide/03_json.md) | JSON as molten iron, schema-required casting |
| 04 | [Buchi Pack](docs/guide/04_buchi_pack.md) | Buchi Pack syntax |
| 05 | [Molding](docs/guide/05_molding.md) | Molding types (all operation molds) |
| 06 | [Lists](docs/guide/06_lists.md) | List operations |
| 07 | [Control Flow](docs/guide/07_control_flow.md) | Conditionals |
| 08 | [Error Handling](docs/guide/08_error_handling.md) | Lax + throw/\|== + Gorilla ceiling |
| 09 | [Functions](docs/guide/09_functions.md) | Functions, pipelines, tail recursion |
| 10 | [Modules](docs/guide/10_modules.md) | Import/Export, prelude |
| 11 | [Async](docs/guide/11_async.md) | Async[T], ]=> await |
| 12 | [Introspection](docs/guide/12_introspection.md) | Structural introspection |
| 13 | [Creating Addons](docs/guide/13_creating_addons.md) | Rust addon authoring |

### Reference

| Document | Content |
|----------|---------|
| [CLI](docs/reference/cli.md) | Command reference |
| [Operators](docs/reference/operators.md) | All 10 operators + arithmetic/comparison/logic |
| [Mold Types](docs/reference/mold_types.md) | Mold type signatures |
| [Naming](docs/reference/naming_conventions.md) | Identifier and version naming |
| [Graph Model](docs/reference/graph_model.md) | 5 graph views |
| [Doc Comments](docs/reference/documentation_comments.md) | AI collaboration tags |
| [Tail Recursion](docs/reference/tail_recursion.md) | TCO rules |
| [Scope Rules](docs/reference/scope_rules.md) | Scope-based auto management |
| [Standard Library](docs/reference/standard_library.md) | Prelude and built-in types |
| [Standard Methods](docs/reference/standard_methods.md) | State-check and monadic methods |
| [Addon Manifest](docs/reference/addon_manifest.md) | addon.toml specification |
| [Diagnostic Codes](docs/reference/diagnostic_codes.md) | Compiler diagnostic codes |

## Versioning

The canonical public release identifier is the Taida version, not the Rust package semver.

- Current release: `@c.12.rc3`
- `Cargo.toml` version `2.0.0`: semver metadata for Rust tooling

In release notes and public communication, `@c.12.rc3` is the primary version. `2.0.0` is supplementary.
