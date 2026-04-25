# Addon Manifest Reference (`native/addon.toml`)

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

This reference documents the `native/addon.toml` schema accepted by
the Rust addon foundation. For a walkthrough of how to write and
ship an addon, see `docs/guide/13_creating_addons.md`. Tag-by-tag
land history is in `CHANGELOG.md`.

The parser is intentionally a **hand-written strict subset of TOML**
implemented in `src/addon/manifest.rs`. It is not the upstream `toml`
crate, and it only accepts the sections and keys listed in this
document.

---

## Backend support policy

The manifest schema does **not** change between backends — the same
`native/addon.toml` is authoritative for every backend that
supports addons. What differs is whether a given Taida backend
currently routes an addon-backed import through its dispatcher:

| Backend        | `supports_addons` | Notes |
|----------------|-------------------|-------|
| Interpreter    | **Yes**           | Dispatches through `dlopen` when the interpreter binary is built with `feature = "native"` (the default build). The addon facade runs as a dynamic Taida module in a dedicated environment. |
| Native (AOT)   | **Yes**           | Lowered at build time. The facade is statically analysed by `src/addon/facade.rs` into an `AddonFacadeSummary`; facade FuncDefs become IR functions, pack / scalar / list / template bindings are replayed into the module init path, and cdylib calls go through `taida_addon_call`. |
| JS transpiler  | **No**            | No JS-side dispatcher exists. Imports produce a deterministic error message pointing at `Run 'taida build --target native' or use the interpreter`. |
| WASM (min / wasi) | **No**         | The wasm dispatcher is not currently shipped. When it lands it will reuse the `src/addon/facade.rs` static analyser so published manifests do not need to change. The compatibility contract for that future widening is described under `targets` below — manifests authored today opt into `["native"]` and must not be auto-rerouted to `wasm` without a manifest edit. |

### Error text for unsupported backends

The error message emitted on an unsupported backend is fixed:

```
addon-backed package 'X' is not supported on backend 'Y' (supported:
interpreter, native). Run 'taida build --target native' or use the
interpreter.
```

Tooling that matches on the policy should prefer the
`"supported: interpreter, native"` prefix; the surface text is
covered by `docs/STABILITY.md` §4.2.

See `docs/guide/13_creating_addons.md` for the author-facing view of
which facade constructs the native backend's static analyser
understands.

---

## Required top-level keys

```toml
abi     = 1                              # integer — must equal TAIDA_ADDON_ABI_VERSION
entry   = "taida_addon_get_v1"           # string — must equal TAIDA_ADDON_ENTRY_SYMBOL
package = "my-org/my-addon"              # string — "<org>/<name>", matched against packages.tdm
library = "my_addon"                     # string — cdylib filename stem (no lib prefix, no ext)
```

`abi`, `entry`, `package`, and `library` are all required. Any
mismatch with the frozen ABI v1 constants is a parse error, not a
load-time warning.

## `targets` (optional, top-level)

```toml
targets = ["native"]
```

`targets` declares the set of Taida backends the addon expects its
cdylib to be dispatched through. The field is **optional** at the
source level but **always populated** in the parsed manifest:

- **Default (key omitted)**: the parser explicitly injects
  `["native"]`. The omitted form and an explicit
  `targets = ["native"]` produce a **bit-identical**
  `AddonManifest` — same struct values, same diagnostic strings.
- **Allowed entries**: drawn from a closed allowlist. The current
  allowlist is `{"native"}`; any other entry (including `"wasm"`,
  `"Native"`, `"unknown"`) is rejected at parse time.
- **Empty array**: `targets = []` is rejected. Authors who want
  the default must omit the key entirely; an empty array would
  otherwise let an addon opt out of the contract by writing a
  technically-valid value.
- **Duplicates**: collapsed silently — `targets = ["native", "native"]`
  is normalised to `["native"]` so the bit-identical guarantee
  survives author typos.
- **Wrong type**: a string, integer, or table value for `targets`
  is rejected as `AddonTargetsTypeMismatch`.

### Compatibility contract

The contract is the addon-side counterpart of the stable promise
in `docs/STABILITY.md` §6 (breaking-change policy):

1. **The default value is part of the surface.** Today every
   manifest that omits `targets` resolves to `["native"]`. This
   default is pinned for the lifetime of the current generation.
2. **Default changes are gen-bumps.** The default value (or the
   meaning of the omitted form) may only change at a generation
   boundary — never within a generation, never as a silent fallback
   in a point release. A widened allowlist (e.g. adding `"wasm"`
   when the wasm dispatcher lands) MAY arrive within a generation,
   but only as an additive change: existing manifests that say
   `targets = ["native"]` keep behaving identically.
3. **Unknown targets reject early.** The parser refuses unknown
   entries with `[E2001] unknown addon target` rather than falling
   back to the default. Silent fallback would be a foot-gun that
   forces the dispatcher to guess what the author meant.

### Diagnostic codes

| Code     | Variant                       | When |
|----------|-------------------------------|------|
| `E2001`  | `UnknownAddonTarget`          | A `targets` entry is not in the supported allowlist (currently `{"native"}`). |
| `E2002`  | `EmptyAddonTargets`           | `targets = []` — the array was present but empty. |
| (none)   | `AddonTargetsTypeMismatch`    | `targets` was the wrong shape (e.g. a bare string or integer). |

## `[functions]`

```toml
[functions]
greet = 1
noop  = 0
```

At least one function entry is required. Keys are the function name
as it appears in Taida source; values are the declared arity as a
non-negative integer. Non-integer arities and duplicate keys are
rejected at parse time.

## `[library.prebuild]` (optional)

```toml
[library.prebuild]
url = "https://example.com/releases/{version}/lib{name}-{target}.{ext}"
```

Declares where `taida install` should fetch the prebuild cdylib. If
this section is absent, the addon falls back to a "developer places
the `.so` manually" mode. If this section is present:

- `url` is required and must be a string.
- Template variables are `{version}`, `{target}`, `{ext}`, `{name}`.
- Unknown variables, unbalanced braces, and `{{` / `}}` escapes are
  rejected at parse time.

### `[library.prebuild.targets]` (required when `[library.prebuild]` is present)

```toml
[library.prebuild.targets]
"x86_64-unknown-linux-gnu"  = "sha256:abcdef0123...64 chars total"
"aarch64-apple-darwin"      = "sha256:..."
```

- Keys must be canonical target triples accepted by
  `HostTarget::from_triple`. The full list of supported triples is
  documented in `docs/guide/13_creating_addons.md`.
- Values must be `sha256:` + exactly 64 **lowercase** hex characters.
  Uppercase hex is rejected to enforce canonical form across
  platforms.
- Unknown / non-canonical target triples are rejected at parse time
  with `PrebuildUnknownTarget` — this prevents cache-directory
  traversal attacks by stopping attacker-controlled keys long
  before they reach `path.join()`.

### `[library.prebuild.signatures]` (reserved)

```toml
[library.prebuild.signatures]
"x86_64-unknown-linux-gnu" = "gpg:<opaque-identifier>"
```

- Reserved for future GPG / detached signature verification.
- Keys must be canonical target triples (same rule as
  `[library.prebuild.targets]`).
- Values must start with `gpg:` followed by a non-empty
  printable-ASCII payload (no whitespace, no control characters).
  Other prefixes (`sigstore:`, …) are rejected so the reserved
  namespace stays clean for a future verifier.
- Taida currently **parses and stores** these entries but does not
  verify them. Adding them today is safe: when the verifier lands
  it will read from this exact field.

---

## HTTPS download limits

When `taida install` downloads a prebuild over HTTPS it configures
the HTTP client with the following explicit policies:

| Policy           | Value                  | Notes |
|------------------|------------------------|-------|
| Request timeout  | 120 seconds            | End-to-end — applies to the whole request |
| Max redirects    | **10**                 | `reqwest::redirect::Policy::limited(10)` (constant `HTTPS_MAX_REDIRECTS` in `src/addon/prebuild_fetcher.rs`) |
| Max payload      | 100 MB                 | `Content-Length` is rejected before download starts when over the limit; streaming bodies are aborted at 100 MB |
| HTTP downgrade   | **rejected**           | `reqwest`'s default redirect policy blocks `https → http` transitions; we do not relax it |
| Scheme whitelist | `https://`, `file://`  | Everything else (including `http://`) is rejected before any network call |

Redirect chains longer than 10 hops result in a deterministic
`DownloadFailed` error rather than a silent infinite loop. The
limit is intentionally high enough for common CDN redirects
(GitHub → CDN → object store) but low enough to catch redirect
loops quickly.

For `file://` URLs, the fetcher rejects:

- Absolute paths (e.g. `file:///etc/passwd`)
- Any path containing `..` components (path-traversal guard)
- Any URL scheme other than `file://` or `https://`

These checks run **before** any filesystem access or network I/O.

---

## Unknown-key forward-compatibility policy

The manifest parser is **strict**: any section header or top-level
key not listed in this document is a parse error. This is a
deliberate ABI-drift guard:

- Unknown sections (e.g. `[library.experimental]`) are rejected.
- Unknown top-level keys (e.g. `maintainer = "..."`) are rejected.
- Unknown keys inside known sections are rejected.
- Duplicate keys (including in `[functions]`) are rejected.

Forward-compat rules for **manifest authors**:

1. **Adding a new section** to this reference is equivalent to an
   ABI bump. Wait for a taida release that understands the section
   before using it, and document the minimum supported
   `taida` version in your addon's README.
2. **Adding a new optional key** inside an existing reserved
   section is also an ABI bump — authors must update the parser
   in lock-step, and older taidas will refuse to load manifests
   that carry the new key.
3. **Writing a manifest with a future key** on an older taida is
   expected to fail. This is a feature, not a bug: silent
   tolerance would mean that a key which becomes load-bearing in a
   later version silently breaks anyone still on the older taida.

Forward-compat rules for **host tool implementers**:

1. New variants of `AddonManifestError` go at the end of the
   `#[non_exhaustive]` enum. Display format of existing variants
   must not change (the resolver pins on the `addon manifest
   error:` prefix).
2. New sections must be added to every branch of
   `parse_minimal_toml`'s section dispatcher, plus a validator in
   `parse_addon_manifest_str`, plus tests that cover both the
   happy path and at least one failure case.
3. The hand-written `is_valid_key` / target-keyed section
   detection must be updated consistently — `[library.prebuild.signatures]`
   is the canonical example of the pattern (target-triple keyed
   section with strict prefix validation).

---

## Error taxonomy

Every error produced while parsing or validating `native/addon.toml`
is an `AddonManifestError` variant. Display format is deterministic
and starts with `addon manifest error:`. The variants currently in
use:

| Variant                                | When |
|----------------------------------------|------|
| `ReadFailed`                           | `fs::read_to_string` failed |
| `Syntax`                               | Line outside the accepted subset |
| `MissingKey`                           | Required top-level key absent |
| `AbiUnsupported`                       | `abi` did not equal `TAIDA_ADDON_ABI_VERSION` |
| `EntryMismatch`                        | `entry` did not equal `TAIDA_ADDON_ENTRY_SYMBOL` |
| `MissingPackageId` / `MissingLibrary`  | Empty required string |
| `NoFunctions`                          | `[functions]` absent or empty |
| `InvalidArity`                         | Function arity not a non-negative integer |
| `TypeMismatch`                         | Key value had wrong type |
| `PrebuildMissingUrl`                   | `[library.prebuild]` present without `url` |
| `PrebuildInvalidSha256`                | `targets.*` not `sha256:` + 64 lowercase hex |
| `PrebuildUnknownUrlVariable`           | `{foo}` not in `{version|target|ext|name}` |
| `PrebuildUnbalancedBrace`              | Lone `{` or `}` in URL template |
| `PrebuildDuplicateTarget`              | Same target listed twice under `targets` |
| `PrebuildUnknownTarget`                | Target key not in `HostTarget::from_triple` |
| `PrebuildInvalidSignatureFormat`       | Signature value not `gpg:<opaque>` |
| `PrebuildSignatureUnknownTarget`       | Signature key not a canonical triple |
| `PrebuildDuplicateSignatureTarget`     | Same target listed twice under `signatures` |
| `UnknownAddonTarget` (`E2001`)         | Top-level `targets` entry not in the supported allowlist |
| `EmptyAddonTargets` (`E2002`)          | Top-level `targets = []` (empty array) |
| `AddonTargetsTypeMismatch`             | Top-level `targets` was not an array of strings |

---

## `_meta.toml` store sidecar

`taida install` writes a provenance sidecar next to every extracted
store package at `~/.taida/store/<org>/<name>/<version>/_meta.toml`.
The sidecar is auto-generated and should not be edited by hand.

```toml
# auto-generated by taida install
# Do not edit by hand.
schema_version = 1
commit_sha = "<40-char hex commit SHA the version tag pointed at>"
tarball_sha256 = "<64-char hex SHA-256 of the tarball before extraction>"
fetched_at = "<RFC-3339 UTC timestamp>"
source = "github:<org>/<name>"
version = "<version string as requested>"
```

| Field | Purpose |
|---|---|
| `schema_version` | Format version (currently `1`). A future schema bump is detected via `UnknownMetaSchema` and forces a pessimistic refresh. |
| `commit_sha` | Commit SHA the version tag pointed at when last fetched. Empty string means the SHA was not known at fetch time (e.g. first install under C17); the next install fills it in via a pessimistic refresh. |
| `tarball_sha256` | SHA-256 of the tarball before extraction. |
| `tarball_etag` | Optional HTTP ETag; field is omitted when absent. |
| `fetched_at` | RFC-3339 UTC timestamp of the fetch (whole seconds). |
| `source` | Origin identifier, e.g. `github:<org>/<name>`. |
| `version` | Version string as requested. |

The sidecar is consulted on every subsequent `taida install` to decide
whether the cached entry is still valid (see the decision table in
`docs/reference/cli.md#taida-install`). The addon manifest schema
itself (`native/addon.toml`) is not affected; the sidecar lives inside
the store cache, not inside the published package.
