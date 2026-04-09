#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────
# RC2.6 E2E gate: verify interpreter <-> native byte parity
# for taida-lang/terminal addon dispatch.
#
# Usage:
#   tests/e2e_rc26_gate.sh [PROJECT_DIR]
#
# Exit codes:
#   0 — PASS (interpreter and native outputs are byte-identical)
#   1 — FAIL (outputs differ, or a prerequisite is missing)
#
# Prerequisites (the script checks each and exits with a message
# if any is missing):
#   - taida binary built: target/debug/taida (or set TAIDA_BIN)
#   - terminal cdylib built: ../terminal/target/{debug,release}/libtaida_lang_terminal.so
#   - terminal facade: ../terminal/taida/terminal.td
#   - terminal addon.toml: ../terminal/native/addon.toml
#
# The script sets up a manual fixture under $PROJECT/.taida/deps/
# because `taida install` cannot yet fetch from a GitHub Release
# (no release has been published). Once `taida publish --target
# rust-addon` + `taida install` are wired end-to-end, this manual
# fixture setup can be replaced by a single `taida install`.
# ─────────────────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ── Resolve taida binary ────────────────────────────────────────
TAIDA="${TAIDA_BIN:-}"
if [ -z "$TAIDA" ]; then
    for profile in debug release; do
        candidate="${REPO_ROOT}/target/${profile}/taida"
        if [ -x "$candidate" ]; then
            TAIDA="$candidate"
            break
        fi
    done
fi
if [ -z "$TAIDA" ] || [ ! -x "$TAIDA" ]; then
    echo "SKIP: taida binary not found. Build with 'cargo build' or set TAIDA_BIN." >&2
    exit 1
fi
echo "taida binary: ${TAIDA}"

# ── Resolve project directory ───────────────────────────────────
PROJECT="${1:-${REPO_ROOT}/../e2e-demo-upstream}"
PROJECT="$(cd "$PROJECT" && pwd)"
if [ ! -f "${PROJECT}/main.td" ]; then
    echo "FAIL: ${PROJECT}/main.td not found." >&2
    exit 1
fi
if [ ! -f "${PROJECT}/packages.tdm" ]; then
    echo "FAIL: ${PROJECT}/packages.tdm not found." >&2
    exit 1
fi
echo "project dir:  ${PROJECT}"

# ── Resolve terminal artifacts ──────────────────────────────────
TERMINAL_ROOT="${REPO_ROOT}/../terminal"
if [ ! -d "$TERMINAL_ROOT" ]; then
    echo "SKIP: ../terminal checkout not found at ${TERMINAL_ROOT}." >&2
    exit 1
fi

CDYLIB=""
for profile in debug release; do
    candidate="${TERMINAL_ROOT}/target/${profile}/libtaida_lang_terminal.so"
    if [ -f "$candidate" ]; then
        CDYLIB="$candidate"
        break
    fi
done
if [ -z "$CDYLIB" ]; then
    echo "SKIP: libtaida_lang_terminal.so not found under ${TERMINAL_ROOT}/target/{debug,release}/." >&2
    exit 1
fi
echo "cdylib:       ${CDYLIB}"

FACADE="${TERMINAL_ROOT}/taida/terminal.td"
if [ ! -f "$FACADE" ]; then
    echo "FAIL: facade source not found at ${FACADE}." >&2
    exit 1
fi

ADDON_TOML="${TERMINAL_ROOT}/native/addon.toml"
if [ ! -f "$ADDON_TOML" ]; then
    echo "FAIL: addon.toml not found at ${ADDON_TOML}." >&2
    exit 1
fi

# ── Set up manual fixture ───────────────────────────────────────
DEPS="${PROJECT}/.taida/deps/taida-lang/terminal"
echo "fixture:      ${DEPS}"

rm -rf "${PROJECT}/.taida"
mkdir -p "${DEPS}/native" "${DEPS}/taida"

cp "$ADDON_TOML" "${DEPS}/native/addon.toml"
cp "$FACADE"     "${DEPS}/taida/terminal.td"
cp "$CDYLIB"     "${DEPS}/native/libtaida_lang_terminal.so"

# Write a packages.tdm for the dep (taida resolver expects it).
cat > "${DEPS}/packages.tdm" <<'MANIFEST'
name <= "taida-lang/terminal"
<<<@a.1
MANIFEST

echo "fixture setup complete."

# ── Run interpreter ─────────────────────────────────────────────
echo ""
echo "=== Interpreter run ==="
INTERP_OUT=$(mktemp)
if ! "$TAIDA" "${PROJECT}/main.td" < /dev/null > "$INTERP_OUT" 2>&1; then
    echo "FAIL: interpreter run exited with non-zero status." >&2
    echo "--- output ---"
    cat "$INTERP_OUT" >&2
    rm -f "$INTERP_OUT"
    exit 1
fi
echo "interpreter stdout:"
cat "$INTERP_OUT"

# ── Build + run native ──────────────────────────────────────────
echo ""
echo "=== Native build ==="
NATIVE_BIN="${PROJECT}/main.bin"
rm -f "$NATIVE_BIN"
if ! "$TAIDA" build --target native "${PROJECT}/main.td" -o "$NATIVE_BIN" 2>&1; then
    echo "FAIL: native build failed." >&2
    exit 1
fi
if [ ! -x "$NATIVE_BIN" ]; then
    echo "FAIL: native binary was not produced at ${NATIVE_BIN}." >&2
    exit 1
fi

echo ""
echo "=== Native run ==="
NATIVE_OUT=$(mktemp)
if ! "$NATIVE_BIN" < /dev/null > "$NATIVE_OUT" 2>&1; then
    echo "FAIL: native binary exited with non-zero status." >&2
    echo "--- output ---"
    cat "$NATIVE_OUT" >&2
    rm -f "$INTERP_OUT" "$NATIVE_OUT"
    exit 1
fi
echo "native stdout:"
cat "$NATIVE_OUT"

# ── Byte parity check ──────────────────────────────────────────
#
# RC2.6B-011 (Nice to Have, Cosmetic): the interpreter qualifies
# addon function names as 'pkg::fn' while the native backend uses
# the bare function name. We normalise this known difference before
# comparing so the gate is not blocked by cosmetics.
normalise() {
    sed -e "s/'taida-lang\/terminal::terminalSize'/'terminalSize'/g" \
        -e "s/'taida-lang\/terminal::readKey'/'readKey'/g" \
        "$1"
}

echo ""
echo "=== Parity check ==="

INTERP_NORM=$(mktemp)
NATIVE_NORM=$(mktemp)
normalise "$INTERP_OUT" > "$INTERP_NORM"
normalise "$NATIVE_OUT" > "$NATIVE_NORM"

if diff "$INTERP_NORM" "$NATIVE_NORM" > /dev/null 2>&1; then
    echo "PASS: interpreter and native outputs are byte-identical (after RC2.6B-011 normalisation)."
    if ! diff "$INTERP_OUT" "$NATIVE_OUT" > /dev/null 2>&1; then
        echo "note: RC2.6B-011 cosmetic parity gap detected (function name qualification)."
        echo "      This is a known Nice to Have item, not a gate blocker."
    fi
    RESULT=0
else
    echo "FAIL: outputs differ (even after RC2.6B-011 normalisation)."
    echo "--- diff (normalised) ---"
    diff "$INTERP_NORM" "$NATIVE_NORM" || true
    echo "--- diff (raw) ---"
    diff "$INTERP_OUT" "$NATIVE_OUT" || true
    RESULT=1
fi

rm -f "$INTERP_NORM" "$NATIVE_NORM"

# ── Cleanup ─────────────────────────────────────────────────────
rm -f "$INTERP_OUT" "$NATIVE_OUT" "$NATIVE_BIN"

exit $RESULT
