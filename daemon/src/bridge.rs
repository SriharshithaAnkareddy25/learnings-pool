//! Phase 5 — the disk bridge.
//!
//! Mirrors the shared notebook (the iroh-docs `Doc`) and a folder of markdown files
//! (PAI's `KNOWLEDGE/`) into each other:
//!
//!   * **doc → disk:** when a learning is added locally or arrives from a teammate, write it
//!     out as `KNOWLEDGE/learning-<id8>.md` (frontmatter + body). A tombstone removes the file.
//!   * **disk → doc:** when a `.md` is created or changed in the folder, parse it into a
//!     `Learning` and file it into the notebook so it syncs to the teammate.
//!
//! Both directions are **content-addressed no-ops once the content already exists on the other
//! side**, which is what stops the doc→disk→notify→doc echo loop without any timers.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use iroh_docs::engine::LiveEvent;
use n0_future::StreamExt;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::learnings::{Learning, Learnings};

/// Run both bridge directions until the process is stopped (long-running, like `watch`).
pub async fn run(store: Learnings, knowledge_dir: PathBuf) -> Result<()> {
    tokio::fs::create_dir_all(&knowledge_dir)
        .await
        .with_context(|| format!("creating knowledge dir {}", knowledge_dir.display()))?;

    println!("Bridging the shared notebook ⇄ {}", knowledge_dir.display());
    println!("(Ctrl-C to stop)\n");

    // Reconcile once up front so we start consistent: pull in any files already on disk, then
    // write out anything in the notebook that isn't represented on disk yet.
    import_dir(&store, &knowledge_dir).await?;
    export_all(&store, &knowledge_dir).await?;

    // Then run both live loops concurrently. If either ends/errors, the bridge stops.
    tokio::select! {
        r = doc_to_disk(&store, &knowledge_dir) => r,
        r = disk_to_doc(&store, &knowledge_dir) => r,
    }
}

// ---------------------------------------------------------------------------------------------
// doc → disk
// ---------------------------------------------------------------------------------------------

/// React to notebook changes by mirroring them to disk.
async fn doc_to_disk(store: &Learnings, dir: &Path) -> Result<()> {
    let mut events = store.subscribe().await?;
    while let Some(event) = events.next().await {
        match event? {
            LiveEvent::InsertRemote { .. }
            | LiveEvent::InsertLocal { .. }
            | LiveEvent::ContentReady { .. }
            | LiveEvent::PendingContentReady
            | LiveEvent::NeighborUp(_)
            | LiveEvent::SyncFinished(_) => {
                export_all(store, dir).await?;
            }
            LiveEvent::NeighborDown(_) => {}
        }
    }
    Ok(())
}

/// Write every learning the notebook holds that isn't already on disk; remove files for
/// tombstoned learnings. Skipping ones already on disk is the doc→disk half of the loop guard.
async fn export_all(store: &Learnings, dir: &Path) -> Result<()> {
    let on_disk = ids_on_disk(dir).await;

    for l in store.list_all().await? {
        if l.is_delete {
            // Remove whatever file currently encodes this content, if any.
            if let Some(path) = on_disk.get(&l.id) {
                let _ = tokio::fs::remove_file(path).await;
                println!("✗ removed {}", path.display());
            }
            continue;
        }
        // Already represented on disk (by our canonical file or a PAI-authored one) → skip.
        if on_disk.contains_key(&l.id) {
            continue;
        }
        let path = canonical_path(dir, &l.id);
        tokio::fs::write(&path, render(&l))
            .await
            .with_context(|| format!("writing {}", path.display()))?;
        println!("→ wrote {}", path.display());
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// disk → doc
// ---------------------------------------------------------------------------------------------

/// Watch the folder and file new/changed markdown into the notebook.
async fn disk_to_doc(store: &Learnings, dir: &Path) -> Result<()> {
    // `notify` calls back on its own OS thread, so we hand paths to the async side via a channel.
    let (tx, mut rx) = mpsc::channel::<PathBuf>(256);
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res {
            if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                for path in event.paths {
                    if is_markdown(&path) {
                        // Drop on a full channel rather than block the watcher thread.
                        let _ = tx.try_send(path);
                    }
                }
            }
        }
    })
    .context("creating filesystem watcher")?;
    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("watching {}", dir.display()))?;
    // `watcher` must stay alive for the lifetime of this loop, so keep it bound here.

    while let Some(path) = rx.recv().await {
        if let Err(e) = import_file(store, &path).await {
            eprintln!("skip {}: {e:#}", path.display());
        }
    }
    Ok(())
}

/// File every markdown file currently in the folder (used once at startup).
async fn import_dir(store: &Learnings, dir: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if is_markdown(&path) {
            if let Err(e) = import_file(store, &path).await {
                eprintln!("skip {}: {e:#}", path.display());
            }
        }
    }
    Ok(())
}

/// Parse one markdown file and file it — unless the notebook already holds that exact content.
/// The `get`-before-`add` check is the disk→doc half of the loop guard.
async fn import_file(store: &Learnings, path: &Path) -> Result<()> {
    let raw = tokio::fs::read_to_string(path).await?;
    let parsed = parse(&raw, path);
    let id = Learning::content_id(&parsed.title, &parsed.body);
    if store.get(&id).await?.is_some() {
        return Ok(()); // already in the notebook — nothing new, breaks the echo
    }
    store
        .add(parsed.title, parsed.body, parsed.tags, parsed.author)
        .await?;
    println!("↑ filed {} from {}", &id[..8], path.display());
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// markdown <-> Learning
// ---------------------------------------------------------------------------------------------

/// The canonical on-disk name for a learning. Derived from its id so it is stable and unique.
fn canonical_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("learning-{}.md", short(id)))
}

fn short(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("md")
}

/// Render a learning to markdown with the frontmatter the parser round-trips.
fn render(l: &Learning) -> String {
    format!(
        "---\nid: {}\ntitle: {}\ntags: [{}]\nauthor: {}\ncreated: {}\n---\n{}\n",
        l.id,
        l.title,
        l.tags.join(", "),
        l.author,
        l.created,
        l.body.trim_end(),
    )
}

/// What we recover from a markdown file. `id`/`created` are intentionally absent — `add`
/// recomputes the content id, so they never need to survive the round trip.
struct Parsed {
    title: String,
    body: String,
    tags: Vec<String>,
    author: String,
}

/// Parse a markdown file into its learning fields. Understands the frontmatter we write, and
/// degrades gracefully for PAI-authored files that have no (or different) frontmatter.
fn parse(raw: &str, path: &Path) -> Parsed {
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            let header = &rest[..end];
            let body = &rest[end + "\n---\n".len()..];

            let mut title = None;
            let mut tags = Vec::new();
            let mut author = None;
            for line in header.lines() {
                if let Some(v) = line.strip_prefix("title:") {
                    title = Some(v.trim().to_string());
                } else if let Some(v) = line.strip_prefix("tags:") {
                    tags = parse_tags(v);
                } else if let Some(v) = line.strip_prefix("author:") {
                    author = Some(v.trim().to_string());
                }
            }

            return Parsed {
                title: title.filter(|t| !t.is_empty()).unwrap_or_else(|| stem(path)),
                body: body.trim_end().to_string(),
                tags,
                author: author.filter(|a| !a.is_empty()).unwrap_or_else(|| "pai".into()),
            };
        }
    }

    // No usable frontmatter: treat the whole file as the body and infer a title.
    Parsed {
        title: first_heading(raw).unwrap_or_else(|| stem(path)),
        body: raw.trim_end().to_string(),
        tags: Vec::new(),
        author: "pai".into(),
    }
}

/// Parse a `tags:` value, tolerating both `[a, b]` and bare `a, b`.
fn parse_tags(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// The first `# heading` text in a file, if any.
fn first_heading(raw: &str) -> Option<String> {
    raw.lines()
        .find_map(|line| line.strip_prefix("# ").map(|h| h.trim().to_string()))
        .filter(|h| !h.is_empty())
}

/// A file's stem (`learning-abc123.md` → `learning-abc123`), as a last-resort title.
fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into())
}

/// Map every learning currently encoded on disk to the file that holds it, keyed by the
/// content id its (title, body) would produce. Used to decide what NOT to re-export.
async fn ids_on_disk(dir: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return out;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if !is_markdown(&path) {
            continue;
        }
        if let Ok(raw) = tokio::fs::read_to_string(&path).await {
            let parsed = parse(&raw, &path);
            let id = Learning::content_id(&parsed.title, &parsed.body);
            out.entry(id).or_insert(path);
        }
    }
    out
}
