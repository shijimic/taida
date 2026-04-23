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

The full triage of the 2026-03-19 audit round is tracked in
`.dev/taida-logs/docs/archive/SECURITY_AUDIT.md` and summarised in
the C25 blocker `C25B-006`. Each finding carries one of:

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
pins the value against accidental regression). The advisory for the
pre-`@c.15.rc3` window is tracked under C25B-014 and will be
published at
<https://github.com/taida-lang/taida/security/advisories> once
issued.

Dependency-graph monitoring is done by
`.github/workflows/security.yml`, which runs `cargo-audit` (CVE
database lookup) and `cargo-deny` (licences / duplicates / yanked
crates / sources allow-list) on every push and weekly on a schedule.
Findings are surfaced as GitHub Actions warnings during `@c.25.rc7`;
promotion to hard-fail is the gate for `@c.26.rc*`.
