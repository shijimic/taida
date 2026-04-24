#!/usr/bin/env bash
# publish-advisory.sh — owner helper for staging a GitHub Security Advisory submission.
#
# PURPOSE
#   Read a staged advisory Markdown file from `docs/advisory/`, extract the
#   GHSA fields from it, and print the exact `gh api` call that the owner
#   would run to submit it through the REST API.
#
#   This script is DELIBERATELY not a one-shot publisher. Advisory publication
#   is a one-way action (the GHSA ID and public URL are not rescindable
#   without owner intervention), so the tool only PRINTS the command. The
#   owner copy-pastes and runs it from a trusted, authenticated shell.
#
# USAGE
#   scripts/advisory/publish-advisory.sh --advisory docs/advisory/C25B-014-advisory.md [--dry-run]
#   scripts/advisory/publish-advisory.sh --advisory docs/advisory/C25B-014-advisory.md --print-body
#
#   --dry-run      (default) print the gh command and the JSON body that
#                  would be POSTed. Do not call gh.
#   --print-body   print only the JSON body and exit (useful for diffing).
#   --advisory     path to a staged Markdown advisory file.
#
# IMPORTANT
#   This script NEVER invokes `gh api` on its own. It exits with a reminder
#   that publication requires the owner to run the printed command by hand.
#
#   Agents are forbidden from publishing advisories. See
#   `docs/advisory/README.md` for the owner-action boundary.
#
# EXIT CODES
#   0  — dry-run output printed successfully (no advisory was submitted)
#   1  — usage error / missing advisory file
#   2  — refused to run: this script never publishes directly

set -euo pipefail

print_usage() {
  cat <<'EOF'
usage: publish-advisory.sh --advisory <path> [--dry-run|--print-body]

  --advisory PATH   Path to a staged Markdown advisory file under
                    docs/advisory/ (e.g. docs/advisory/C25B-014-advisory.md).
  --dry-run         Print the gh api command that the owner should run.
                    Default if no mode flag is given.
  --print-body      Print only the JSON body that would be POSTed.
  -h, --help        Show this help message.

This tool is a DRY-RUN ONLY helper. It never publishes advisories.
See docs/advisory/README.md for the owner publication procedure.
EOF
}

ADVISORY_PATH=""
MODE="dry-run"

while [ "$#" -gt 0 ]; do
  case "$1" in
    --advisory)
      ADVISORY_PATH="${2:-}"
      shift 2
      ;;
    --dry-run)
      MODE="dry-run"
      shift
      ;;
    --print-body)
      MODE="print-body"
      shift
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "error: unknown argument '$1'" >&2
      print_usage >&2
      exit 1
      ;;
  esac
done

if [ -z "$ADVISORY_PATH" ]; then
  echo "error: --advisory is required" >&2
  print_usage >&2
  exit 1
fi

if [ ! -f "$ADVISORY_PATH" ]; then
  echo "error: advisory file not found: $ADVISORY_PATH" >&2
  exit 1
fi

# Field extraction is deliberately simple. The staged advisory is Markdown,
# not a structured format, so we take a best-effort pass at pulling the
# canonical fields. The owner ALWAYS reviews the output before running.
extract_h2() {
  # extract_h2 <header> <path>
  # Prints the body of the first `## <header>` section up to the next
  # `##` or EOF. Trims leading/trailing blank lines.
  local header="$1"
  local path="$2"
  awk -v h="## $header" '
    $0 == h       { inside=1; next }
    /^## /        { if (inside) exit }
    inside        { print }
  ' "$path" | sed -e '/./,$!d' | sed -e ':a' -e '/^$/{$d;N;ba' -e '}'
}

TITLE="$(extract_h2 "Title (≤ 100 chars)" "$ADVISORY_PATH" | tail -n +1 | head -n 1)"
DESCRIPTION="$(extract_h2 "Description" "$ADVISORY_PATH")"
SEVERITY_BLOCK="$(extract_h2 "Severity (self-assessed)" "$ADVISORY_PATH")"
AFFECTED_VERSIONS="$(extract_h2 "Affected versions" "$ADVISORY_PATH")"
PATCHED_VERSIONS="$(extract_h2 "Patched versions" "$ADVISORY_PATH")"
CWES="$(extract_h2 "CWE(s)" "$ADVISORY_PATH")"

# Best-effort CVSS extraction from the severity block.
CVSS_VECTOR="$(printf '%s' "$SEVERITY_BLOCK" | grep -oE 'AV:[A-Z]/AC:[A-Z]/PR:[A-Z]/UI:[A-Z]/S:[A-Z]/C:[A-Z]/I:[A-Z]/A:[A-Z]' | head -n 1 || true)"

# Owner reviews the body; this script only renders it. We emit a lightly
# structured JSON skeleton that mirrors the fields GHSA accepts via REST.
# The owner is expected to edit the rendered body before submitting — this
# is a STAGING tool, not a submission tool.
BODY="$(python3 - "$ADVISORY_PATH" "$TITLE" "$CVSS_VECTOR" "$AFFECTED_VERSIONS" "$PATCHED_VERSIONS" "$CWES" "$DESCRIPTION" <<'PY'
import json
import sys

(
    _path,
    title,
    cvss_vector,
    affected_versions,
    patched_versions,
    cwes,
    description,
) = sys.argv[1:]

payload = {
    "summary": title.strip() or "(missing — edit before submission)",
    "description": description.strip()
        or "(missing — edit before submission)",
    "severity": "high",
    "cvss_vector_string": cvss_vector.strip() or "(edit before submission)",
    "cwe_ids": [line.strip().split(":")[0].strip("-* ")
                for line in cwes.splitlines()
                if line.strip().startswith(("-", "*"))
                and "CWE-" in line],
    "vulnerabilities": [
        {
            "package": {
                "ecosystem": "other",
                "name": "taida",
            },
            "vulnerable_version_range": affected_versions.strip()
                or "(edit before submission)",
            "patched_versions": patched_versions.strip()
                or "(edit before submission)",
        }
    ],
    "credits": [
        {"login": "taida-lang", "type": "reporter"},
    ],
    "request_cve": True,
}
print(json.dumps(payload, indent=2, ensure_ascii=False))
PY
)"

if [ "$MODE" = "print-body" ]; then
  printf '%s\n' "$BODY"
  exit 0
fi

# Default: --dry-run — print the gh api command the owner should run.
cat <<EOF
# publish-advisory.sh — DRY RUN
#
# The following is the gh api command that would submit the advisory at:
#   $ADVISORY_PATH
#
# This script does NOT run it. Publication is an owner action. Review the
# JSON body below, edit as needed, then run the command by hand from a
# trusted authenticated shell. See docs/advisory/README.md.

cat <<'JSON' > /tmp/advisory-body.json
$BODY
JSON

gh api \\
  -X POST \\
  -H "Accept: application/vnd.github+json" \\
  repos/taida-lang/taida/security-advisories \\
  --input /tmp/advisory-body.json

# After the GHSA ID is assigned, follow the post-publication checklist in
# docs/advisory/README.md (§ "Publication procedure", step 4).
EOF

exit 0
