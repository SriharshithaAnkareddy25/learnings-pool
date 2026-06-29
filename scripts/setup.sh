#!/usr/bin/env bash
#
# Phase 8 — one-time setup + run-on-boot.
#
# Builds and installs the daemon, pairs this machine into the shared notebook, and installs a
# systemd user service so the daemon stays running (and starts on boot). Run once per machine.
#
# Usage:
#   ./scripts/setup.sh                 # FIRST teammate: create a new shared notebook (prints a ticket)
#   ./scripts/setup.sh join <ticket>   # SECOND teammate: join using the ticket from the first
#
# After this, register the MCP server with Claude (see the printed next steps / docs/PAIRING.md).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_SRC="$ROOT/daemon/target/release/learnings-daemon"
BIN_DST="$HOME/.local/bin/learnings-daemon"
DATA_DIR="$HOME/.local/share/learnings-sync"
KNOWLEDGE_DIR="$HOME/.claude/PAI/MEMORY/KNOWLEDGE"
UNIT_SRC="$ROOT/systemd/learnings-daemon.service"
UNIT_DST="$HOME/.config/systemd/user/learnings-daemon.service"
IROH_PORT=11801
MODE="${1:-create}"

# shellcheck disable=SC1091
. "$HOME/.cargo/env" 2>/dev/null || true

echo "==> Building the daemon (release)..."
(cd "$ROOT/daemon" && cargo build --release)

echo "==> Installing the binary to $BIN_DST ..."
mkdir -p "$HOME/.local/bin" "$DATA_DIR" "$KNOWLEDGE_DIR"
install -m 0755 "$BIN_SRC" "$BIN_DST"

# Pair this machine into the shared notebook (only if it isn't already paired).
if [ -f "$DATA_DIR/active-doc" ]; then
  echo "==> Already paired (found $DATA_DIR/active-doc) — skipping pairing."
else
  case "$MODE" in
    join)
      TICKET="${2:-}"
      [ -n "$TICKET" ] || { echo "ERROR: usage: $0 join <ticket>"; exit 1; }
      echo "==> Joining the shared notebook..."
      "$BIN_DST" --data-dir "$DATA_DIR" --port "$IROH_PORT" pair join "$TICKET"
      ;;
    create)
      echo "==> Creating a new shared notebook..."
      "$BIN_DST" --data-dir "$DATA_DIR" --port "$IROH_PORT" pair create
      ;;
    *)
      echo "ERROR: unknown mode '$MODE' (use nothing, or: join <ticket>)"; exit 1
      ;;
  esac
fi

echo "==> Installing the systemd user service..."
mkdir -p "$(dirname "$UNIT_DST")"
install -m 0644 "$UNIT_SRC" "$UNIT_DST"
systemctl --user daemon-reload
systemctl --user enable --now learnings-daemon.service

# Run-on-boot without an active login session needs lingering. Best-effort (may need a one-time
# `sudo loginctl enable-linger $USER` if this can't do it unprivileged).
loginctl enable-linger "$USER" 2>/dev/null && echo "==> Lingering enabled (starts on boot)." \
  || echo "NOTE: could not enable lingering automatically. For start-on-boot run once:
       sudo loginctl enable-linger $USER"

echo
echo "==> Service status:"
systemctl --user --no-pager --lines=0 status learnings-daemon.service || true
echo
echo "==> Done. Next steps:"
echo "   1. Register the MCP server with Claude (once):"
echo "        claude mcp add learnings --env LEARNINGS_API=http://127.0.0.1:7777 -- \\"
echo "          uv run --project $ROOT/mcp learnings-mcp"
if [ "$MODE" = "create" ] && [ ! -f "$DATA_DIR/.ticket-sent" ]; then
  echo "   2. Send your teammate the ticket printed above; they run:"
  echo "        ./scripts/setup.sh join <ticket>"
fi
echo "   Check it's alive:  curl localhost:7777/status"
