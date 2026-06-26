//! Phase 2/3 — the learnings store (the "librarian").
//!
//! Defines what a `Learning` is, files it into the shared notebook (the iroh-docs `Doc`)
//! keyed by a content fingerprint, reads learnings back, and can notify us when new ones
//! arrive. Phase 3 adds opening the *same* notebook across separate CLI runs (`open`).

use std::str::FromStr;

use anyhow::{Context, Result, anyhow, ensure};
use bytes::Bytes;
use iroh_docs::{
    AuthorId, DocTicket, NamespaceId,
    api::{Doc, protocol::AddrInfoOptions, protocol::ShareMode},
    engine::LiveEvent,
    store::Query,
    sync::Entry,
};
use n0_future::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::node::Iroh;

/// Soft cap on a single learning's JSON size, to keep records reasonable.
const MAX_LEARNING_SIZE: usize = 64 * 1024;

/// One learning — a single page in the shared notebook.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Learning {
    /// Content fingerprint: `blake3(title + "\n" + body)`. Same content → same id.
    pub id: String,
    pub title: String,
    /// The full note, in markdown.
    pub body: String,
    pub tags: Vec<String>,
    /// Who wrote it.
    pub author: String,
    /// Seconds since the Unix epoch.
    pub created: u64,
    /// Tombstone flag — how we mark a learning "deleted" in a store that can't truly erase.
    #[serde(default)]
    pub is_delete: bool,
}

impl Learning {
    /// Build a new learning, computing its content-fingerprint id.
    pub fn new(title: String, body: String, tags: Vec<String>, author: String) -> Self {
        let id = Self::content_id(&title, &body);
        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self { id, title, body, tags, author, created, is_delete: false }
    }

    /// The content fingerprint used as the key. Identical content yields an identical id,
    /// which is what gives us free de-duplication and an immutable, conflict-free record.
    pub fn content_id(title: &str, body: &str) -> String {
        let mut input = String::with_capacity(title.len() + body.len() + 1);
        input.push_str(title);
        input.push('\n');
        input.push_str(body);
        blake3::hash(input.as_bytes()).to_hex().to_string()
    }

    fn from_bytes(bytes: Bytes) -> Result<Self> {
        serde_json::from_slice(&bytes).context("invalid learning json")
    }

    fn as_bytes(&self) -> Result<Bytes> {
        let buf = serde_json::to_vec(self)?;
        ensure!(buf.len() < MAX_LEARNING_SIZE, "learning too large");
        Ok(buf.into())
    }
}

/// The store: wraps one shared notebook (`Doc`) and files/reads `Learning`s in it.
pub struct Learnings {
    iroh: Iroh,
    doc: Doc,
    author: AuthorId,
}

impl Learnings {
    /// Create a brand-new shared notebook (the first peer / `pair create`).
    pub async fn create(iroh: Iroh) -> Result<Self> {
        let doc = iroh.docs().create().await?;
        Self::with_doc(iroh, doc).await
    }

    /// Join a teammate's notebook using their ticket (`pair join <ticket>`).
    pub async fn join(iroh: Iroh, ticket: &str) -> Result<Self> {
        let ticket = DocTicket::from_str(ticket).context("invalid ticket")?;
        let doc = iroh.docs().import(ticket).await?;
        Self::with_doc(iroh, doc).await
    }

    /// Reopen an already-known notebook by id (used by `add` / `list` / `watch`).
    pub async fn open(iroh: Iroh, id: NamespaceId) -> Result<Self> {
        let doc = iroh
            .docs()
            .open(id)
            .await?
            .ok_or_else(|| anyhow!("notebook {id} not found — run `pair create` or `pair join` first"))?;
        Self::with_doc(iroh, doc).await
    }

    async fn with_doc(iroh: Iroh, doc: Doc) -> Result<Self> {
        // Reuse the node's stable default author so writes are attributed consistently.
        let author = iroh.docs().author_default().await?;
        Ok(Self { iroh, doc, author })
    }

    /// This notebook's id — what we save locally to reopen it later.
    pub fn id(&self) -> NamespaceId {
        self.doc.id()
    }

    /// A write-share ticket — the string you hand a teammate so they can join.
    ///
    /// `RelayAndAddresses` bakes our relay URL **and** direct addresses into the ticket. The
    /// default (`Id`) embeds only the NodeId and leans entirely on iroh-DNS address lookup to
    /// find us — which fails the moment that lookup is unavailable, leaving the joiner with no
    /// way to reach us. Including addresses is what iroh-docs' own sync tests do.
    pub async fn share_ticket(&self) -> Result<String> {
        let ticket = self
            .doc
            .share(ShareMode::Write, AddrInfoOptions::RelayAndAddresses)
            .await?;
        Ok(ticket.to_string())
    }

    /// Begin actively syncing this notebook with `peers` (dialing them and accepting from them).
    ///
    /// Critically, **opening a doc does not start sync** — only `import` (the join path) does, and
    /// that state does not survive a process restart. So every long-running command (`watch`,
    /// `bridge`) must call this after opening, or it will sit idle and never connect to anyone.
    /// Passing the peer's full address (not just its id) lets us connect without a relay or DNS.
    pub async fn start_sync(&self, peers: Vec<iroh::EndpointAddr>) -> Result<()> {
        self.doc.start_sync(peers).await?;
        Ok(())
    }

    /// Notifies on every change, including learnings that arrive from the teammate.
    pub async fn subscribe(&self) -> Result<impl Stream<Item = Result<LiveEvent>> + use<>> {
        self.doc.subscribe().await
    }

    /// File a new learning into the notebook. Returns the stored record.
    pub async fn add(
        &self,
        title: String,
        body: String,
        tags: Vec<String>,
        author: String,
    ) -> Result<Learning> {
        let learning = Learning::new(title, body, tags, author);
        let content = learning.as_bytes()?;
        // key = the content fingerprint; value = the JSON record.
        self.doc
            .set_bytes(self.author, learning.id.as_bytes().to_vec(), content)
            .await?;
        Ok(learning)
    }

    /// All current (non-tombstoned) learnings, oldest first.
    pub async fn list(&self) -> Result<Vec<Learning>> {
        let entries = self.doc.get_many(Query::single_latest_per_key()).await?;
        let entries = entries.collect::<Vec<Result<Entry>>>().await;

        let mut out = Vec::new();
        for entry in entries.into_iter().flatten() {
            if let Some(learning) = self.learning_from_entry(&entry).await? {
                if !learning.is_delete {
                    out.push(learning);
                }
            }
        }
        out.sort_by_key(|l| l.created);
        Ok(out)
    }

    /// Every learning, including tombstoned ones — used by the bridge so it can mirror a
    /// delete to disk. Like `list`, but does not drop `is_delete` records.
    pub async fn list_all(&self) -> Result<Vec<Learning>> {
        let entries = self.doc.get_many(Query::single_latest_per_key()).await?;
        let entries = entries.collect::<Vec<Result<Entry>>>().await;

        let mut out = Vec::new();
        for entry in entries.into_iter().flatten() {
            if let Some(learning) = self.learning_from_entry(&entry).await? {
                out.push(learning);
            }
        }
        out.sort_by_key(|l| l.created);
        Ok(out)
    }

    /// Fetch one learning by id, if present.
    pub async fn get(&self, id: &str) -> Result<Option<Learning>> {
        let entry = self
            .doc
            .get_one(Query::single_latest_per_key().key_exact(id))
            .await?;
        match entry {
            Some(entry) => self.learning_from_entry(&entry).await,
            None => Ok(None),
        }
    }

    /// Read the JSON content behind an entry. Returns `None` if the content isn't available
    /// locally yet (can happen briefly after pairing, when metadata syncs before content).
    async fn learning_from_entry(&self, entry: &Entry) -> Result<Option<Learning>> {
        match self.iroh.blobs().get_bytes(entry.content_hash()).await {
            Ok(bytes) => Ok(Some(Learning::from_bytes(bytes)?)),
            Err(_) => Ok(None),
        }
    }
}
