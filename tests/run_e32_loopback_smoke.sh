#!/usr/bin/env bash
# E32 loopback smoke for descriptor-built native server + static route asset.
#
# This is intentionally outside the default cargo test suite. It binds
# 127.0.0.1:0, reads the announced port, performs one HTTP request, and
# compares the response body hash to the committed AssetBundle bytes.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/taida-e32-loopback.XXXXXX")"
ARTIFACT_DIR="${E32_LOOPBACK_ARTIFACT_DIR:-}"
SERVER_LOG="$WORK_DIR/server.log"
CLIENT_TRACE="$WORK_DIR/client.trace"

if [ -n "$ARTIFACT_DIR" ]; then
  mkdir -p "$ARTIFACT_DIR"
fi

cleanup() {
  status=$?
  if [ -n "${SERVER_PID:-}" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [ -n "$ARTIFACT_DIR" ]; then
    cp "$SERVER_LOG" "$ARTIFACT_DIR/server.log" 2>/dev/null || true
    cp "$CLIENT_TRACE" "$ARTIFACT_DIR/client.trace" 2>/dev/null || true
  fi
  rm -rf "$WORK_DIR"
  exit "$status"
}
trap cleanup EXIT

if [ -n "${TAIDA_BIN:-}" ]; then
  TAIDA="$TAIDA_BIN"
elif [ -x "$PROJECT_DIR/target/debug/taida" ]; then
  TAIDA="$PROJECT_DIR/target/debug/taida"
elif [ -x "$PROJECT_DIR/target/release/taida" ]; then
  TAIDA="$PROJECT_DIR/target/release/taida"
else
  echo "TAIDA_BIN is not set and no built taida binary exists" >&2
  exit 1
fi

cat >"$WORK_DIR/packages.tdm" <<'PKG'
PKG

mkdir -p "$WORK_DIR/public"
cat >"$WORK_DIR/public/payload.txt" <<'PAYLOAD'
e32-loopback-payload
PAYLOAD

cat >"$WORK_DIR/server.td" <<'TD'
>>> taida-lang/net => @(httpServe)
>>> taida-lang/os => @(Read)

handler req =
  body <= Read[".taida/build/assets/frontend/payload.txt"]().getOrDefault("")
  @(status <= 200, headers <= @[@(name <= "content-type", value <= "text/plain")], body <= body)
=> :@(status: Int, headers: @[@(name: Str, value: Str)], body: Str)

asyncResult <= httpServe(0, handler, 1)
asyncResult ]=> result
result ]=> r
stdout(r.requests.toString())
TD

cat >"$WORK_DIR/main.td" <<'TD'
>>> ./server.td => @(serverMain)

frontendAssets <= AssetBundle(
  name <= "frontend-assets",
  root <= "public",
  files <= @["**/*"],
  output <= "assets/frontend"
)

serverX <= BuildUnit(
  name <= "server-x",
  target <= "native",
  entry <= serverMain,
  assets <= @[RouteAsset(path <= "/", asset <= frontendAssets)]
)

<<< serverX
TD

(cd "$WORK_DIR" && "$TAIDA" build main.td --unit server-x)

(cd "$WORK_DIR" && TAIDA_NET_ANNOUNCE_PORT=1 ".taida/build/native/server-x/server-x") \
  >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

deadline=$((SECONDS + 5))
port=""
while [ "$SECONDS" -lt "$deadline" ]; do
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "server exited before announcing port" >&2
    cat "$SERVER_LOG" >&2 || true
    exit 1
  fi
  port="$(sed -n 's/^listening on 127\.0\.0\.1:\([0-9][0-9]*\)$/\1/p' "$SERVER_LOG" | tail -n 1)"
  if [ -n "$port" ]; then
    break
  fi
  sleep 0.1
done

if [ -z "$port" ]; then
  echo "server did not announce a loopback port within 5s" >&2
  cat "$SERVER_LOG" >&2 || true
  exit 1
fi

curl --max-time 10 --fail --silent --show-error "http://127.0.0.1:${port}/" \
  -o "$WORK_DIR/body.out" 2>"$CLIENT_TRACE"

expected="$(sha256sum "$WORK_DIR/.taida/build/assets/frontend/payload.txt" | awk '{print $1}')"
actual="$(sha256sum "$WORK_DIR/body.out" | awk '{print $1}')"
if [ "$expected" != "$actual" ]; then
  echo "response SHA-256 mismatch: expected=$expected actual=$actual" >&2
  exit 1
fi

wait "$SERVER_PID"
