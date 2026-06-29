# learnings-pool

A shared notebook for two Claude/PAI agents. When your agent learns something
worth keeping, it writes it down — and a moment later your teammate's agent can
read it. No server, no cloud account: the two machines talk **directly** to each
other, encrypted, over [iroh](https://iroh.computer).

Think of it as a small pool of "things we figured out." Either agent can add to
it (`share_learning`) or search it (`search_learnings`) before solving a problem,
so neither of you solves the same thing twice.

## How it works

Each machine runs one always-on **daemon** (Rust). It holds the iroh network
identity, keeps the shared pool in sync peer-to-peer, and watches a folder on
disk — anything dropped into PAI's `KNOWLEDGE/` folder gets shared automatically.
Your Claude agent never talks to iroh directly; it talks to a small **MCP server**
(Python) that forwards calls to the daemon's localhost API.

```
You (machine A)                          Teammate (machine B)
  Claude ⇄ MCP ⇄ daemon ──── iroh p2p ──── daemon ⇄ MCP ⇄ Claude
```

The only coupling between the two halves is the daemon's localhost HTTP API.

## Layout

```
learnings-pool/
├── daemon/   Rust — the always-on background worker. Holds the iroh node,
│             keeps the shared pool synced, watches KNOWLEDGE/, and exposes a
│             small localhost HTTP API.
└── mcp/      Python — the MCP server your Claude agent talks to. A thin
              forwarder that calls the daemon's localhost API.
```

## Build it yourself

### Prerequisites

- **Rust** (`cargo`) — builds the daemon. <https://rustup.rs>
- **uv** — runs the Python MCP server. <https://docs.astral.sh/uv/>
- **Claude Code CLI** — the agent that uses the tools.

### 1. Try it on one machine first

These scripts simulate two machines on your computer (two data folders) so you
can watch a learning travel from one side to the other before pairing for real.

```bash
cd daemon && cargo build
../scripts/test-sync.sh     # a learning written on A reaches B over iroh
../scripts/test-bridge.sh   # a .md dropped in A's folder appears in B's folder
../scripts/test-api.sh      # a learning POSTed to A's API is readable from B's API
../scripts/test-mcp.sh      # the four MCP tools forward to the daemon
```

### 2. Pair two real machines

One-time setup. The first teammate creates the pool and gets a **ticket**
(treat it like a shared secret); the second joins with it.

```bash
# First teammate — creates the pool, prints a ticket, starts the daemon on boot
./scripts/setup.sh

# Second teammate — joins the existing pool
./scripts/setup.sh join <paste-the-ticket-here>
```

`setup.sh` builds and installs the daemon, sets up the Python MCP server, and
installs a systemd user service so the daemon keeps running. By default it syncs
your whole PAI `KNOWLEDGE/` folder — point `LEARNINGS_KNOWLEDGE_DIR` at a
dedicated folder first if you'd rather share only that.

Full walkthrough (lingering, logs, choosing what syncs):
**[docs/PAIRING.md](docs/PAIRING.md)**.

### 3. Connect your Claude agent

Register the MCP server with Claude Code, once per machine:

```bash
claude mcp add learnings --env LEARNINGS_API=http://127.0.0.1:7777 -- \
  uv run --project ./mcp learnings-mcp
```

That's it. Your agent now has four tools:

| Tool | What it does |
|------|--------------|
| `share_learning`  | add a learning to the shared pool |
| `search_learnings`| find learnings by text or tag |
| `get_learning`    | fetch one learning by id |
| `sync_status`     | how many learnings, how many peers connected |

Ask Claude to `share_learning(...)` on one machine; from the other, ask it to
`search_learnings(...)` — one agent's insight, found by the other.

### Talking to the daemon directly (optional)

The daemon's API is plain HTTP on `127.0.0.1`, handy for debugging:

```bash
learnings-daemon --port 11801 serve --api-port 7777    # run it manually

curl localhost:7777/status
curl -X POST localhost:7777/learnings -d '{"title":"Use X","body":"...","tags":["api"]}'
curl 'localhost:7777/learnings?query=X'
curl localhost:7777/learnings/<id>
```

## Status

Two-node sync works and bridges to disk. Drop a learning into one machine's
`KNOWLEDGE/` folder and it appears in the other's, peer-to-peer over iroh.

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

## Design

The full design and rationale: [`Plans/learnings-pool-design.md`](Plans/learnings-pool-design.md).
