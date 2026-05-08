#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd -P)"

if ! command -v node >/dev/null 2>&1; then
  echo "error: node is required for backend parity tests" >&2
  exit 1
fi

if ! command -v cc >/dev/null 2>&1; then
  echo "error: cc is required for native backend parity tests" >&2
  exit 1
fi

if [ -z "${TAIDA_BIN:-}" ] \
  && [ ! -x "$PROJECT_DIR/target/release/taida" ] \
  && [ ! -x "$PROJECT_DIR/target/debug/taida" ]; then
  echo "[backend-parity] building taida binary"
  cargo build --bin taida
fi

# Export a single resolved binary path for the Rust test crates that shell out
# through tests/common::taida_bin().
source "$SCRIPT_DIR/scripts/lib_taida_bin.sh"
TAIDA_BIN="$(resolve_taida_bin)" || exit 1
export TAIDA_BIN

echo "[backend-parity] running native compile parity tests"
cargo test --test native_compile -- --nocapture

echo "[backend-parity] running crash regression corpus"
cargo test --test crash_regression -- --nocapture

echo "[backend-parity] running three-way parity tests"
cargo test --test parity -- --nocapture
