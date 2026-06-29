# Pairing & running (Phase 8)

How two teammates connect their machines and keep the sync running. You do this **once**.

## The idea

Each machine runs one always-on **daemon** (it holds the iroh node, syncs the shared notebook,
serves a localhost HTTP API, and bridges PAI's `KNOWLEDGE/` folder). Your Claude agent talks to
that daemon through the **MCP server**, which Claude launches per session. Pairing is a one-time
exchange of a **ticket** — treat it like a shared secret; whoever holds it can join the notebook.

```
You (machine A)                          Teammate (machine B)
  daemon (always on) ──── iroh sync (p2p) ──── daemon (always on)
  Claude ⇄ MCP ⇄ daemon                        daemon ⇄ MCP ⇄ Claude
```

## One-time setup

### 1. First teammate — create the notebook

```bash
cd ~/Projects/learnings-pool
./scripts/setup.sh
```

This builds + installs the daemon, **prints a ticket**, and starts the daemon as a systemd user
service. Copy the ticket and send it to your teammate (Signal, Slack DM, etc. — it's a secret).

### 2. Second teammate — join with the ticket

```bash
cd ~/Projects/learnings-pool
./scripts/setup.sh join <paste-the-ticket-here>
```

Same thing, but joins the existing notebook instead of creating one.

### 3. Both teammates — connect Claude

Register the MCP server with Claude Code (once):

```bash
claude mcp add learnings --env LEARNINGS_API=http://127.0.0.1:7777 -- \
  uv run --project ~/Projects/learnings-pool/mcp learnings-mcp
```

That's it. From now on your agent has four tools: `share_learning`, `search_learnings`,
`get_learning`, `sync_status`.

## Check it's working

```bash
systemctl --user status learnings-daemon     # should be active (running)
curl localhost:7777/status                    # {"learnings":N,"peers":1} once paired & both up
```

In a Claude session: ask it to `sync_status` (should report a peer), `share_learning(...)` on one
machine, then `search_learnings(...)` on the other — the entry comes back.

## Keeping it alive

The systemd user service restarts on failure and starts on login. For it to run **on boot before
you log in**, enable lingering once:

```bash
sudo loginctl enable-linger $USER
```

Useful commands:

```bash
systemctl --user restart learnings-daemon     # after upgrading the binary
journalctl --user -u learnings-daemon -f       # live logs
systemctl --user stop learnings-daemon         # stop syncing
```

## Choosing what syncs (and not disturbing your other setup)

By default the daemon syncs your whole PAI `KNOWLEDGE/` folder. To sync only a **dedicated
folder** instead — so the rest of your PAI memory and any other agents stay untouched — set
`LEARNINGS_KNOWLEDGE_DIR` when running setup:

```bash
LEARNINGS_KNOWLEDGE_DIR=~/shared-learnings ./scripts/setup.sh join <ticket>
```

Already set up? Switch the folder without re-running setup:

```bash
# edit the --knowledge-dir path in the service file, then:
nano ~/.config/systemd/user/learnings-daemon.service
systemctl --user daemon-reload && systemctl --user restart learnings-daemon
```

Ports are configurable the same way: `LEARNINGS_API_PORT` (default 7777) and
`LEARNINGS_IROH_PORT` (default 11801) — change them only if something else already uses those.

Running this does **not** stop your other agents: it adds one background daemon and one extra
MCP server (`claude mcp add` appends, it doesn't replace). The only shared resource is the
folder above. Note: synced learnings are immutable (no delete in v1), so pick the folder
**before** dropping anything sensitive in it.

## Notes

- The daemon binds its HTTP API to `127.0.0.1` only — nothing is exposed to the network.
- Sync is peer-to-peer and encrypted; there's no central server. Offline is fine — a learning
  added while your teammate is away flows when you're both back online.
- Re-running `setup.sh` after pairing is safe: it skips pairing if this machine is already paired,
  rebuilds/reinstalls the binary, and refreshes the service.
