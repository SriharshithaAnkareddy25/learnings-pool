# Plan: Sync Claude/PAI "learnings" between two teammates over iroh (Rust daemon + Python MCP)

## Context

You and your team lead both drive a project with Claude agents, and **both run PAI**. Each
agent accumulates **learnings** (insights, gotchas, decisions) in PAI's memory, but those
learnings live on separate machines, so each of you misses what the other discovered. You
want them to **converge** into one shared pool both agents can read.

The two integration surfaces you mentioned — "endpoints or an MCP" — are the two *layers*
of the same system, and you need both:

- **MCP** = the agent-facing doorway. Your Claude agent calls tools (`share_learning`,
  `search_learnings`) to write/read the shared pool.
- **Transport** = moves learnings between the two machines underneath. This is iroh:
  direct, encrypted, peer-to-peer, no central server to host or secure.

> **`iroh-examples` (this clone) is reference only** — nothing is built or changed here.
> The implementation lives in a **new standalone repo** (suggested `~/Projects/learnings-pool/`).

---

## Decisions (confirmed)

1. **Both teammates run PAI** → sync PAI memory directly (no neutral folder needed).
2. **What lives where, and format** — PAI stores two different things on disk:
   - **Learnings = MARKDOWN** (`MEMORY/KNOWLEDGE/*.md`, related `LEARNING/.../*.md`).
     **This is what we sync.**
   - **Telemetry/state = JSON/JSONL** (`STATE/work.json`, `SIGNALS/ratings.jsonl`, etc.) —
     per-machine, full of local session IDs and absolute paths. **Explicitly EXCLUDED.**
3. **Scope (v1): sync everything in `KNOWLEDGE/`** — no opt-in tag yet (add a `share: team`
   filter later).
4. **Language: Rust daemon + Python MCP server (Option B).** The sync engine (iroh-docs)
   stays in Rust where it's mature and first-class; Python is used only for the MCP server.
   Rationale: iroh's Python bindings expose **only core networking** (`iroh-docs`,
   `iroh-gossip`, `iroh-blobs` are explicitly out of scope for FFI), so a pure-Python build
   would have to throw away iroh-docs and hand-roll sync — defeating the point of iroh.
5. **Wire/record format = JSON** — each learning is `{ id, title, body (markdown), tags,
   author, created }`, serialized with serde (exactly how `tauri-todos` serializes `Todo`).

---

## Architecture (Rust daemon + Python MCP)

```
  Claude agent (you)               Claude agent (lead)
        │ MCP (stdio)                     │ MCP (stdio)
        ▼                                 ▼
  MCP server (Python, mcp SDK)      MCP server (Python)
        │ localhost HTTP                  │ localhost HTTP
        ▼                                 ▼
  Sync daemon (Rust)  ◄── iroh-docs auto-sync (p2p) ──►  Sync daemon (Rust)
   • iroh Endpoint + Router                              • iroh Endpoint + Router
   • iroh-docs (multi-writer KV, auto-converges)
   • bridge ⇅ KNOWLEDGE/ (notify watcher)                • bridge ⇅ KNOWLEDGE/
   • axum localhost HTTP API
        ▼                                                       ▼
  PAI MEMORY/KNOWLEDGE/*.md                              PAI MEMORY/KNOWLEDGE/*.md
```

**Why split this way:**
- iroh-docs does the hard part (multi-writer convergence, live sync, offline tolerance) —
  keep it in Rust where it exists and the `tauri-todos` example already does ~90% of it.
- The MCP server is a thin forwarder; Python is fine (and what you want) there.
- The daemon is long-running (the iroh node must stay alive to sync); the MCP-over-stdio
  process dies per agent session, so it can't host the node — it talks to the daemon.

---

## Implementation steps

### Phase 0 — New repo, two parts
`~/Projects/learnings-pool/`, its own git repo, with:
- `daemon/` — Rust crate (cargo). Pin the exact versions `tauri-todos` uses:
  `iroh 1.0`, `iroh-docs 0.101`, `iroh-blobs 0.103`, `iroh-gossip 0.101` (see
  `tauri-todos/src-tauri/Cargo.toml`). Plus `axum`, `notify`, `serde`, `blake3`.
- `mcp/` — Python package. Deps: `mcp` (official SDK), `httpx`.

### Phase 1 — Rust node (`daemon/src/iroh.rs`)
**Copy `tauri-todos/src-tauri/src/iroh.rs` almost verbatim:** `Endpoint` build, persistent
`SecretKey` via `load_secret_key`, `Gossip`, `FsStore`, `Docs::persistent`, and the `Router`
wiring (`BLOBS_ALPN`/`GOSSIP_ALPN`/`DOCS_ALPN`). This is the node, unchanged in spirit.

### Phase 2 — Learnings store (`daemon/src/learnings.rs`)
**Adapt `tauri-todos/src-tauri/src/todos.rs`** — it is the template:
- `struct Learning { id, title, body, tags: Vec<String>, author, created, is_delete }`
  replacing `Todo`; serialize with serde_json like `Todo::as_bytes`/`from_bytes`.
- **Key by content hash:** `id = blake3(title + "\n" + body)`. Same learning on both
  machines → same key → dedupes; immutable so no edit conflicts. Reuse `insert_bytes`
  (`doc.set_bytes(author, key, content)`).
- Reuse `get_todos` → `list_learnings` (`Query::single_latest_per_key()`) and
  `doc_subscribe` (`doc.subscribe()` → `LiveEvent`) to react to incoming entries.
- Pairing reuses `Todos::new`: `iroh.docs().create()` (first peer) vs
  `iroh.docs().import(DocTicket)` (joiner) + `doc.share(ShareMode::Write, …)` for the ticket.

### Phase 3 — Pairing CLI (`daemon/src/cli.rs`)
Subcommands `pair --create` (prints the `DocTicket`) and `pair --join <ticket>`, plus
`add`, `list`, `watch` for testing.

### Phase 4 — Two-node sync (the de-risk gate)
Run two daemons (two machines or two dirs locally), pair them, `add` on one, confirm it
appears on the other live via `list`/`watch`. **Stop and verify here before the bridge/MCP.**
This is iroh-docs doing the work — minimal new code to reach this point.

### Phase 5 — Disk bridge (`daemon/src/bridge.rs`)
- **doc → disk:** on each `LiveEvent` insert, write/update `KNOWLEDGE/<id>.md`
  (frontmatter + body). Tombstone (`is_delete`) removes the file.
- **disk → doc:** watch `KNOWLEDGE/` with the `notify` crate; on a new/changed `.md`, parse
  frontmatter+body → `Learning` → `add`. (v1 syncs everything; `share: team` tag filter later.)
- Use PAI's existing frontmatter shape so synced files look native to PAI memory.

### Phase 6 — Local HTTP API (`daemon/src/api.rs`)
axum server bound to `127.0.0.1` — **pattern from `iroh-gateway/src/main.rs`**
(`Router::new().route(...).layer(Extension(state))`, `axum::serve`). Routes:
`POST /learnings`, `GET /learnings?query=`, `GET /learnings/{id}`, `GET /status`
(peer connected? + entry count).

### Phase 7 — Python MCP server (`mcp/server.py`)
Official `mcp` Python SDK, stdio transport, `httpx` to the Phase-6 API. Tools:
- `share_learning(title, body, tags)` → `POST /learnings`
- `search_learnings(query)` → `GET /learnings?query=`
- `get_learning(id)` → `GET /learnings/{id}`
- `sync_status()` → `GET /status`

Register per teammate via `claude mcp add` / `.mcp.json`.

### Phase 8 — Pairing UX & run-on-boot
Document the one-time exchange (you `pair --create` → send lead the ticket → they
`pair --join`). Add `--daemon` mode; note keeping it alive (systemd user unit / terminal tab)
so sync stays live.

---

## What you were missing / gotchas

- **Two layers, not one.** "Endpoints OR MCP" is really transport **AND** agent interface.
- **iroh Python ≠ iroh Rust** — the Python bindings are core-networking only (no docs/gossip/
  blobs). This is exactly why the daemon is Rust; don't expect iroh-docs from Python.
- **Sync the markdown, not the JSON telemetry.** PAI's JSON/JSONL is per-machine state with
  session IDs and local paths — excluding it avoids conflicts and context leaks.
- **Immutable + content-addressed.** Hash-keyed learnings dedupe across machines and never
  conflict — iroh-docs still gives multi-writer sync, but we sidestep edit-conflict cases.
- **Lifecycle mismatch.** MCP-over-stdio dies per session → the iroh node lives in a
  separate long-running daemon; MCP just forwards to it.
- **iroh-docs maturity.** It's `0.101` (pre-1.0) while core `iroh` is `1.0`. Pin the exact
  versions `tauri-todos` uses; expect API churn if you bump.
- **Two-language repo.** Keep a clean contract between Rust daemon and Python MCP = the
  localhost HTTP API. That boundary is the only coupling; document it (an OpenAPI note is enough).
- **Trust is scoped:** only a peer holding the `DocTicket` can join; every entry is signed by
  an `AuthorId`. Treat the ticket like a shared secret.
- **Offline is fine:** iroh-docs syncs on reconnect (CRDT), so a learning added while the peer
  is offline flows when both are back up.

---

## Critical reference files (read first when implementing)

- `tauri-todos/src-tauri/src/iroh.rs` — node/endpoint/docs setup (copy nearly verbatim)
- `tauri-todos/src-tauri/src/todos.rs` — KV entry CRUD + ticket + live subscribe (the template)
- `tauri-todos/src-tauri/Cargo.toml` — exact iroh / iroh-docs / iroh-blobs / iroh-gossip versions
- `iroh-gateway/src/main.rs` — axum localhost HTTP API pattern (Phase 6)
- `browser-chat/shared/src/lib.rs` — ticket serialize/parse reference

---

## Verification (end-to-end)

1. **Build:** `cargo build` in `daemon/`; `pip install -e mcp/` (or `uv`), `import mcp` works.
2. **Two-node sync (Phase 4 gate):** two terminals/machines. A: `pair --create` → copy
   ticket. B: `pair --join <ticket>`. A: `add "Use X helper" "..."`. B: `list` shows it
   within seconds; `watch` prints it live. ✅ transport proven.
3. **Bridge (Phase 5):** drop a `.md` into A's `KNOWLEDGE/` → it appears in B's `KNOWLEDGE/`.
4. **MCP (Phase 7):** register the server in Claude Code; in a session call
   `share_learning(...)`, then from the other machine call `search_learnings(...)` and confirm
   the entry returns. `sync_status()` shows peer connected.
5. **Offline tolerance:** kill B's daemon, `add` on A, restart B → entry syncs on reconnect.

---

## Out of scope for v1 (later)

- `share: team` opt-in tag / privacy filter (v1 syncs all of `KNOWLEDGE/`).
- More than 2 peers (iroh-docs supports it; pairing UX needs work).
- Editable learnings with conflict resolution (v1 is immutable/content-addressed).
- Auto-dedup of semantically-similar learnings (an agent task, not transport).
