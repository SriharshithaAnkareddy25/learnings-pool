#!/usr/bin/env bash
#
# Phase 4 — prove two nodes sync.
#
# Simulates two machines using two data folders on this one computer, then shows a learning
# written on A arriving at B over iroh. Respects the single-process store constraint:
# we only ever run one command at a time per folder, and `watch` is the long-lived one.
#
# Usage:  ./scripts/test-sync.sh
set -euo pipefail

# shellcheck disable=SC1091
. "$HOME/.cargo/env" 2>/dev/null || true
cd "$(dirname "$0")/../daemon"

BIN=./target/debug/learnings-daemon
A=/tmp/ls-a
B=/tmp/ls-b
# Pin each node to a fixed UDP port. On one machine with no relay, this keeps each node's
# address stable across the separate create/add/watch invocations, so the address baked into
# the ticket is still valid when the node later watches. (Real, separate machines use the
# relay + DNS discovery instead and don't need this.)
PORT_A=11801
PORT_B=11802
TITLE="Use the X helper (sync test)"
BODY="Bare Y skips validation; the X helper validates first."
MARKER="X helper (sync test)"

echo "Building (if needed)..."
cargo build --quiet

echo "Cleaning test folders..."
rm -rf "$A" "$B"; mkdir -p "$A" "$B"

echo "1) A creates the shared notebook..."
$BIN --data-dir "$A" --port "$PORT_A" pair create > /tmp/ls-create.txt
TICKET=$(grep -E '^doc' /tmp/ls-create.txt | head -1)
echo "   ticket: ${TICKET:0:24}... (${#TICKET} chars)"

echo "2) B joins with the ticket..."
$BIN --data-dir "$B" --port "$PORT_B" pair join "$TICKET" > /dev/null
echo "   B joined."

echo "3) A files a learning (while A is not watching)..."
$BIN --data-dir "$A" --port "$PORT_A" add "$TITLE" "$BODY" --tags gotcha,api --author A > /dev/null
echo "   filed on A."

echo "4) Start A watching (the node that holds the learning, listening for B)..."
timeout 60 $BIN --data-dir "$A" --port "$PORT_A" watch > /tmp/ls-a-watch.txt 2>&1 &
APID=$!
sleep 3   # let A be listening before B dials it

echo "5) Start B watching (B dials A and pulls the learning)..."
timeout 60 $BIN --data-dir "$B" --port "$PORT_B" watch > /tmp/ls-b-watch.txt 2>&1 &
BPID=$!

echo "6) Waiting for the learning to reach B (up to ~50s)..."
FOUND=0
for i in $(seq 1 50); do
  if grep -q "$MARKER" /tmp/ls-b-watch.txt 2>/dev/null; then
    FOUND=1; echo "   reached B after ~$((i + 3))s"; break
  fi
  sleep 1
done

# Give iroh-docs a moment to flush the synced entry to B's on-disk store before we kill the
# watcher, so the follow-up `list` (a fresh process) sees the persisted learning.
[ "$FOUND" -eq 1 ] && sleep 3

# Stop the watchers so their folders are free for the final `list`.
kill "$APID" "$BPID" 2>/dev/null || true
wait "$APID" "$BPID" 2>/dev/null || true

echo
echo "----- B's watch output -----"
cat /tmp/ls-b-watch.txt
echo "----------------------------"

if [ "$FOUND" -ne 1 ]; then
  echo "RESULT: FAILED — the learning did not reach B in time."
  exit 1
fi

echo
echo "7) Confirm B's notebook now persists it:"
$BIN --data-dir "$B" --port "$PORT_B" list

echo
echo "RESULT: PASS — a learning written on A reached B over iroh."
