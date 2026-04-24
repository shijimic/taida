# Security Advisory staging

This directory holds **publication-ready** GitHub Security Advisory
(GHSA) drafts for the `taida-lang/taida` repository. Each advisory is
a standalone Markdown file whose body maps 1:1 onto the fields of the
[GHSA submission form](https://github.com/taida-lang/taida/security/advisories/new).

## Scope & intent

- This is a **staging area for the owner**, not a build artefact.
  Nothing in this directory is consumed by `taida build` or
  `taida install`.
- Files here are checked into the public repo so that:
  1. The draft survives local workstation churn and tracks through PR
     review before it goes live.
  2. Anyone auditing the repo can see advisories that are in flight
     (as drafts) alongside advisories that have been published.
- Actual publication — creating the GHSA, requesting a CVE, and
  updating the CHANGELOG cross-reference — is an **owner action**.
  Agents are explicitly forbidden from publishing advisories; the
  agent's job ends at staging the content of this directory.

## Current advisories

| File | Subject | Status | Blocker |
|------|---------|--------|---------|
| `C25B-014-advisory.md` | `taida upgrade` trusted a personal-fork GitHub account as release source before `@c.15.rc3`. | **Draft — not yet published.** | `C26B-008` (`.dev/C26_BLOCKERS.md`) |

The draft is the finalised form of the older working copy at
`.dev/security_advisories/GHSA-DRAFT-taida-upgrade-supply-chain.md`
(checked into the private `.dev/` tree). Once the GHSA ID is assigned
by GitHub, the staged copy here is annotated with the ID and URL, and
the `.dev/` draft is retired.

## Publication procedure (owner action)

The owner performs the following steps from a **trusted, signed-in
browser session** for the `taida-lang` GitHub organisation. No
automation is permitted to perform step 2 or step 3.

1. **Open the staged draft.** For `C25B-014`, that is this file's
   sibling `C25B-014-advisory.md`. Read it end to end.

2. **Create the GHSA through the GitHub UI.** Navigate to
   <https://github.com/taida-lang/taida/security/advisories/new>
   and copy the advisory fields from the staged draft. The draft is
   organised so that every section maps directly onto the form:

   | Form field | Source section in the staged draft |
   |-----------|------------------------------------|
   | Title | `## Title` |
   | Severity | `## Severity` |
   | CVSS vector | `## Severity` (the vector is inside the block) |
   | Affected product | `## Affected product` |
   | Affected versions | `## Affected versions` |
   | Patched versions | `## Patched versions` |
   | Ecosystem | `## Affected ecosystem` |
   | CWEs | `## CWE(s)` |
   | Description | `## Description` |
   | Fixed commit(s) | `## Fixed commit(s)` |
   | References | `## References` |

   Tick **Request CVE ID** if the draft's severity section says so.

3. **Alternative: submit through `gh api`.** A scripted helper is
   provided at `scripts/advisory/publish-advisory.sh`. It wraps
   `gh api` calls with a `--dry-run` mode so the owner can diff the
   submission body locally before going live. Read the helper's own
   header before running it. The helper **never** publishes on its
   own: it prints the `gh api` command that would be run and requires
   the owner to invoke it explicitly. (This is intentional — advisory
   publication is a one-way action and must not be wrapped in
   automation.)

4. **Record the assigned GHSA ID.** Once GitHub accepts the submission
   it assigns an ID of the form `GHSA-xxxx-xxxx-xxxx`. Record it in:

   1. `CHANGELOG.md` under the affected version's `Security` section
      (e.g. `@c.15.rc3 → Security` for `C25B-014-advisory.md`). Add
      both the GHSA URL and, if assigned, the CVE ID.
   2. `docs/advisory/C25B-014-advisory.md` — annotate the top of the
      file with `Status: PUBLISHED — GHSA-xxxx-xxxx-xxxx
      (<URL>)` and delete the submission checklist.
   3. `.dev/C26_BLOCKERS.md::C26B-008` — flip status to FIXED and
      cross-reference the GHSA ID.
   4. `.github/SECURITY.md` — add a line under the disclosed-issues
      section linking to the published advisory.

5. **Retire the older `.dev/` draft.** After the GHSA is live and the
   cross-references above are in place, delete
   `.dev/security_advisories/GHSA-DRAFT-taida-upgrade-supply-chain.md`.
   The `docs/advisory/` copy is now authoritative.

6. **Optional: pinned issue + README banner.** For one release cycle,
   consider pinning an issue on `taida-lang/taida` linking to the
   advisory, and adding a banner line to `README.md` to catch stale
   `@c.13.rc3` / `@c.14.rc3` installations. Remove both once
   `@c.26` ships.

## Why the `docs/advisory/` directory exists

Before Round 6 of the C26 track, the advisory draft was only present
under `.dev/security_advisories/`. Because `.dev/` is in `.gitignore`,
the draft was not visible to anyone outside the maintainer's
workstation, which:

- made it hard to review the advisory text in a PR,
- hid the existence of the draft from downstream auditors who scan
  public repos for in-flight disclosures, and
- required the owner to dig through `.dev/` to find the submission
  material at publication time.

Staging the publication-ready copy at `docs/advisory/` fixes all
three while preserving the strict owner-only boundary at step 2.

## Agent boundary

Agents working in this repository may:

- create or edit files under `docs/advisory/` when staging a new
  advisory body or updating a draft,
- create or edit files under `scripts/advisory/` when adding dry-run
  helpers,
- update cross-references in `CHANGELOG.md`, `docs/STABILITY.md`,
  `.dev/C26_BLOCKERS.md`, and `.github/SECURITY.md` to match an
  advisory's state.

Agents must **not**:

- call `gh api` or any other tool that creates, publishes, or
  modifies a GitHub Security Advisory,
- submit CVE requests,
- send disclosure email to `security@github.com` or equivalent
  channels,
- push tags, branches, or commits to any remote (this is enforced
  by the broader C26 worktree contract, but is repeated here because
  advisory workflow is particularly sensitive).

The owner is the only actor permitted to perform publication.
