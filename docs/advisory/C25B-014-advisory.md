# `taida upgrade` trusted a personal-fork GitHub account as release source before `@c.15.rc3`

> **Status**: DRAFT — not yet published on GitHub Security Advisories.
>
> **Blocker**: `C26B-008` (`.dev/C26_BLOCKERS.md`) — advisory
> publication + CVE request, owner action.
>
> **Provenance**: Finalised copy of the working draft originally
> produced under `C25B-014` by the C25 Phase 6 driver. The older
> `.dev/security_advisories/GHSA-DRAFT-taida-upgrade-supply-chain.md`
> file remains the authoritative text until the GHSA ID is assigned;
> after publication, this staged copy is annotated with the assigned
> ID and the `.dev/` draft is retired.
>
> **Action required from the repo owner**: paste the fields below
> into <https://github.com/taida-lang/taida/security/advisories/new>,
> or drive the submission through `scripts/advisory/publish-advisory.sh`
> (`--dry-run` first, then review the body, then submit manually).
> See `docs/advisory/README.md` for the full procedure.
>
> Agents must **not** publish this advisory or request a CVE.

---

## Title (≤ 100 chars)

`taida upgrade` trusted a personal-fork GitHub account as release source before `@c.15.rc3`

## Severity (self-assessed)

**High** (CVSS 3.1 estimate: **8.1 / AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H** — single-account supply-chain compromise yields arbitrary code execution on every `taida upgrade` user).

The advisory form lets the owner pick one of {Low / Moderate / High / Critical}. Recommend **High** on the grounds that:

- the attack vector is network-reachable (`taida upgrade` fetches over HTTPS),
- the attack complexity is low (any compromise / sale / rename / takeover of a single GitHub account),
- no privileges or user interaction are needed beyond running `taida upgrade`,
- the impact is remote code execution on every affected CLI worldwide (the upgrade payload is a Taida binary; the user then runs it).

**CVE request: yes.** The form's "Request CVE ID" checkbox should be ticked. The issue is a pre-release-level design flaw that warrants a CVE record in addition to a GHSA.

## CWE(s)

- **CWE-494**: Download of Code Without Integrity Check
- **CWE-829**: Inclusion of Functionality from Untrusted Control Sphere
- **CWE-1104**: Use of Unmaintained Third Party Components *(applies only if the fork is ever deleted / emptied; the primary CWEs are 494 and 829)*

## Affected product

`taida` — the Taida Lang CLI (this repository, `taida-lang/taida`).

## Affected versions

- `< @c.15.rc3`

In practical terms: any `taida` binary downloaded before the `@c.15.rc3` release. The two public releases in that window were:

- `@c.13.rc3`
- `@c.14.rc3`

## Patched versions

- `@c.15.rc3` and later.

## Affected ecosystem

`Other` (GitHub Advisory does not have a native ecosystem for Taida addon ABI binaries; pick `Other` and write "Taida Lang standalone CLI, distributed through `install.sh` and GitHub releases of `taida-lang/taida`" in the notes).

## Description

### Summary

Before release `@c.15.rc3`, the `taida` CLI's `taida upgrade` command looked up new releases from a hard-coded **personal GitHub fork** (`shijimic/taida`) rather than the canonical organisation repository (`taida-lang/taida`). Anyone with control of that single personal account — by credential compromise, account sale, username rename, account deletion, or GitHub takeover of a released username — could replace every published `taida` binary and its `SHA256SUMS` file. Every `taida upgrade` invocation worldwide would then trust and execute attacker-controlled bytes, with no integrity check to stop it, because the older CLI only pinned **the download location** (over HTTPS) and not **the publisher's signing identity**.

### Technical impact

On affected CLIs, `taida upgrade` downloads a release archive, expands it over the current `taida` binary (or asks the user to do so through the bundled `install.sh`), and then ends the session. The replaced binary is invoked by the user on subsequent `taida run` / `taida build` calls, which can execute arbitrary shell commands through the `taida-lang/os` addon (`execShell`, `run`, writable file I/O — see also the concurrent `C25B-006` security audit for the existing by-design surface). A malicious release therefore grants remote code execution on every machine that has ever run `taida upgrade` against the compromised fork.

### Root cause

The `TAIDA_OWNER` / `TAIDA_REPO` constants in `src/upgrade.rs` were hard-coded to `shijimic` / `taida` during pre-RC development and were never rotated to the canonical organisation before the CLI was released publicly. No second-factor — lockfile hash, published `SHA256SUMS` signed by a distinct key, SLSA provenance, Sigstore attestation, etc. — was present; the HTTPS transport only protected against passive MITM, not against publisher compromise.

The SLSA provenance + Sigstore keyless signing side of this defence is now live for all `@c.26+` releases under `C26B-007` sub-phase 7.4 (SEC-011); the rotation of the release source to `taida-lang/taida` is the complementary fix tracked here.

### Patch

Commit `56c89e0` (and the accompanying `b2fb2e5`) on PR #30 of `taida-lang/taida`:

- Rename the constants from `shijimic` / `taida` to `taida-lang` / `taida`.
- Pin that value in a regression test (`src/upgrade.rs::canonical_release_source_is_taida_lang_org`) so every future edit has to pass through a compiler failure and an explicit reviewer acknowledgement.
- Correct stale references in `docs/reference/cli.md` and in the scaffold documentation comments inside `src/pkg/init.rs`.

The patch is already present and verified in every `@c.15.rc3+` build. Once on a patched CLI, `taida upgrade` discovers and trusts releases from the canonical org as expected.

### Migration / exploitation window / mitigation

Users still running `@c.13.rc3` or `@c.14.rc3` **cannot self-upgrade out of the vulnerable window** — those CLIs only see releases on the personal fork, and the canonical org never publishes there. Affected users must either:

1. Reinstall through `install.sh` (which pulls from `taida-lang/taida` directly), or
2. Download a `@c.15.rc3+` archive from <https://github.com/taida-lang/taida/releases> by hand and install it over the existing `taida` binary.

Until the user has installed `@c.15.rc3+`, mitigations are:

- Do not run `taida upgrade` from a pre-patched CLI.
- If the personal fork (`shijimic/taida`) ever serves a release, do not trust it; verify that the archive content is identical to the canonical org's release of the same version before running.
- Treat any `taida` binary whose provenance you cannot trace to `github.com/taida-lang/taida/releases` as suspect.

There is **no evidence** that the personal fork was compromised during the vulnerable window. The advisory is published out of disclosure hygiene and to give downstream install-base tooling (`install.sh`, third-party package mirrors, org-wide inventory scanners) an auditable reference.

### Credit

Discovered during post-release supply-chain review by the Taida Lang core team (self-reported).

## Fixed commit(s)

- `56c89e0` (`taida-lang/taida` PR #30) — rotate constants, add regression test, update scaffold docs.
- `b2fb2e5` (`taida-lang/taida` PR #30) — follow-up documentation pass.

## References

- CHANGELOG entry: [`CHANGELOG.md @c.15.rc3 → Security`](https://github.com/taida-lang/taida/blob/main/CHANGELOG.md)
- Regression test: [`src/upgrade.rs::canonical_release_source_is_taida_lang_org`](https://github.com/taida-lang/taida/blob/main/src/upgrade.rs)
- Stable policy: [`docs/STABILITY.md`](https://github.com/taida-lang/taida/blob/main/docs/STABILITY.md) §5 (security)
- C26 blocker tracker (private): `.dev/C26_BLOCKERS.md::C26B-008`
- C25 blocker tracker (private): `.dev/C25_BLOCKERS.md::C25B-014`
- Related mitigation (post-fix): `C26B-007` sub-phase 7.4 — SLSA provenance + Sigstore keyless signing for `taida publish` (live on `@c.26+`)

---

## Submission checklist (owner action)

- [ ] Draft the advisory on GitHub: <https://github.com/taida-lang/taida/security/advisories/new>.
- [ ] Paste this file's **Title**, **Description**, **Affected versions**, **Patched versions** into the form.
- [ ] Tick **Request CVE ID**.
- [ ] Set **Severity** to `High` (CVSS `AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H ≈ 8.1`).
- [ ] Attach `CWE-494` and `CWE-829`.
- [ ] Set **Ecosystem** to `Other`; in the notes say "Taida Lang CLI — `taida-lang/taida` GitHub releases".
- [ ] Publish.
- [ ] Once the GHSA ID is assigned (`GHSA-xxxx-xxxx-xxxx`):
  - [ ] In `CHANGELOG.md` under `## @c.15.rc3 → Security`, add immediately after the existing paragraph:

    > **Advisory**: `GHSA-xxxx-xxxx-xxxx`
    > (`https://github.com/taida-lang/taida/security/advisories/GHSA-xxxx-xxxx-xxxx`).
    > CVE: `CVE-20xx-xxxxx` *(if assigned)*.

  - [ ] In `docs/advisory/C25B-014-advisory.md` (this file), change the top-of-file status line to:

    > **Status**: PUBLISHED — `GHSA-xxxx-xxxx-xxxx` (`<URL>`) — CVE `CVE-20xx-xxxxx` *(if assigned)*.

    …and delete this submission checklist.
  - [ ] In `.dev/C26_BLOCKERS.md::C26B-008`, flip status to FIXED with the GHSA ID as evidence.
  - [ ] In `.github/SECURITY.md`, link the advisory under the disclosed-issues section.
- [ ] If a CVE is assigned, record it alongside the GHSA ID everywhere the GHSA ID is recorded.
- [ ] Consider a pinned issue on `taida-lang/taida` linking to the advisory for at least one release cycle, so stale `@c.13.rc3` / `@c.14.rc3` installations see it when users visit the repo.
- [ ] Consider adding a banner line to `README.md` until the `@c.26` release ships.
- [ ] Delete `.dev/security_advisories/GHSA-DRAFT-taida-upgrade-supply-chain.md` — the staged copy in this directory is now authoritative.

## Owner-only action notes

The C26 agent track will **not** publish this advisory itself. Publishing it requires the `taida-lang/taida` owner role in GitHub; the agent only has code-level write access to the repo. The draft above is complete enough that submission should take ≤ 5 minutes of manual work.

After publication, annotate the top of this file with the assigned GHSA ID (see submission checklist step 8) and retire the `.dev/` draft.
