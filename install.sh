#!/usr/bin/env bash
# Taida Lang — public installer.
#
# This script is the official bootstrap for the Taida CLI. It runs on
# the user's machine (Linux / macOS) and downloads a prebuilt release
# archive from GitHub. Every downloaded artefact is cryptographically
# verified before it is unpacked:
#
#   1. SHA-256 against the release-signed `SHA256SUMS` file.
#   2. Sigstore cosign signature (C26B-007 Sub-phase 7.4 / SEC-011).
#      The `.cosign.bundle` is fetched alongside the tarball and
#      checked by `scripts/release/verify-signatures.sh` (embedded
#      inline below so this script stays self-contained for the
#      `curl | bash` flow taida.dev recommends).
#
# Exit codes:
#   0    install succeeded
#   1    usage / argument error
#   2    network / download failure
#   3    verification failure (SHA-256 or Sigstore) — DO NOT RUN the
#        downloaded binary if this triggers; the failure is a hard
#        supply-chain signal
#   4    cosign binary missing while TAIDA_VERIFY_SIGNATURES=required
#
# C26B-030 closes the install-side half of SEC-011; the release side
# lives in `.github/workflows/release.yml`'s `sign` + `provenance`
# jobs. Together they prove every official Taida binary was built by
# the `taida-lang/taida` GitHub Actions workflow under a
# transparency-log-recorded OIDC identity.

set -euo pipefail

TAIDA_REPO="${TAIDA_REPO:-taida-lang/taida}"
TAIDA_VERSION="${TAIDA_VERSION:-latest}"
TAIDA_VERIFY_SIGNATURES="${TAIDA_VERIFY_SIGNATURES:-best-effort}"
TAIDA_INSTALL_PREFIX="${TAIDA_INSTALL_PREFIX:-$HOME/.taida}"

usage() {
    cat <<EOF
Taida Lang installer

Usage: $0 [--version <TAG>] [--prefix <DIR>]

Environment variables:
  TAIDA_REPO                 GitHub repository. Default: ${TAIDA_REPO}
  TAIDA_VERSION              Release tag, or 'latest'. Default: latest
  TAIDA_INSTALL_PREFIX       Install root. Default: \$HOME/.taida
  TAIDA_VERIFY_SIGNATURES    Signature policy:
                               off           — skip cosign entirely (not recommended)
                               best-effort   — warn on missing cosign / bundle (default)
                               required      — fail hard on any gap (CI-grade)
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version) TAIDA_VERSION="$2"; shift 2 ;;
        --prefix) TAIDA_INSTALL_PREFIX="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage; exit 1 ;;
    esac
done

# ── detect host triple ─────────────────────────────────────────
os="$(uname -s)"
arch="$(uname -m)"
case "${os}-${arch}" in
    Linux-x86_64)  TRIPLE="x86_64-unknown-linux-gnu" ; EXT="tar.gz" ;;
    Linux-aarch64) TRIPLE="aarch64-unknown-linux-gnu"; EXT="tar.gz" ;;
    Darwin-x86_64) TRIPLE="x86_64-apple-darwin"      ; EXT="tar.gz" ;;
    Darwin-arm64)  TRIPLE="aarch64-apple-darwin"     ; EXT="tar.gz" ;;
    *) echo "unsupported host: ${os}-${arch}" >&2; exit 1 ;;
esac

# ── resolve tag ────────────────────────────────────────────────
if [ "${TAIDA_VERSION}" = "latest" ]; then
    TAIDA_VERSION="$(curl -fsSL "https://api.github.com/repos/${TAIDA_REPO}/releases/latest" \
        | grep -o '"tag_name": *"[^"]*"' | head -1 | cut -d'"' -f4)"
    if [ -z "${TAIDA_VERSION}" ]; then
        echo "cannot resolve latest release tag for ${TAIDA_REPO}" >&2
        exit 2
    fi
fi

echo "Installing Taida ${TAIDA_VERSION} (${TRIPLE}) -> ${TAIDA_INSTALL_PREFIX}"

tmpdir="$(mktemp -d)"
trap 'rm -rf "${tmpdir}"' EXIT

ASSET="taida-${TAIDA_VERSION}-${TRIPLE}.${EXT}"
BASE_URL="https://github.com/${TAIDA_REPO}/releases/download/${TAIDA_VERSION}"
ART_URL="${BASE_URL}/${ASSET}"
BUNDLE_URL="${ART_URL}.cosign.bundle"
SUMS_URL="${BASE_URL}/SHA256SUMS"
SUMS_BUNDLE_URL="${SUMS_URL}.cosign.bundle"

# ── download ──────────────────────────────────────────────────
echo "  downloading ${ASSET}"
curl -fsSL -o "${tmpdir}/${ASSET}" "${ART_URL}" \
    || { echo "artefact download failed: ${ART_URL}" >&2; exit 2; }
echo "  downloading SHA256SUMS"
curl -fsSL -o "${tmpdir}/SHA256SUMS" "${SUMS_URL}" \
    || { echo "SHA256SUMS download failed: ${SUMS_URL}" >&2; exit 2; }

# ── SHA-256 check against release-signed SHA256SUMS ───────────
cd "${tmpdir}"
if ! grep " ${ASSET}\$" SHA256SUMS > sum.line; then
    echo "asset ${ASSET} not listed in SHA256SUMS — refusing install" >&2
    exit 3
fi
if ! sha256sum -c sum.line > /dev/null; then
    echo "SHA-256 mismatch for ${ASSET} — refusing install" >&2
    exit 3
fi
echo "  SHA-256: OK"

# ── Sigstore cosign verification (C26B-007 Sub-phase 7.4 / SEC-011) ──
case "${TAIDA_VERIFY_SIGNATURES}" in
    off|0|false|no) SEC011_MODE="off" ;;
    required|enforce|1) SEC011_MODE="required" ;;
    *) SEC011_MODE="best-effort" ;;
esac

if [ "${SEC011_MODE}" != "off" ]; then
    echo "  downloading ${ASSET}.cosign.bundle"
    curl -fsSL -o "${tmpdir}/${ASSET}.cosign.bundle" "${BUNDLE_URL}" || BUNDLE_MISS=1
    curl -fsSL -o "${tmpdir}/SHA256SUMS.cosign.bundle" "${SUMS_BUNDLE_URL}" || SUMS_BUNDLE_MISS=1

    if [ -n "${BUNDLE_MISS:-}" ] || [ -n "${SUMS_BUNDLE_MISS:-}" ]; then
        if [ "${SEC011_MODE}" = "required" ]; then
            echo "SEC-011 required: cosign bundle missing upstream" >&2
            exit 3
        else
            echo "SEC-011 warn: cosign bundle missing — skipping verify (best-effort)"
        fi
    else
        if ! command -v cosign >/dev/null 2>&1; then
            if [ "${SEC011_MODE}" = "required" ]; then
                cat >&2 <<'NOCOSIGN'
SEC-011 required: cosign binary not on PATH.
Install via `brew install cosign` or download from
https://github.com/sigstore/cosign/releases/latest and re-run this
installer with TAIDA_VERIFY_SIGNATURES=required.
NOCOSIGN
                exit 4
            else
                echo "SEC-011 warn: cosign not installed — skipping verify (best-effort)"
            fi
        else
            if cosign verify-blob \
                    --bundle "${ASSET}.cosign.bundle" \
                    --certificate-identity-regexp "^https://github.com/${TAIDA_REPO}/" \
                    --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
                    "${ASSET}" >/dev/null 2>&1
            then
                echo "  SEC-011: OK (${ASSET})"
            else
                echo "SEC-011 verify: cosign rejected ${ASSET} — aborting install" >&2
                exit 3
            fi
            if cosign verify-blob \
                    --bundle SHA256SUMS.cosign.bundle \
                    --certificate-identity-regexp "^https://github.com/${TAIDA_REPO}/" \
                    --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
                    SHA256SUMS >/dev/null 2>&1
            then
                echo "  SEC-011: OK (SHA256SUMS)"
            else
                echo "SEC-011 verify: cosign rejected SHA256SUMS — aborting install" >&2
                exit 3
            fi
        fi
    fi
fi

# ── install ───────────────────────────────────────────────────
mkdir -p "${TAIDA_INSTALL_PREFIX}/bin"
tar -xzf "${ASSET}" -C "${TAIDA_INSTALL_PREFIX}/bin" --strip-components=1

echo "Installed Taida ${TAIDA_VERSION} to ${TAIDA_INSTALL_PREFIX}/bin/"
echo "Add to PATH:  export PATH=\"${TAIDA_INSTALL_PREFIX}/bin:\$PATH\""
