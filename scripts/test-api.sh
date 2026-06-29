#!/usr/bin/env bash
#
# Phase 6 — prove the localhost HTTP API, end to end across two nodes.
#
# Two simulated machines, each running `serve` (HTTP API + sync). We POST a learning to A's API
# and then GET it back from B's API — so it travels: A HTTP -> A store -> iroh sync -> B store
# -> B HTTP. This is the full stack the Python MCP server (Phase 7) will sit on top of.
#
# Usage:  ./scripts/test-api.sh
set -euo pipefail

# shellcheck disable=SC1091
. "$HOME/.cargo/env" 2>/dev/null || true
cd "$(dirname "$0")/../daemon"

BIN=./target/debug/learnings-daemon
A=/tmp/la-a
B=/tmp/la-b
PORT_A=11821      # iroh port (stable address for relay-free local sync)
PORT_B=11822
API_A=7788        # localhost HTTP API ports
API_B=7799
MARKER="Pin the iroh port (api test)"

echo "Building (if needed)..."
cargo build --quiet

echo "Cleaning test folders..."
rm -rf "$A" "$B"; mkdir -p "$A" "$B"

echo "1) A creates the shared notebook..."
$BIN --data-dir "$A" --port "$PORT_A" pair create > /tmp/la-create.txt
TICKET=$(grep -E '^doc' /tmp/la-create.txt | head -1)
echo "   ticket: ${TICKET:0:24}... (${#TICKET} chars)"

echo "2) B joins with the ticket..."
$BIN --data-dir "$B" --port "$PORT_B" pair join "$TICKET" > /dev/null
echo "   B joined."

echo "3) Start A serving its API (data holder, listening for B)..."
timeout 70 $BIN --data-dir "$A" --port "$PORT_A" serve --api-port "$API_A" > /tmp/la-a.txt 2>&1 &
APID=$!

echo "4) Start B serving its API..."
timeout 70 $BIN --data-dir "$B" --port "$PORT_B" serve --api-port "$API_B" > /tmp/la-b.txt 2>&1 &
BPID=$!

wait_for_api() {  # $1 = port
  for _ in $(seq 1 40); do
    curl -sf "localhost:$1/status" >/dev/null 2>&1 && return 0
    sleep 0.5
  done
  return 1
}

echo "5) Waiting for both APIs to come up..."
wait_for_api "$API_A" && wait_for_api "$API_B" || { echo "RESULT: FAILED — an API never came up."; kill "$APID" "$BPID" 2>/dev/null || true; exit 1; }
sleep 3   # let the two nodes find each other

echo "6) POST a learning to A's API..."
curl -s -X POST "localhost:$API_A/learnings" \
  -H 'Content-Type: application/json' \
  -d "{\"title\":\"$MARKER\",\"body\":\"Keeps the address stable across restarts.\",\"tags\":[\"iroh\",\"gotcha\"],\"author\":\"A\"}" > /dev/null
echo "   posted to A."

echo "7) Polling B's API until the learning shows up (up to ~50s)..."
FOUND=0
for i in $(seq 1 50); do
  if curl -s "localhost:$API_B/learnings?query=api%20test" 2>/dev/null | grep -q "$MARKER"; then
    FOUND=1; echo "   visible on B's API after ~${i}s"; break
  fi
  sleep 1
done

echo
echo "----- B's /status -----"
curl -s "localhost:$API_B/status"; echo
echo "----- B's /learnings?query=api%20test -----"
curl -s "localhost:$API_B/learnings?query=api%20test"; echo
echo "-------------------------------------------"

kill "$APID" "$BPID" 2>/dev/null || true
wait "$APID" "$BPID" 2>/dev/null || true

if [ "$FOUND" -ne 1 ]; then
  echo "RESULT: FAILED — the learning POSTed to A never appeared on B's API."
  exit 1
fi

echo
echo "RESULT: PASS — a learning POSTed to A's HTTP API was readable from B's HTTP API over iroh."
