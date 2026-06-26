#!/usr/bin/env bash
#
# Phase 5 — prove the disk bridge.
#
# Two simulated machines, each running `bridge` over its own KNOWLEDGE folder. A markdown file
# dropped into A's folder should travel: A disk → A's doc → (iroh sync) → B's doc → B disk,
# landing as a file in B's folder. We use file drops (not `add`/`list`) because the long-running
# `bridge` holds each node's single-process store.
#
# Usage:  ./scripts/test-bridge.sh
set -euo pipefail

# shellcheck disable=SC1091
. "$HOME/.cargo/env" 2>/dev/null || true
cd "$(dirname "$0")/../daemon"

BIN=./target/debug/learnings-daemon
A=/tmp/lb-a
B=/tmp/lb-b
KA=/tmp/lb-ka   # A's KNOWLEDGE folder
KB=/tmp/lb-kb   # B's KNOWLEDGE folder
PORT_A=11811
PORT_B=11812
MARKER="Prefer the shared helper (bridge test)"

echo "Building (if needed)..."
cargo build --quiet

echo "Cleaning test folders..."
rm -rf "$A" "$B" "$KA" "$KB"; mkdir -p "$A" "$B" "$KA" "$KB"

echo "1) A creates the shared notebook..."
$BIN --data-dir "$A" --port "$PORT_A" pair create > /tmp/lb-create.txt
TICKET=$(grep -E '^doc' /tmp/lb-create.txt | head -1)
echo "   ticket: ${TICKET:0:24}... (${#TICKET} chars)"

echo "2) B joins with the ticket..."
$BIN --data-dir "$B" --port "$PORT_B" pair join "$TICKET" > /dev/null
echo "   B joined."

echo "3) Start A bridging its KNOWLEDGE folder..."
timeout 70 $BIN --data-dir "$A" --port "$PORT_A" bridge --knowledge-dir "$KA" > /tmp/lb-a.txt 2>&1 &
APID=$!
sleep 3   # let A be listening before B dials it

echo "4) Start B bridging its KNOWLEDGE folder..."
timeout 70 $BIN --data-dir "$B" --port "$PORT_B" bridge --knowledge-dir "$KB" > /tmp/lb-b.txt 2>&1 &
BPID=$!
sleep 3

echo "5) Drop a PAI-style learning into A's KNOWLEDGE folder..."
cat > "$KA/my-note.md" <<EOF
# ${MARKER}

The bare call skips a validation step; the shared helper does it for you.
Use the shared helper everywhere.
EOF
echo "   dropped $KA/my-note.md"

echo "6) Waiting for it to appear as a file in B's KNOWLEDGE folder (up to ~50s)..."
FOUND=0
for i in $(seq 1 50); do
  if grep -rqs "$MARKER" "$KB" 2>/dev/null; then
    FOUND=1; echo "   appeared in B after ~${i}s"; break
  fi
  sleep 1
done

# Let writes settle, then stop the bridges.
[ "$FOUND" -eq 1 ] && sleep 2
kill "$APID" "$BPID" 2>/dev/null || true
wait "$APID" "$BPID" 2>/dev/null || true

echo
echo "----- files now in B's KNOWLEDGE folder ($KB) -----"
ls -1 "$KB" || true
echo "---------------------------------------------------"
if [ "$FOUND" -eq 1 ]; then
  echo
  echo "----- B's synced file content -----"
  cat "$KB"/*.md 2>/dev/null | sed 's/^/   /'
  echo "-----------------------------------"
fi

# Loop-safety: the bridge must not have churned files in a tight echo loop. One canonical
# file per learning on each side is what we expect.
A_COUNT=$(find "$KA" -name '*.md' | wc -l | tr -d ' ')
B_COUNT=$(find "$KB" -name '*.md' | wc -l | tr -d ' ')
echo
echo "file counts — A: $A_COUNT (the original), B: $B_COUNT (the synced copy)"

if [ "$FOUND" -ne 1 ]; then
  echo "RESULT: FAILED — the learning did not bridge to B in time."
  exit 1
fi

echo
echo "RESULT: PASS — a markdown learning dropped on A bridged through iroh to a file on B."
