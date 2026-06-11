#!/usr/bin/env bash
set -euo pipefail

# The scan is only as good as its scanner: a missing ripgrep makes every
# `if rg ...` condition falsy (exit 127 is not caught by set -e inside a
# condition) and the gate silently passes. Fail loudly instead.
if ! command -v rg >/dev/null 2>&1; then
  echo "secret-scan: ripgrep (rg) is not installed; refusing to pass an unscanned tree" >&2
  exit 1
fi

patterns=(
  'ghp_[A-Za-z0-9]{20,}'
  'github_pat_[A-Za-z0-9_]{20,}'
  'AKIA[0-9A-Z]{16}'
  'AIza[0-9A-Za-z_-]{20,}'
  'xox[baprs]-[0-9A-Za-z-]{10,}'
  '-----BEGIN (RSA|EC|OPENSSH|DSA|PGP) PRIVATE KEY-----'
)

for pattern in "${patterns[@]}"; do
  if rg -n --hidden --glob '!.git' --glob '!target' --glob '!dist' --regexp "${pattern}" .; then
    echo "secret-like token detected by pattern: ${pattern}" >&2
    exit 1
  fi
done

echo "secret scan passed"
