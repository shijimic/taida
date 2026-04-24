# scripts/release/

Release automation for the Taida main repository. Invoked by
`.github/workflows/release.yml` and manual release operators.

## Files

| File | Purpose |
|------|---------|
| `secret-scan.sh` | Pre-release gate: scans the working tree for secrets. Invoked from the release gate job. |
| `package-unix.sh` | Packages the Unix release tarball (`taida-<tag>-<triple>.tar.gz`). |
| `package-windows.ps1` | Packages the Windows release ZIP (`taida-<tag>-<triple>.zip`). |
| `verify-signatures.sh` | **(C26B-007 Sub-phase 7.4 / SEC-011)** verifies a downloaded release artefact against its Sigstore cosign bundle. Consumed by `install.sh` (the root-level public installer, which inlines the same cosign invocation so the `curl \| bash` flow stays self-contained) and by `taida install` (through `src/addon/signature_verify.rs` — C26B-030). Both callers default to the `best-effort` policy with `TAIDA_VERIFY_SIGNATURES=required` to fail closed. |

## SEC-011 — Sigstore + SLSA (C26B-007 Sub-phase 7.4)

All official release artefacts are signed via [Sigstore](https://sigstore.dev/)
keyless cosign + accompanied by an [SLSA Level 3](https://slsa.dev/) provenance
document. No long-lived signing key exists; trust flows from:

1. GitHub Actions' OIDC token (short-lived, scoped to the workflow run).
2. The Rekor transparency log (tamper-evident, public).
3. `cosign verify-blob` against the `.cosign.bundle` file shipped
   alongside each release asset.

### What the release workflow produces

For every release tag `@<gen>.<num>[.<label>]` (enforced by the
`prepare` job in `release.yml`), each platform triple yields:

```
taida-<TAG>-<TRIPLE>.tar.gz          # or .zip on Windows
taida-<TAG>-<TRIPLE>.tar.gz.cosign.bundle
```

Plus these per-release files:

```
SHA256SUMS
SHA256SUMS.cosign.bundle
taida-<TAG>.intoto.jsonl             # SLSA Level 3 provenance
```

### Verifying a download

```bash
# 1. Download the binary + its bundle from the release page.
curl -LO https://github.com/taida-lang/taida/releases/download/<TAG>/taida-<TAG>-<TRIPLE>.tar.gz
curl -LO https://github.com/taida-lang/taida/releases/download/<TAG>/taida-<TAG>-<TRIPLE>.tar.gz.cosign.bundle

# 2. Run the verifier.
./scripts/release/verify-signatures.sh \
    taida-<TAG>-<TRIPLE>.tar.gz \
    --repo taida-lang/taida \
    --tag <TAG>
```

The script exits `0` iff the signature was produced by a GitHub
Actions run in `taida-lang/taida` against a commit reachable from
the release tag. Any other outcome is a hard fail — do not trust
the artefact.

### Verifying SLSA provenance

```bash
# Requires slsa-verifier (https://github.com/slsa-framework/slsa-verifier).
slsa-verifier verify-artifact \
    --provenance-path taida-<TAG>.intoto.jsonl \
    --source-uri github.com/taida-lang/taida \
    --source-tag <TAG> \
    taida-<TAG>-<TRIPLE>.tar.gz
```

### Key security properties

- **No bespoke signing key.** Losing / rotating a private key is
  impossible because none exists. Compromising the pipeline would
  require hijacking the GitHub OIDC flow for `taida-lang/taida`.
- **Transparent.** Every signature is recorded in Rekor; a silent
  compromise would have to alter the public transparency log.
- **Defense in depth.** `SHA256SUMS` is itself signed, so a tampered
  checksum file does not slip by even if a reader forgets to verify
  an individual tarball.
- **Hermetic build.** SLSA L3 provenance is produced by the
  `slsa-github-generator` reusable workflow, which runs in an
  isolated GitHub-hosted runner and records the full build recipe.

### Accepted limitations

- Users who skip `verify-signatures.sh` and rely only on `SHA256SUMS`
  are still safer than pre-C26, but lose the transparency-log trail.
  `install.sh` will call the verifier by default; opt-outs require an
  explicit flag.
- The verifier depends on `cosign` being installed locally. If it
  is missing, the script prints an install hint and exits `2`.
