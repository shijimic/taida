# Addon Manifest Reference (`native/addon.toml`)

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

This reference documents the `native/addon.toml` schema accepted by
the Rust addon foundation introduced in RC1 and extended by the
install-time pipeline in RC1.5. For a walkthrough of how to write
and ship an addon, see `docs/guide/13_creating_addons.md`.

The parser is intentionally a **hand-written strict subset of TOML**
implemented in `src/addon/manifest.rs`. It is not the upstream `toml`
crate, and it only accepts the sections and keys listed in this
document.

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

## `[library.prebuild]` (RC1.5, optional)

```toml
[library.prebuild]
url = "https://example.com/releases/{version}/lib{name}-{target}.{ext}"
```

Declares where `taida install` should fetch the prebuild cdylib. If
this section is absent, the addon is in RC1-style "developer
places the `.so` manually" mode. If this section is present:

- `url` is required and must be a string.
- Template variables are `{version}`, `{target}`, `{ext}`, `{name}`.
- Unknown variables, unbalanced braces, and `{{` / `}}` escapes are
  rejected at parse time.

### `[library.prebuild.targets]` (RC1.5, required when `[library.prebuild]` is present)

```toml
[library.prebuild.targets]
"x86_64-unknown-linux-gnu"  = "sha256:abcdef0123...64 chars total"
"aarch64-apple-darwin"      = "sha256:..."
```

- Keys must be canonical target triples accepted by
  `HostTarget::from_triple` (the 5 RC1.5 v1 baseline targets plus
  the RC15B-003 extensions listed in
  `docs/guide/13_creating_addons.md`).
- Values must be `sha256:` + exactly 64 **lowercase** hex characters.
  Uppercase hex is rejected to enforce canonical form across
  platforms.
- Unknown / non-canonical target triples are rejected at parse time
  with `PrebuildUnknownTarget` — this prevents cache-directory
  traversal attacks by stopping attacker-controlled keys long
  before they reach `path.join()`.

### `[library.prebuild.signatures]` (RC15B-005, reserved)

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

## RC15B-106: HTTPS download limits

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
- Any path containing `..` components (RC15B-101)
- Any URL scheme other than `file://` or `https://`

These checks run **before** any filesystem access or network I/O.

---

## RC15B-107: Unknown-key forward-compatibility policy

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
   section (for example, adding `signatures` to
   `[library.prebuild]` in RC15B-005) is also an ABI bump —
   authors must update the parser in lock-step, and older taidas
   will refuse to load manifests that carry the new key.
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
   detection must be updated consistently — see the
   `[library.prebuild.signatures]` addition in RC15B-005 for the
   pattern.

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
| `PrebuildUnknownTarget` (RC15B-103)    | Target key not in `HostTarget::from_triple` |
| `PrebuildInvalidSignatureFormat` (RC15B-005) | Signature value not `gpg:<opaque>` |
| `PrebuildSignatureUnknownTarget` (RC15B-005) | Signature key not a canonical triple |
| `PrebuildDuplicateSignatureTarget` (RC15B-005) | Same target listed twice under `signatures` |
