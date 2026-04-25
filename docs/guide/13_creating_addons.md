# Creating Addons

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

Addons extend Taida Lang with Rust-backed functions that the main
binary loads at runtime. An addon is a Rust `cdylib` plus a
`native/addon.toml` manifest that pins the ABI, the package name, the
function table, and (optionally) prebuild distribution metadata so
users can `taida install` without needing a Rust toolchain.

This guide is aimed at addon *authors*. See `docs/reference/addon_manifest.md`
for the manifest reference and forward-compat policy, and
`docs/reference/cli.md` for the `taida publish` CLI contract.

### Backend policy (what runs your addon)

The supported backend matrix for addons is:

| Backend        | Status      | What happens at import                                                                 |
|----------------|-------------|----------------------------------------------------------------------------------------|
| Interpreter    | **Supported** | Addon facade runs as a dynamic module; cdylib functions dispatch via `dlopen` when the interpreter binary is built with `feature = "native"` (the default). |
| Native (AOT)   | **Supported** | Addon facade is statically analysed into an `AddonFacadeSummary` by `src/addon/facade.rs` and lowered through `src/codegen/lower/imports.rs`. FuncDefs become IR functions; pack / scalar / list / template bindings are replayed into the module init path. |
| JS transpiler  | **Rejected**  | There is no JS-side addon dispatcher today. The import produces a deterministic error. |
| WASM (min / wasi) | **Rejected** | The wasm dispatcher is not currently exposed. Future widening will reuse the `src/addon/facade.rs` static analyser so authors will not have to re-write facades. See `docs/STABILITY.md` §1.2. |

Authors targeting interpreter + native do not need to write two
facade files. The same `taida/<stem>.td` must work on both — the
interpreter resolves user imports against the facade's live
environment snapshot, and the native backend resolves against the
`AddonFacadeSummary` extracted by the shared loader. Every construct
accepted on one path is accepted on the other (see
[What the native backend understands inside a facade](#what-the-native-backend-understands-inside-a-facade)
immediately below).

### What the native backend understands inside a facade

The facade loader accepts the following top-level constructs:

- **Aliases** — `FacadeName <= lowercaseAddonFn`, where
  `lowercaseAddonFn` appears in the manifest's `[functions]` table.
- **Pack literals** — `FacadeName <= @(field <= value, ...)`.
- **Scalar / list / arithmetic / template bindings** —
  `N <= 0`, `msg <= "hello"`, `greet <= \`hi, ${who}\``, arithmetic
  expressions, function / method calls, field accesses, mold /
  type instantiations.
- **FuncDefs** — `Name args = body => :Type`. Both
  exported (public) and private (`_`-prefixed) FuncDefs are
  collected; privates promoted into the summary through
  transitive reachability from an exported FuncDef body or pack
  binding.
- **Relative imports** — `>>> ./child.td =>
  @(Sym1, Sym2)`. Non-relative paths (`>>> taida-lang/foo`,
  `>>> npm:*`, versioned imports) are rejected.
- **Export declarations** — `<<< @(Sym1, Sym2)`. When present, the
  `<<<` clause is authoritative — symbols absent from it cannot be
  named in user imports.

Currently rejected (no real addon in the ecosystem uses these today,
so the rejection is informational):

- `TypeDef` / `EnumDef` / `MoldDef` statements inside a facade.
- `<<< <path>` re-exports.

If your facade depends on any of these, compile errors at
`taida build --target native` will indicate which construct was
rejected. Interim workaround: expose a facade FuncDef that wraps
the missing construct with a pure-Taida surface the loader does
understand.

The publish and release workflow is **tag-push-only**: `taida publish`
just pushes a git tag and exits, and the addon repository's own CI
(`.github/workflows/release.yml`, scaffolded by `taida init --target
rust-addon`) builds and publishes the release as
`github-actions[bot]`. See [§3 The release workflow](#3-the-release-workflow)
below for the symmetric 4-job pipeline, and [§8 Migrating older
addons](#8-migrating-older-addons) for the steps existing addons
may need to take.

---

## 0. Getting started with `taida init --target rust-addon`

The fastest path from nothing to a publishable addon is the
built-in scaffold. `taida init --target rust-addon` writes the
complete on-disk layout you need — Rust crate, facade, manifest,
and the C14 release workflow — in one step:

```bash
$ taida init --target rust-addon my-addon
Initialized Taida project 'my-addon' (rust-addon) in my-addon
  packages.tdm
  Cargo.toml
  src/lib.rs
  native/addon.toml
  taida/my-addon.td
  .gitignore
  README.md
  .github/workflows/release.yml
```

What you get:

- **`packages.tdm`** with a `<<<@a` placeholder identity. Before
  your first publish, replace it with the qualified form
  `<<<@a owner/my-addon @(MyExport, ...)` — `taida publish`
  will reject a bare identity.
- **`Cargo.toml`** with `crate-type = ["rlib", "cdylib"]` and
  `taida-addon = "2.0"` (the ABI v1 author crate).
- **`src/lib.rs`** with a minimal `declare_addon!` entry point
  exporting a sample `echo` function through
  `taida_addon_get_v1`.
- **`native/addon.toml`** with `abi = 1`, an `OWNER/...`
  placeholder in `package` / `url`, and an empty
  `[library.prebuild.targets]` table. CI fills the SHA-256
  target entries at release time through the `addon.lock.toml`
  path (see §3 and §5 below).
- **`taida/<name>.td`** — your Taida-side facade. Imports from
  this package resolve against the symbols this file exports.
- **`.github/workflows/release.yml`** — the C14 template,
  symmetric with Taida core's own release workflow. See §3.

Next steps:

1. Replace `OWNER` in `native/addon.toml` (two places) with your
   GitHub org or user.
2. Replace the `<<<@a` placeholder in `packages.tdm` with the
   qualified form and declare your exports.
3. `cargo build --release` to verify the cdylib compiles.
4. (Optional) point `native/addon.toml`'s `prebuild.url` at a
   relative `file://target/release/lib<name>.so` to test
   `taida install` locally against your own build output —
   see §6.
5. When you are ready to cut the first release, push the
   repository to GitHub and run `taida publish --dry-run`
   to preview the version bump. `taida publish` (without
   `--dry-run`) creates and pushes the tag; CI does the
   rest (§3, §4).

The same scaffold is what the `taida-lang/terminal` addon is
built on — its `.github/workflows/release.yml` is the template
in this repo with the two placeholders substituted. If the
scaffold ever drifts from terminal's working setup, the
symmetry is re-asserted by
`tests/init_release_workflow_symmetry.rs`.

---

## 1. Directory layout

A minimal addon crate sits alongside (or inside) your package:

```
my-addon/
  packages.tdm                  # Taida package manifest
  Cargo.toml                    # cdylib crate
  src/lib.rs                    # addon entry point
  native/
    addon.toml                  # install-time manifest
```

`Cargo.toml` must declare the crate as a `cdylib`:

```toml
[lib]
crate-type = ["cdylib"]
```

and depend on the in-tree `taida-addon` crate for the ABI types.

---

## 2. The addon.toml manifest

A minimal manifest without prebuild distribution (users must place
the `.so` themselves) looks like:

```toml
abi = 1
entry = "taida_addon_get_v1"
package = "my-org/my-addon"
library = "my_addon"

[functions]
greet = 1
```

To ship a prebuild that `taida install` can fetch, add a
`[library.prebuild]` section:

```toml
[library.prebuild]
url = "https://github.com/my-org/my-addon/releases/download/v{version}/lib{name}-{target}.{ext}"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu"  = "sha256:<64 lowercase hex chars>"
"aarch64-unknown-linux-gnu" = "sha256:..."
"x86_64-apple-darwin"       = "sha256:..."
"aarch64-apple-darwin"      = "sha256:..."
"x86_64-pc-windows-msvc"    = "sha256:..."
```

### URL template variables

| Variable    | Expands to |
|-------------|------------|
| `{version}` | The exact version resolved by `taida install` |
| `{target}`  | The host target triple (e.g. `x86_64-unknown-linux-gnu`) |
| `{ext}`     | Platform cdylib extension (`so`, `dylib`, `dll`) |
| `{name}`    | The `[library] name` value |

Unknown variables, unbalanced braces, and `{{` / `}}` escapes are
rejected at manifest parse time — there is no tolerance for typos.

### Supported host targets

| Triple | Status |
|--------|--------|
| `x86_64-unknown-linux-gnu`  | Baseline |
| `aarch64-unknown-linux-gnu` | Baseline |
| `x86_64-apple-darwin`       | Baseline |
| `aarch64-apple-darwin`      | Baseline |
| `x86_64-pc-windows-msvc`    | Baseline |
| `x86_64-unknown-linux-musl`  | Extension |
| `aarch64-unknown-linux-musl` | Extension |
| `i686-unknown-linux-gnu`     | Extension |
| `riscv64gc-unknown-linux-gnu`| Extension |
| `x86_64-unknown-freebsd`     | Extension |

You do not have to ship binaries for every target — only the ones you
have tested. Users on unlisted targets will get a deterministic
`addon is not available for your platform` error at install time,
which lists the targets your manifest does declare.

### SHA-256 integrity

The `targets` values are always lowercase `sha256:` + 64 hex chars.
Uppercase hex is rejected for canonical form. `taida install`
streams the downloaded bytes through SHA-256 and aborts with a
structured error on mismatch — there is no silent fallback.

### Reserved: `[library.prebuild.signatures]`

A `signatures` sub-table is reserved for future GPG / detached
signature verification:

```toml
[library.prebuild.signatures]
"x86_64-unknown-linux-gnu" = "gpg:<opaque-identifier>"
```

Values must start with `gpg:` and carry a non-empty printable-ASCII
payload. Taida currently **parses and stores** these values but does
not verify them. Adding them now is safe — older taidas that don't
understand the section will reject the manifest, which is the
intended forward-compat policy (see
`docs/reference/addon_manifest.md`).

---

## 3. The release workflow

The addon release pipeline is a **4-job CI workflow**
(`.github/workflows/release.yml`) that is structurally symmetric
with the Taida core's own `release.yml`. `taida init --target
rust-addon` scaffolds this workflow automatically when you create
a new addon crate. Older addons that pre-date this layout may
need migration (see [§8 Migrating older addons](#8-migrating-older-addons)).

The template lives at
`crates/addon-rs/templates/release.yml.template` in the Taida
repository. It is rendered with two placeholders at scaffold time:

| Placeholder       | Meaning                                                              |
|-------------------|----------------------------------------------------------------------|
| `{{LIBRARY_STEM}}` | cdylib filename stem (e.g. `taida_lang_terminal`) — without `lib` prefix and without extension |
| `{{CRATE_DIR}}`   | Relative path from repo root to the `Cargo.toml` (usually `.`)       |

### Trigger

The workflow fires on two events:

- `push` to a tag matching the Taida version regex
  `^[a-z]\.[0-9]+(\.[a-z0-9][a-z0-9-]*)?$` (bare — no `@` prefix).
  Examples: `a.1`, `b.3.rc`, `aa.7.beta`. Semver tags (`v1.2.3`)
  are intentionally ignored.
- `workflow_dispatch` with a `tag` input, for manually re-running a
  release against an already-pushed tag.

### Jobs

| Job       | Purpose                                                            |
|-----------|--------------------------------------------------------------------|
| `prepare` | Validate the tag regex, resolve the ref, export `release_tag` + `release_ref` outputs |
| `gate`    | `cargo fmt --check` → `cargo clippy --all-targets -- -D warnings` → `cargo test --all`  |
| `build`   | 5-platform matrix: build `cdylib` + compute SHA-256, upload artefact |
| `publish` | Download all matrix artefacts, generate `addon.lock.toml` + `prebuild-targets.toml.txt` + `SHA256SUMS`, run `gh release create` |

The 5-platform matrix is:

| Runner           | Target triple                   | `cross`? |
|------------------|----------------------------------|-----------|
| `ubuntu-latest`  | `x86_64-unknown-linux-gnu`       | no        |
| `ubuntu-latest`  | `aarch64-unknown-linux-gnu`      | yes       |
| `macos-15-intel` | `x86_64-apple-darwin`            | no        |
| `macos-14`       | `aarch64-apple-darwin`           | no        |
| `windows-latest` | `x86_64-pc-windows-msvc`         | no        |

The `publish` job authenticates with `GH_TOKEN: ${{ github.token }}`,
so the **release author is always `github-actions[bot]`** — never the
person who ran `taida publish`. This is a non-negotiable contract
of the addon release pipeline.

### Release assets

A successful `publish` job attaches 8 assets to the GitHub Release:

- 5 × `lib<LIBRARY_STEM>-<triple>.<so|dylib|dll>` (the matrix cdylibs)
- `addon.lock.toml` — CI-generated lockfile listing the SHA-256 of
  each of the 5 cdylibs. `taida install` reads this as the
  authoritative source of truth.
- `prebuild-targets.toml.txt` — a TOML fragment that could be pasted
  into `[library.prebuild.targets]` if needed, but the lockfile is
  the primary mechanism in the current pipeline.
- `SHA256SUMS` — flat text listing of every asset's SHA-256 for
  human verification.

### Reference implementation

`taida-lang/terminal` is the canonical reference: it is published
through this pipeline, and its CI run is available at the addon's
GitHub Actions tab. Both the workflow file (matching the template
with `LIBRARY_STEM=taida_lang_terminal`) and the release asset
structure can be used as a ground-truth example.

---

## 4. Publishing a new version with `taida publish`

From the addon crate root, with `packages.tdm`'s identity set to
`<<<@<version> <owner>/<name>`, a tagged release is a two-step
process:

```bash
# 1. Preview: what version would this publish?
$ taida publish --dry-run
Publish plan for my-org/my-addon:
  Last release tag: a.3
  API diff: added 2
  Next version: a.4
  Tag to push: a.4
  Remote: origin
  Dry-run: no git changes performed.

# 2. Execute: push the tag, then exit immediately.
$ taida publish
Created tag 'a.4' and pushed to origin.
CI will build and publish the release.
```

`taida publish` does **not** wait for CI. Open the GitHub Actions tab
to watch the 4 jobs complete (typically ~90 seconds for the baseline
5-platform matrix).

### Automatic version bump

`taida publish` compares the export symbol set of `taida/*.td`
between the previous release tag and HEAD. See
`docs/reference/cli.md#taida-publish` for the full bump table; the
one-line summary is:

- Symbol removed / renamed → generation bump (`a.3` → `b.1`)
- Symbol added / internal change only → number bump (`a.3` → `a.4`)
- No previous tag → `a.1` (initial release)

### Escape hatches

| Flag                        | Use case                                                    |
|-----------------------------|-------------------------------------------------------------|
| `--force-version a.5`       | Override the auto-detected version (skips API diff)         |
| `--label rc`                | Append a pre-release label (`a.4` → `a.4.rc`)               |
| `--retag`                   | Force-replace an already-pushed tag (skips API diff)        |

`--force-version` and `--retag` deliberately bypass the API diff
snapshot so that older packages (which may contain syntax now
rejected by the parser, e.g. discard-binding `[E1616]`) can still
be re-tagged without tripping the Taida parser on the old tag's
`taida/*.td`.

---

## 5. How `taida install` fetches prebuilds

When a package has a `native/addon.toml` with a `[library.prebuild]`
section, `taida install`:

1. Detects the host target (`HostTarget::detect_host_target`).
2. Looks the host triple up in `[library.prebuild.targets]`.
   Unknown host → deterministic error listing every target the
   manifest declares.
3. **SHA source selection**: if the `addon.toml` at the tag contains
   a placeholder SHA (`sha256:` + 64 zeros), the resolver falls back
   to the release asset `addon.lock.toml` for the authoritative
   hash. This is the expected path when an addon's initial release
   author left `[library.prebuild.targets]` as placeholders and
   relies on CI to publish the canonical lockfile
   (`src/pkg/resolver.rs::choose_sha_source`).
4. Expands `{version}`, `{target}`, `{ext}`, `{name}` in the URL
   template.
5. Downloads the binary over HTTPS (up to 10 redirects) or reads a
   `file://` URL (relative paths only; see
   `docs/reference/addon_manifest.md` for the security model).
6. Streams the bytes through SHA-256 and rejects any mismatch.
7. Caches the verified binary under
   `~/.taida/addon-cache/<org>/<name>/<version>/<target>/lib<name>.<ext>`
   and places a working copy at
   `.taida/deps/<pkg>/native/lib<name>.<ext>`.
8. Writes the target+hash pair into `taida.lock` as a
   `[[package.addon]]` sub-table so reproducible installs can
   verify the chain without re-downloading.

Downloads larger than ~256 KiB show a byte-count progress indicator
on stderr. Users can force a re-download with
`taida install --force-refresh`, or prune the cache entirely with
`taida cache clean --addons`.

---

## 6. Testing locally with `file://`

During development you do not need to publish to GitHub. Point the
URL template at a relative `file://` path:

```toml
[library.prebuild]
url = "file://target/release/libmy_addon.so"

[library.prebuild.targets]
"x86_64-unknown-linux-gnu" = "sha256:<compute this after each build>"
```

Constraints:

- Only **relative** paths are accepted under `file://`. Absolute
  paths and `..` components are rejected before any filesystem
  access.
- The path is resolved relative to the project root containing
  `packages.tdm`.
- The SHA-256 must be updated every time you rebuild, because the
  integrity check still runs.

This is how the in-tree `taida-addon-terminal-sample` crate is
exercised in `tests/addon_terminal_install_e2e.rs`.

---

## 7. Checklist before releasing

- [ ] `packages.tdm` declares a qualified identity
      (`<<<@<version> <owner>/<name>`) — a bare `<<<@<version>`
      with no `<owner>/<name>` is rejected by `taida publish`
- [ ] `.github/workflows/release.yml` is the standard template
      (4 jobs, 5-platform matrix, `github-actions[bot]` release
      author). Older addons that still use a `prebuild.yml`
      workflow must migrate — see
      [§8 Migrating older addons](#8-migrating-older-addons)
- [ ] The tag you plan to push does not already exist on origin
      (or you've passed `--retag` intentionally)
- [ ] `cargo build --release` succeeds locally (the CI matrix will
      catch cross-target issues, but local x86_64 failures fail fast)
- [ ] `taida install` completes end-to-end against a local
      `file://` URL during development
- [ ] The `[functions]` table lists every symbol your `cdylib`
      exports through `declare_addon!`
- [ ] Your README tells users the **minimum supported taida
      version** (older taidas will reject unknown manifest keys by
      design; see `docs/reference/addon_manifest.md`)

---

## 8. Migrating older addons

Older addons (those that used `taida publish --target rust-addon`
with `prebuild.yml` workflows and author=CLI-runner releases) need
mechanical changes to run cleanly under the current publish pipeline.

### Step 1 — Add identity to `packages.tdm`

Change a packages.tdm that uses a bare version with no owner /
name:

```taida
// older form
<<<@a.1
>>> ./main.td => @(...)
```

to the qualified-identity form:

```taida
// current form
>>> ./main.td
<<<@a.1 <owner>/<name> @(...)
```

`taida publish` will refuse to run against a bare `<<<@<version>`
(no identity) because the resolver has no way to derive a fetch URL
without qualifying `owner/name`.

### Step 2 — Replace `prebuild.yml` with the current `release.yml` template

The older `prebuild.yml` workflow had only 2 jobs (Build +
Release-attach) and assumed the CLI had already run
`gh release create` as the CLI user. In the current pipeline the CI
owns release creation.

Option A — clean scaffold: in a *separate* scratch checkout, run
`taida init --target rust-addon my-addon`, copy the generated
`.github/workflows/release.yml`, and adjust the `LIBRARY_STEM` and
`CRATE_DIR` env values at the top of the file to match your
existing project. Replace your `prebuild.yml` with this file and
delete the old one.

Option B — manual copy: `crates/addon-rs/templates/release.yml.template`
in the Taida repository is the upstream. Copy it, replace
`{{LIBRARY_STEM}}` with your cdylib stem (without `lib` prefix and
without extension) and `{{CRATE_DIR}}` with the relative path from
your repo root to the `Cargo.toml` (usually `.`).

Either way, open a PR that:

- Removes `.github/workflows/prebuild.yml`
- Adds `.github/workflows/release.yml`
- Keeps the same tag naming scheme your tests already validate
  (`a.1`, `b.1.rc`, etc.)

### Step 3 — Accept placeholder `addon.toml` + CI-generated `addon.lock.toml`

The current template publishes an `addon.lock.toml` asset as the
authoritative SHA source. In your tracked
`native/addon.toml` you can either (a) keep
`[library.prebuild.targets]` with placeholder (`sha256:` + 64 zeros)
values on `main`, or (b) delete the section entirely. Both paths
are supported by the resolver — option (b) is cleaner but requires
that every release ships `addon.lock.toml`. Option (a) is defensive
in case a future release omits the lockfile.

`taida install` auto-detects placeholder SHA values and falls back
to the release asset `addon.lock.toml`
(`is_placeholder_sha()` + `choose_sha_source()` in the Taida
source tree). See `docs/reference/addon_manifest.md` for the full
decision matrix.

### Step 4 — Drop obsolete CLI options from your scripts

Any `Makefile`, shell alias, or CI script that calls
`taida publish` must stop passing the now-removed options:

| Older form                       | Current replacement                           |
|----------------------------------|-----------------------------------------------|
| `taida publish --target rust-addon` | `taida publish` (target is implicit)       |
| `taida publish --dry-run=plan`   | `taida publish --dry-run`                     |
| `taida publish --dry-run=build`  | Removed — local build happens in CI only      |
| `TAIDA_PUBLISH_SKIP_RELEASE=1`   | Removed — the CLI never creates releases now  |

### Step 5 — (Optional) Re-tag your initial release

If your addon's existing `a.1` tag was pushed by an older CLI
(meaning the release author is a person, not `github-actions[bot]`),
you can re-run the current pipeline against the same tag:

```bash
taida publish --force-version a.1 --retag
```

This force-replaces the tag on origin, fires the new `release.yml`,
and re-creates the release with `github-actions[bot]` as the author
and the full 8-asset payload (5 cdylibs + `addon.lock.toml` +
`prebuild-targets.toml.txt` + `SHA256SUMS`).

`--force-version` and `--retag` together ensure the API diff
snapshot is skipped, so older source in your old tag's `taida/*.td`
(containing syntax now rejected by the parser, for example) does
not block the re-tag through the Taida parser.
