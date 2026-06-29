#!/usr/bin/env bash
#
# Phase 7 — prove the MCP tool layer forwards to the daemon.
#
# Exercises the four MCP tools (share/search/get/sync_status) by calling them directly against a
# running daemon — the same code paths Claude hits, without needing a full Claude session. Also
# checks the daemon-down path returns a clean message, not a traceback.
#
# Usage:  ./scripts/test-mcp.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DAEMON="$ROOT/daemon"
MCP="$ROOT/mcp"
PY="$MCP/.venv/bin/python"
BIN="$DAEMON/target/debug/learnings-daemon"

D=/tmp/lm-data
IROH_PORT=11831
API_PORT=7780

# shellcheck disable=SC1091
. "$HOME/.cargo/env" 2>/dev/null || true

echo "Building daemon (if needed)..."
(cd "$DAEMON" && cargo build --quiet)

echo "1) Daemon-down path (no daemon running)..."
LEARNINGS_API="http://127.0.0.1:59999" "$PY" - <<'PY'
import learnings_mcp.server as s
try:
    s.sync_status()
    print("   DOWN_FAIL — expected an error but got a result"); raise SystemExit(1)
except s.DaemonError as e:
    assert "not reachable" in str(e), str(e)
    print("   ok — clean error:", str(e)[:60], "...")
PY

echo "2) Start the daemon (pair create, then serve on :$API_PORT)..."
rm -rf "$D"; mkdir -p "$D"
"$BIN" --data-dir "$D" --port "$IROH_PORT" pair create >/dev/null 2>&1
"$BIN" --data-dir "$D" --port "$IROH_PORT" serve --api-port "$API_PORT" >/tmp/lm-daemon.log 2>&1 &
DPID=$!
for _ in $(seq 1 40); do curl -sf "localhost:$API_PORT/status" >/dev/null 2>&1 && break; sleep 0.5; done

echo "3) Round-trip through the MCP tools..."
LEARNINGS_API="http://127.0.0.1:$API_PORT" LEARNINGS_AUTHOR="tester" "$PY" - <<'PY'
import learnings_mcp.server as s

created = s.share_learning("MCP roundtrip test", "Filed via the MCP tool layer.", ["mcp", "test"])
assert created["title"] == "MCP roundtrip test", created
assert created["author"] == "tester", created
lid = created["id"]
print("   share_learning  -> id", lid[:12], "author", created["author"])

hits = s.search_learnings("roundtrip")
assert any(h["id"] == lid for h in hits), hits
print("   search_learnings-> found", len(hits), "match(es)")

one = s.get_learning(lid)
assert one["id"] == lid, one
print("   get_learning    -> ok")

missing = s.get_learning("deadbeefdeadbeef")
assert missing.get("found") is False, missing
print("   get_learning(404)-> clean not-found result")

st = s.sync_status()
assert st["learnings"] >= 1, st
print("   sync_status     ->", st)

print("PY_OK")
PY
STATUS=$?

kill "$DPID" 2>/dev/null || true
wait "$DPID" 2>/dev/null || true

if [ "$STATUS" -ne 0 ]; then
  echo "RESULT: FAILED — a tool round-trip assertion failed."
  exit 1
fi

echo
echo "RESULT: PASS — all four MCP tools forward to the daemon correctly."
