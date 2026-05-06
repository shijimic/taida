# Security policy

## Supported versions

Security fixes are applied to the latest release on the `main` branch
of `taida-lang/taida`. Older labelled releases (`@c.xx.rc*`) receive
fixes only if they are the current `rc` track. There is no long-term
support (LTS) branch.

At the time of the `@c.25.rc7` RC cycle the supported line is:

- `@c.25.rc*` — current RC, receives all fixes.
- `@c.24.rc1` — predecessor; receives critical fixes only until
  `@c.25.rc7` ships.
- Anything older — **unsupported**. Reinstall from
  <https://github.com/taida-lang/taida/releases> to move forward.

## Reporting a vulnerability

Please do **not** file a public GitHub issue for security
vulnerabilities.

Use the GitHub private security advisory flow:

<https://github.com/taida-lang/taida/security/advisories/new>

Reports are triaged by the maintainers; we aim to acknowledge within
72 hours and to publish an advisory within 30 days of acknowledgement.

## Known accepted risks

The Taida Lang runtime has **opt-in** OS and shell access surfaces
(the `taida-lang/os` package, `execShell`, `run`, unrestricted file
I/O, unrestricted `tcpListen` bind address). These surfaces are
**intentionally unsandboxed** in the current RC cycle; a Taida program
that imports `taida-lang/os` runs with the same privileges as the
user executing it.

Concretely, the following behaviours are classified as **accepted
risk** for `@c.25.rc*` and are documented here so that operators of
Taida code can plan around them:

- `execShell` executes user-supplied strings via `/bin/sh -c`
  (or `cmd /C` on Windows) without sanitisation. Prefer `run()` —
  which uses argv-style separation and does not invoke a shell —
  whenever the command does not actually need shell features.
- `Read` / `writeFile` / `writeBytes` / `appendFile` / `remove` /
  `createDir` / `rename` / `readBytes` / `ListDir` / `Stat` / `Exists`
  in `taida-lang/os` accept arbitrary absolute paths without a sandbox.
- `tcpListen(port)` binds to `0.0.0.0` (all interfaces). Operators
  running untrusted Taida programs should rely on OS-level firewalls
  to constrain reachability.

A capability / permission model (along the lines of Deno's
`--allow-run` / `--allow-read` / `--allow-write`) is **planned for
the D26 breaking-change phase** and will be introduced alongside a
namespaced redesign of the `taida-lang/os` surface.

Each finding from the audit round carries one of the following
states:

- **MITIGATED** — fix has landed.
- **ACCEPTED** — by design; surface-level contract published here.
- **DEFERRED** — real issue, fixed before the next labelled release
  (`@c.26.rc*`).
- **FALSE_POSITIVE** — ruled out with evidence.

No finding is in an undecided state.

## Supply-chain pinning

`taida upgrade` (pre-`@c.15.rc3`) used to read releases from a
personal GitHub fork; this was rotated to `taida-lang/taida` in
`@c.15.rc3` (`src/upgrade.rs::canonical_release_source_is_taida_lang_org`
pins the value against accidental regression). No GitHub Security
Advisory is currently published for this window — Taida Lang has no
confirmed install base as of `@c.26`, so there are no affected
parties to notify. If an install base emerges and the pre-`@c.15.rc3`
window is confirmed as exploitable against real users, a GHSA +
CVE request will be filed at that point.

Dependency-graph monitoring is done by
`.github/workflows/security.yml`, which runs `cargo-audit` (CVE
database lookup) and `cargo-deny` (licences / duplicates / yanked
crates / sources allow-list) on every push and weekly on a schedule.
Findings are surfaced as GitHub Actions warnings during `@c.25.rc7`;
promotion to hard-fail is the gate for `@c.26.rc*`.

## Upgrade path verification

`taida upgrade` performs the self-replacing binary update path and
must verify provenance before overwriting the running executable.
The contract for this path is:

- The release asset list **must** include `SHA256SUMS`. If the asset
  is missing, or the line for the downloaded binary cannot be located
  inside it, the upgrade is aborted before any file replacement
  occurs. There is no opt-out flag for this check.
- `SHA256SUMS` itself is verified with Sigstore cosign keyless
  verification. The certificate identity is pinned to a workflow path
  under `taida-lang/taida`:
  `^https://github.com/taida-lang/taida/\.github/workflows/.+@refs/tags/.+$`.
  The regular expression is a constant in the upgrader, not derived
  from any environment variable. The OIDC issuer is pinned to
  `https://token.actions.githubusercontent.com`.
- After cosign verification succeeds, the upgrader recomputes the
  SHA-256 of the downloaded binary and compares it against the line
  in `SHA256SUMS`. Only if both checks pass does the binary
  replacement proceed.
- Production builds ignore `TAIDA_GITHUB_API_URL`. The host is fixed
  to `https://api.github.com`. The environment variable is honoured
  only in test builds.

The `install.sh` script applies the same identity pin: the cosign
`--certificate-identity-regexp` value is hard-coded to the
`taida-lang/taida` workflow regex and is **not** derived from
`TAIDA_REPO`. If a fork or test repository needs to substitute the
source URL, that substitution is intentionally out of scope of the
cosign identity check.

The default value of `TAIDA_VERIFY_SIGNATURES` in `install.sh` is
`required`. Operators have to opt out explicitly (e.g.
`TAIDA_VERIFY_SIGNATURES=best-effort ./install.sh`) to fall back to
the warn-only path. A new install on an offline host without cosign
therefore fails fast, matching the hard-required policy enforced by
`taida upgrade`.

Self-upgrade staging files are written to `~/.taida/cache/upgrade/`
(directory mode `0700`). Each artefact — the candidate binary, the
`SHA256SUMS` blob, and **every** cosign bundle (including the bundle
staged by the addon signature verifier) — goes through a single
`O_NOFOLLOW | O_EXCL` + mode `0600` helper. A pre-placed symlink at
any staging path makes the upgrader fail closed instead of clobbering
its target. The legacy `/tmp/taida_upgrade_<pid>_<nanos>_*` path is no
longer used.

Before staging, the upgrader validates the cache directory itself:
the entry must be a real directory (not a symlink), owned by the
current effective UID, and the group/world mode bits must be clear.
A directory pre-created with looser bits is tightened to `0700` and
the `chmod` error path is propagated rather than silenced. When the
`taida upgrade` invoker is `root` via `sudo`, the operator should not
override `HOME` to a writable shared location — the cache directory
discovery follows `HOME` / `USERPROFILE` and assumes the directory
sits under that user's account.

Test-only helpers in the upgrade module (e.g. a `file://`-friendly
download path used by fixtures) are linked only when the `test-utils`
Cargo feature is enabled. Default release builds (`cargo build
--release`) do not enable the feature, so the helper symbols are
absent from production binaries and downstream crates depending on
`taida` cannot reach them.

## httpServe connection isolation

A malformed request from a single client must never tear down the
listener thread or the other concurrent connections. The Native
runtime calls `taida_net4_abort_connection()` — which `shutdown(fd,
SHUT_RDWR)`s the offending socket and sets a per-request abort flag —
on every wire-data validation failure (chunked-size overflow, invalid
chunked trailer, truncated body, WebSocket UTF-8 violation, malformed
WebSocket close frame, invalid close code, generic frame protocol
error). The accept loop checks the flag after the handler returns and
closes that connection without re-entering keep-alive; sibling
connections continue to be served. Process-wide `exit(1)` is reserved
for handler programmer errors (token mismatch, calling a streaming
API after WebSocket upgrade, etc.) which are not attacker-reachable.

`Transfer-Encoding: chunked` size accumulation uses
`__builtin_mul_overflow` / `__builtin_add_overflow` on `uint64_t` and
bounds the result to `SIZE_MAX`. The streaming readBodyChunk path
uses `strtoull` with `errno == ERANGE` checks instead of `strtoul`,
so 32-bit (LP32 / ILP32) builds cannot wrap an over-large
`chunk-size` to a smaller value and admit a request-smuggling vector.

Response header validators in all three backends share a single
RFC 7230 / 9110 grammar check. `name` is restricted to the token set
(rejects NUL, `:`, SP, HTAB, CR, LF, control bytes, and underscore
to avoid CL.CL smuggling against reverse proxies that vary on
`underscores_in_headers`); `value` is restricted to HTAB / SP / VCHAR
/ obs-text. `Content-Length`, `Transfer-Encoding`, and `Set-Cookie`
are runtime-reserved. `httpEncodeResponse` (eager path) calls into
the same helpers, so the eager and streaming paths cannot diverge.

## Source package pinning

Source-package downloads consumed via `packages.tdm` are pinned by
SHA-256 in the manifest. The package store recomputes the SHA-256
from the downloaded bytes and rejects any mismatch before the cache
is written. Cosign verification is required for any source package
whose origin matches the official release URL pattern; non-official
source URLs are rejected during the supported window.

Production builds ignore `TAIDA_GITHUB_BASE_URL`; the host is fixed
to `https://github.com`. `TAIDA_VERIFY_SIGNATURES` defaults to
`required`, and any value other than `required` causes a production
binary to refuse to start. Test builds may relax these constraints
through a build feature, but the released binary distributed via
`install.sh` does not enable that feature.
