//! Phase 3 — the command-line interface and pairing.
//!
//! Commands: `id`, `pair create`, `pair join <ticket>`, `add`, `list`, `watch`.
//!
//! IMPORTANT: the on-disk store is single-process. Run only ONE command at a time per
//! `--data-dir` (the long-running `watch` holds the store while it runs). To test sync on
//! one computer, use two different `--data-dir` folders (two simulated machines).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand};
use iroh_docs::{DocTicket, NamespaceId, engine::LiveEvent};
use n0_future::StreamExt;

use crate::learnings::Learnings;
use crate::node::Iroh;

#[derive(Parser)]
#[command(name = "learnings-daemon", about = "Sync PAI learnings between teammates over iroh")]
pub struct Cli {
    /// Folder holding this node's identity + data.
    /// Use different folders to simulate two machines on one computer.
    #[arg(long, default_value = "data", global = true)]
    pub data_dir: PathBuf,

    /// Pin this node's UDP port. Keeps its address stable across restarts so two nodes on one
    /// machine can sync directly without a relay. Default: an ephemeral port.
    #[arg(long, global = true)]
    pub port: Option<u16>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Print this machine's NodeId (its permanent network identity).
    Id,
    /// Create or join the shared notebook.
    Pair {
        #[command(subcommand)]
        action: PairAction,
    },
    /// File a learning into the shared notebook.
    Add {
        /// Short headline.
        title: String,
        /// The full note (markdown).
        body: String,
        /// Comma-separated tags, e.g. --tags gotcha,api
        #[arg(long, value_delimiter = ',', default_value = "")]
        tags: Vec<String>,
        /// Who is writing this.
        #[arg(long, default_value = "me")]
        author: String,
    },
    /// List the learnings currently in the shared notebook.
    List,
    /// Stay running, keep syncing, and print learnings as they arrive.
    Watch,
    /// Mirror the shared notebook ⇄ a folder of markdown files (PAI's KNOWLEDGE/).
    Bridge {
        /// Folder to mirror. Defaults to PAI's KNOWLEDGE memory.
        #[arg(long)]
        knowledge_dir: Option<PathBuf>,
    },
    /// Run the daemon: keep syncing and serve the localhost HTTP API (what the MCP server calls).
    Serve {
        /// Port for the localhost HTTP API.
        #[arg(long, default_value_t = 7777)]
        api_port: u16,
        /// Also mirror this folder while serving (defaults to PAI's KNOWLEDGE memory if the flag
        /// is given with no value). Omit the flag entirely to run the API only.
        #[arg(long, num_args = 0..=1, default_missing_value = "")]
        knowledge_dir: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum PairAction {
    /// Create the shared notebook and print a ticket to send your teammate.
    Create,
    /// Join your teammate's notebook using their ticket.
    Join { ticket: String },
}

/// Boot the node and run the requested command.
pub async fn run(cli: Cli) -> Result<()> {
    let node = Iroh::new(cli.data_dir.clone(), cli.port).await?;
    let active_path = cli.data_dir.join("active-doc");

    let result = dispatch(&node, &active_path, cli.command).await;

    node.shutdown().await?;
    result
}

async fn dispatch(node: &Iroh, active_path: &Path, command: Command) -> Result<()> {
    match command {
        Command::Id => {
            println!("{}", node.endpoint_id());
        }

        Command::Pair { action: PairAction::Create } => {
            let store = Learnings::create(node.clone()).await?;
            save_active(active_path, store.id())?;
            println!("Connecting to a relay so the ticket has a reachable address...");
            node.online().await;
            let ticket = store.share_ticket().await?;
            println!("Created the shared notebook.\n");
            println!("Send this ticket to your teammate:\n");
            println!("{ticket}\n");
            println!("They join with:  learnings-daemon pair join <ticket>");
        }

        Command::Pair { action: PairAction::Join { ticket } } => {
            // Remember the teammate's address from the ticket, so later `watch`/`bridge` runs can
            // re-dial them directly — `open` carries no peer info, and without a relay/DNS the
            // node id alone isn't enough to find them again.
            if let Ok(parsed) = DocTicket::from_str(&ticket) {
                save_peers(&peers_path(active_path), &parsed.nodes);
            }
            let store = Learnings::join(node.clone(), &ticket).await?;
            save_active(active_path, store.id())?;
            println!("Joined the shared notebook ({}).", store.id());
            println!("Run `watch` (here and on your teammate's machine) to sync.");
        }

        Command::Add { title, body, tags, author } => {
            let store = open_active(node, active_path).await?;
            let tags: Vec<String> = tags.into_iter().filter(|t| !t.is_empty()).collect();
            let learning = store.add(title, body, tags, author).await?;
            println!("Filed learning {}", &learning.id[..16]);
        }

        Command::List => {
            let store = open_active(node, active_path).await?;
            let all = store.list().await?;
            if all.is_empty() {
                println!("(no learnings yet)");
            }
            for l in all {
                let tags = if l.tags.is_empty() { String::new() } else { format!("  [{}]", l.tags.join(", ")) };
                println!("• {}  ({}…){}", l.title, &l.id[..8], tags);
            }
        }

        Command::Watch => {
            println!("Connecting to a relay...");
            node.online().await;
            let store = open_active_syncing(node, active_path).await?;
            watch(store).await?;
        }

        Command::Bridge { knowledge_dir } => {
            let dir = knowledge_dir.unwrap_or_else(default_knowledge_dir);
            println!("Connecting to a relay...");
            node.online().await;
            let store = open_active_syncing(node, active_path).await?;
            crate::bridge::run(store, dir).await?;
        }

        Command::Serve { api_port, knowledge_dir } => {
            println!("Connecting to a relay...");
            node.online().await;
            let store = open_active_syncing(node, active_path).await?;
            match knowledge_dir.map(resolve_knowledge_dir) {
                // API + bridge over one shared store: this is the single always-on process.
                Some(dir) => {
                    println!("Also bridging {}", dir.display());
                    tokio::select! {
                        r = crate::api::run(store.clone(), api_port) => r?,
                        r = crate::bridge::run(store, dir) => r?,
                    }
                }
                // API only.
                None => crate::api::run(store, api_port).await?,
            }
        }
    }
    Ok(())
}

/// `--knowledge-dir` with no value means "use the PAI default"; with a value, use it.
fn resolve_knowledge_dir(dir: PathBuf) -> PathBuf {
    if dir.as_os_str().is_empty() {
        default_knowledge_dir()
    } else {
        dir
    }
}

/// PAI's curated-knowledge folder — the default target the bridge mirrors.
fn default_knowledge_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".claude/PAI/MEMORY/KNOWLEDGE")
}

/// Stay running and print learnings as they appear (including ones synced from a teammate).
async fn watch(store: Learnings) -> Result<()> {
    println!("Watching for learnings (Ctrl-C to stop)...\n");

    let mut seen: HashSet<String> = HashSet::new();
    for l in store.list().await? {
        println!("• {}  ({}…)", l.title, &l.id[..8]);
        seen.insert(l.id);
    }

    let mut events = store.subscribe().await?;
    while let Some(event) = events.next().await {
        // On any change (local, remote, or content arriving), re-scan and print new ones.
        match event? {
            LiveEvent::InsertRemote { .. }
            | LiveEvent::InsertLocal { .. }
            | LiveEvent::ContentReady { .. }
            | LiveEvent::PendingContentReady
            | LiveEvent::NeighborUp(_)
            | LiveEvent::SyncFinished(_) => {
                for l in store.list().await? {
                    if seen.insert(l.id.clone()) {
                        println!("\n↓ new learning from sync: {}  ({}…)", l.title, &l.id[..8]);
                        println!("   {}", l.body);
                    }
                }
            }
            LiveEvent::NeighborDown(_) => {}
        }
    }
    Ok(())
}

/// Remember which notebook is the active shared one (so add/list/watch reopen the same one).
fn save_active(path: &Path, id: NamespaceId) -> Result<()> {
    std::fs::write(path, id.to_string()).context("failed to save active notebook id")?;
    Ok(())
}

/// Reopen the active shared notebook saved by a previous `pair create`/`join`.
async fn open_active(node: &Iroh, path: &Path) -> Result<Learnings> {
    let id_str = std::fs::read_to_string(path)
        .map_err(|_| anyhow!("no active notebook — run `pair create` or `pair join` first"))?;
    let id = NamespaceId::from_str(id_str.trim()).context("saved notebook id is invalid")?;
    Learnings::open(node.clone(), id).await
}

/// Reopen the active notebook AND start syncing it with the saved peers. Long-running commands
/// must use this — a plain `open` never engages sync, so the node would never connect.
async fn open_active_syncing(node: &Iroh, active_path: &Path) -> Result<Learnings> {
    let store = open_active(node, active_path).await?;
    let peers = load_peers(&peers_path(active_path));
    store.start_sync(peers).await?;
    Ok(store)
}

/// Where the saved peer addresses live, next to the active-doc marker.
fn peers_path(active_path: &Path) -> PathBuf {
    active_path.with_file_name("peers.json")
}

/// Persist the teammate's addresses (best-effort — sync still works via discovery without them).
fn save_peers(path: &Path, peers: &[iroh::EndpointAddr]) {
    if let Ok(json) = serde_json::to_string(peers) {
        let _ = std::fs::write(path, json);
    }
}

/// Load saved peer addresses, or an empty list if none/unreadable.
fn load_peers(path: &Path) -> Vec<iroh::EndpointAddr> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
