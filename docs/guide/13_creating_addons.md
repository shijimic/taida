# Creating Addons (RC1.5)

> **PHILOSOPHY.md — IV.** キモい言語だ。だが、書くのも読むのもAI。眺めるのが人間の仕事。

Addons extend Taida Lang with Rust-backed functions that the main
binary loads at runtime. An addon is a Rust `cdylib` plus a
`native/addon.toml` manifest that pins the ABI, the package name, the
function table, and (optionally) prebuild distribution metadata so
users can `taida install` without needing a Rust toolchain.

This guide is aimed at addon *authors*. See `docs/reference/addon_manifest.md`
(RC15B-106/107) for the manifest reference and forward-compat policy.

---

## 1. Directory layout

A minimal addon crate sits alongside (or inside) your package:

```
my-addon/
  packages.tdm                  # Taida package manifest
  Cargo.toml                    # cdylib crate
  src/lib.rs                    # addon entry point
  native/
    addon.toml                  # RC1.5 install-time manifest
```

`Cargo.toml` must declare the crate as a `cdylib`:

```toml
[lib]
crate-type = ["cdylib"]
```

and depend on the in-tree `taida-addon` crate for the ABI types.

---

## 2. The addon.toml manifest

A minimal manifest without prebuild distribution (RC1 mode — users must
place the `.so` themselves) looks like:

```toml
abi = 1
entry = "taida_addon_get_v1"
package = "my-org/my-addon"
library = "my_addon"

[functions]
greet = 1
```

To ship a prebuild that `taida install` can fetch, add a
`[library.prebuild]` section (RC1.5):

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
| `x86_64-unknown-linux-gnu`  | Baseline (RC1.5 v1) |
| `aarch64-unknown-linux-gnu` | Baseline (RC1.5 v1) |
| `x86_64-apple-darwin`       | Baseline (RC1.5 v1) |
| `aarch64-apple-darwin`      | Baseline (RC1.5 v1) |
| `x86_64-pc-windows-msvc`    | Baseline (RC1.5 v1) |
| `x86_64-unknown-linux-musl`  | Extension (RC15B-003) |
| `aarch64-unknown-linux-musl` | Extension (RC15B-003) |
| `i686-unknown-linux-gnu`     | Extension (RC15B-003) |
| `riscv64gc-unknown-linux-gnu`| Extension (RC15B-003) |
| `x86_64-unknown-freebsd`     | Extension (RC15B-003) |

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

RC15B-005 reserves a `signatures` sub-table for future GPG / detached
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

## 3. Releasing prebuilds

The `.github/workflows/addon-prebuild-template.yml` template
(RC15B-004) automates the build-and-upload pipeline for the 5
baseline targets:

1. Copy the template into your addon repository.
2. Adjust `ADDON_NAME` and `CRATE_DIR` at the top of the file.
3. Uncomment the `push: tags:` trigger.
4. Push a `vX.Y.Z` tag.
5. The workflow builds for each target in the matrix, computes
   SHA-256 hashes, uploads the binaries to the matching GitHub
   Release, and also attaches a `prebuild-targets.toml.txt`
   fragment you can paste directly into your
   `[library.prebuild.targets]` block.

RC15B-003 extension targets (musl, i686, riscv64, FreeBSD) are
commented out in the template. Enable them one at a time after
verifying your crate builds cleanly on each — they typically need
`cross` on GitHub-hosted runners.

---

## 4. How `taida install` fetches prebuilds

When a package has a `native/addon.toml` with a `[library.prebuild]`
section, `taida install`:

1. Detects the host target (`HostTarget::detect_host_target`).
2. Looks the host triple up in `[library.prebuild.targets]`.
   Unknown host → deterministic error listing every target the
   manifest declares.
3. Expands `{version}`, `{target}`, `{ext}`, `{name}` in the URL
   template.
4. Downloads the binary over HTTPS (up to 10 redirects, see
   RC15B-106) or reads a `file://` URL (relative paths only; see
   `docs/reference/addon_manifest.md` for the security model).
5. Streams the bytes through SHA-256 and rejects any mismatch.
6. Caches the verified binary under
   `~/.taida/addon-cache/<org>/<name>/<version>/<target>/lib<name>.<ext>`
   and places a working copy at
   `.taida/deps/<pkg>/native/lib<name>.<ext>`.
7. Writes the target+hash pair into `taida.lock` as a
   `[[package.addon]]` sub-table so reproducible installs can
   verify the chain without re-downloading.

Downloads larger than ~256 KiB show a byte-count progress indicator
on stderr (RC15B-002). Users can force a re-download with
`taida install --force-refresh`, or prune the cache entirely with
`taida cache clean --addons` (RC15B-001).

---

## 5. Testing locally with `file://`

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
  access (RC15B-101).
- The path is resolved relative to the project root containing
  `packages.tdm`.
- The SHA-256 must be updated every time you rebuild, because the
  integrity check still runs.

This is how the in-tree `taida-addon-terminal-sample` crate is
exercised in `tests/addon_terminal_install_e2e.rs`.

---

## 6. Checklist before releasing

- [ ] `cargo build --release --target <triple>` succeeds for every
      target in `[library.prebuild.targets]`
- [ ] SHA-256s in the manifest match the uploaded artefacts
- [ ] `taida install` completes end-to-end against a local
      `file://` URL before you tag the release
- [ ] The `[functions]` table lists every symbol your `cdylib`
      exports through `declare_addon!`
- [ ] Your README tells users the **minimum supported taida
      version** (older taidas will reject unknown manifest keys by
      design; see `docs/reference/addon_manifest.md`)
