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
  under `taida-lang/taida` (the regular expression is a constant in
  the upgrader, not derived from any environment variable). The OIDC
  issuer is pinned to `https://token.actions.githubusercontent.com`.
- After cosign verification succeeds, the upgrader recomputes the
  SHA-256 of the downloaded binary and compares it against the line
  in `SHA256SUMS`. Only if both checks pass does the binary
  replacement proceed.
- Production builds ignore `TAIDA_GITHUB_API_URL`. The host is fixed
  to `https://api.github.com`. The environment variable is honoured
  only in test builds.

The `install.sh` script applies the same identity pin: the cosign
`--certificate-identity-regexp` value is hard-coded to
`taida-lang/taida` and is **not** derived from `TAIDA_REPO`. If a
fork or test repository needs to substitute the source URL, that
substitution is intentionally out of scope of the cosign identity
check.

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
