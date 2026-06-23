//! Phase 2 — the learnings store (the "librarian").
//!
//! Defines what a `Learning` is, files it into the shared notebook (the iroh-docs `Doc`)
//! keyed by a content fingerprint, reads learnings back, and can notify us when new ones
//! arrive. Adapted from the `tauri-todos` example (which manages todos the same way).

use std::str::FromStr;

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use iroh_docs::{
    AuthorId, DocTicket,
    api::{Doc, protocol::ShareMode},
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
    ticket: DocTicket,
    author: AuthorId,
}

impl Learnings {
    /// Open the shared notebook.
    /// * `ticket = None`  → create a brand-new notebook (the first peer).
    /// * `ticket = Some`  → join the teammate's existing notebook (Phase 3 uses this).
    pub async fn new(ticket: Option<String>, iroh: Iroh) -> Result<Self> {
        let author = iroh.docs().author_create().await?;

        let doc = match ticket {
            None => iroh.docs().create().await?,
            Some(ticket) => {
                let ticket = DocTicket::from_str(&ticket)?;
                iroh.docs().import(ticket).await?
            }
        };

        // A write-share ticket others can use to join this notebook.
        let ticket = doc.share(ShareMode::Write, Default::default()).await?;

        Ok(Self { iroh, doc, ticket, author })
    }

    /// The share ticket — the string you hand a teammate so they can join (Phase 3).
    pub fn ticket(&self) -> String {
        self.ticket.to_string()
    }

    /// Notifies on every change, including learnings that arrive from the teammate.
    /// Used in Phase 5 to mirror incoming learnings into PAI's KNOWLEDGE/ folder.
    #[allow(dead_code)]
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

    /// Fetch one learning by id, if present.
    #[allow(dead_code)]
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
