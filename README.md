# learnings-sync

Sync Claude/PAI **learnings** between two teammates over [iroh](https://iroh.computer)
(direct, encrypted, peer-to-peer — no central server).

When either teammate's agent records a learning, it converges into a shared pool that both
agents can read. See the full design in
[`../iroh-examples-main/Plans/this-is-the-repo-sleepy-tiger.md`](../iroh-examples-main/Plans/this-is-the-repo-sleepy-tiger.md).

## Layout

```
learnings-sync/
├── daemon/   Rust — the always-on background worker.
│             Holds the iroh node + iroh-docs (the auto-syncing shared store),
│             watches PAI's KNOWLEDGE/ folder, and exposes a small localhost HTTP API.
└── mcp/      Python — the MCP server your Claude agent talks to.
│             A thin forwarder that calls the daemon's localhost API.
```

The **only** coupling between the two is the daemon's localhost HTTP API.

## Status

Two-node sync works and bridges to disk. Drop a learning into one machine's `KNOWLEDGE/`
folder and it appears in the other's, peer-to-peer over iroh.

| Phase | What it adds |
|-------|--------------|
| 0 ✅  | Repo + two sub-projects + dependencies |
| 1 ✅  | iroh node (machine identity + network presence) |
| 2 ✅  | Learnings store (the synced records) |
| 3 ✅  | Pairing CLI (`pair`, `add`, `list`, `watch`) |
| 4 ✅  | Two-node sync working (de-risk gate) — `scripts/test-sync.sh` |
| 5 ✅  | Bridge to a `KNOWLEDGE/` folder (`bridge`) — `scripts/test-bridge.sh` |
| 6 ✅  | Localhost HTTP API (`serve`) — `scripts/test-api.sh` |
| 7 ✅  | Python MCP server (the agent's tools) — `scripts/test-mcp.sh` |
| 8 ✅  | Pairing UX + run-on-boot (`scripts/setup.sh`, systemd) — [docs/PAIRING.md](docs/PAIRING.md) |

**Set up two real machines:** see **[docs/PAIRING.md](docs/PAIRING.md)** — `./scripts/setup.sh`
on the first (prints a ticket), `./scripts/setup.sh join <ticket>` on the second, then
`claude mcp add` on both.

### Verify it yourself

```bash
cd daemon && cargo build
../scripts/test-sync.sh     # Phase 4: a learning written on A reaches B over iroh
../scripts/test-bridge.sh   # Phase 5: a .md dropped in A's folder appears in B's folder
../scripts/test-api.sh      # Phase 6: a learning POSTed to A's API is readable from B's API
../scripts/test-mcp.sh      # Phase 7: the four MCP tools forward to the daemon
```

### Connect your Claude agent (Phase 7)

The MCP server (`mcp/`) gives Claude four tools — `share_learning`, `search_learnings`,
`get_learning`, `sync_status` — each forwarding to the daemon's HTTP API. It's short-lived
(Claude launches it per session); the always-on daemon does the syncing.

1. Run the daemon: `learnings-daemon --port 11801 serve --api-port 7777`
2. Register the server with Claude Code. The repo ships a `.mcp.json`:
   ```bash
   claude mcp add learnings -- uv run --project ./mcp learnings-mcp
   # or rely on the checked-in .mcp.json (command: uv run --project ./mcp learnings-mcp)
   ```
   Config via env: `LEARNINGS_API` (default `http://127.0.0.1:7777`), `LEARNINGS_AUTHOR`.
3. In a session, ask Claude to `share_learning(...)`; from your teammate's machine, ask Claude to
   `search_learnings(...)` and the entry comes back — one agent's insight, found by the other.

### The daemon (Phase 6)

`serve` is the always-on process: it keeps the notebook syncing and exposes a localhost HTTP API
(bound to `127.0.0.1` only) that the Phase-7 MCP server will call.

```bash
learnings-daemon --port 11801 serve --api-port 7777            # API only
learnings-daemon --port 11801 serve --api-port 7777 --knowledge-dir  # API + bridge PAI's KNOWLEDGE

curl localhost:7777/status
curl -X POST localhost:7777/learnings -d '{"title":"Use X","body":"...","tags":["api"]}'
curl 'localhost:7777/learnings?query=X'
curl localhost:7777/learnings/<id>
```

Both simulate two machines on one computer with two data folders. Each node pins a fixed UDP
port (`--port`) so its address stays stable across the separate CLI runs and the two can reach
each other directly without a relay; real, separate machines use the relay + DNS discovery
instead.
