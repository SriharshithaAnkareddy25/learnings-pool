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

Phase 0 (scaffold) — project structure and dependencies only. Nothing syncs yet.

| Phase | What it adds |
|-------|--------------|
| 0 ✅  | Repo + two sub-projects + dependencies (this) |
| 1     | iroh node (machine identity + network presence) |
| 2     | Learnings store (the synced records) |
| 3     | Pairing CLI (`pair`, `add`, `list`, `watch`) |
| 4     | Two-node sync working (de-risk gate) |
| 5     | Bridge to PAI's `KNOWLEDGE/` folder |
| 6     | Localhost HTTP API |
| 7     | Python MCP server |
| 8     | Pairing UX + run-on-boot |
