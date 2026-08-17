//! Phase 1 — the iroh "node".
//!
//! This turns the machine into a participant on the iroh network:
//!   * a stable cryptographic **identity** (saved to disk so it survives restarts),
//!   * an **Endpoint** (the network "phone"),
//!   * three protocols installed on it — **docs** (the shared notebook), plus **blobs**
//!     and **gossip** which docs is built on,
//!   * a **Router** (switchboard) that sends each incoming connection to the right protocol.
//!
//! Adapted almost verbatim from the `tauri-todos` example in iroh-examples.
//! Named `node` (not `iroh`) to avoid clashing with the `iroh` library's own name.

use std::path::PathBuf;

use anyhow::{Context, Result};
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId, SecretKey};
use iroh_blobs::{api::blobs::Blobs, store::fs::FsStore, BlobsProtocol, ALPN as BLOBS_ALPN};
use iroh_docs::{protocol::Docs, ALPN as DOCS_ALPN};
use iroh_gossip::{net::Gossip, ALPN as GOSSIP_ALPN};
use tokio::io::AsyncWriteExt;

/// A running iroh node: its endpoint, blob store, and the docs protocol.
#[derive(Clone, Debug)]
pub struct Iroh {
    router: Router,
    store: FsStore,
    docs: Docs,
}

impl Iroh {
    /// Start a node, persisting everything (identity + data) under `path`.
    ///
    /// `bind_port` pins the UDP port. Leaving it `None` uses an ephemeral port (normal use).
    /// Pinning it keeps this node's address stable across restarts, which is what lets two
    /// nodes on one machine reach each other directly (over loopback/LAN) without depending on
    /// a relay — the basis of the local two-node test.
    pub async fn new(path: PathBuf, bind_port: Option<u16>) -> Result<Self> {
        // Make sure the data folder exists.
        tokio::fs::create_dir_all(&path).await?;

        // Load our permanent identity from disk, or create + save one on first run.
        let key = load_secret_key(path.join("keypair")).await?;

        // The Endpoint is the network "phone". `presets::N0` uses n0's public relays
        // so two machines can reach each other even behind home routers (NAT).
        let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::N0).secret_key(key);
        if let Some(port) = bind_port {
            builder = builder
                .bind_addr((std::net::Ipv4Addr::UNSPECIFIED, port))
                .map_err(|e| anyhow::anyhow!("invalid bind port {port}: {e:?}"))?;
        }
        let endpoint = builder.bind().await?;

        // Gossip: helps nodes find and track each other. Docs needs it underneath.
        let gossip = Gossip::builder().spawn(endpoint.clone());

        // Blobs: on-disk content storage. Docs is built on top of it.
        let blobs = FsStore::load(&path).await?;

        // Docs: the multi-writer key/value store that auto-syncs between peers.
        let docs = Docs::persistent(path)
            .spawn(endpoint.clone(), (*blobs).clone(), gossip.clone())
            .await?;

        // The Router is the switchboard: it reads each incoming connection's label (ALPN)
        // and hands it to the matching protocol.
        let router = Router::builder(endpoint.clone())
            .accept(BLOBS_ALPN, BlobsProtocol::new(&blobs, None))
            .accept(GOSSIP_ALPN, gossip)
            .accept(DOCS_ALPN, docs.clone())
            .spawn();

        Ok(Self {
            router,
            docs,
            store: blobs,
        })
    }

    /// The network endpoint (the "phone"). Used by the HTTP status route in Phase 6.
    #[allow(dead_code)]
    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    /// This node's permanent public identity — its "phone number" on the network.
    pub fn endpoint_id(&self) -> EndpointId {
        self.router.endpoint().id()
    }

    /// Try to connect to a relay so far-apart teammates (behind separate NATs) can reach each
    /// other and hole-punch. We wait, but only up to a few seconds: where the relay is
    /// unreachable (UDP-restricted networks/CI), nodes on the same machine or LAN still reach
    /// each other over the direct addresses already in the ticket, so we proceed instead of
    /// hanging forever.
    pub async fn online(&self) {
        let wait = std::time::Duration::from_secs(10);
        if tokio::time::timeout(wait, self.router.endpoint().online())
            .await
            .is_err()
        {
            eprintln!("(no relay yet — continuing with direct addresses)");
        }
    }

    /// The docs protocol — used from Phase 2 onward to store/sync learnings.
    pub fn docs(&self) -> &Docs {
        &self.docs
    }

    /// The blob store (content lives here under the hood).
    #[allow(dead_code)]
    pub fn blobs(&self) -> &Blobs {
        self.store.blobs()
    }

    /// Cleanly shut the node down.
    pub async fn shutdown(self) -> Result<()> {
        self.router.shutdown().await?;
        Ok(())
    }
}

/// Load the node's secret key from `key_path`, or generate + save a new one on first run.
///
/// Saving it is what makes our identity *stable*: without this, the node would invent a
/// brand-new identity on every restart and our teammate would no longer recognize us.
pub async fn load_secret_key(key_path: PathBuf) -> Result<SecretKey> {
    if key_path.exists() {
        let key_bytes = tokio::fs::read(key_path).await?;
        let secret_key = SecretKey::try_from(&key_bytes[0..32])?;
        Ok(secret_key)
    } else {
        let secret_key = SecretKey::generate();

        let key_path = key_path.canonicalize().unwrap_or(key_path);
        let key_path_parent = key_path.parent().ok_or_else(|| {
            anyhow::anyhow!("no parent directory found for '{}'", key_path.display())
        })?;
        tokio::fs::create_dir_all(&key_path_parent).await?;

        // Write to a temp file first, then rename — avoids a half-written key file.
        let (file, temp_file_path) = tempfile::NamedTempFile::new_in(key_path_parent)
            .context("unable to create tempfile")?
            .into_parts();
        let mut file = tokio::fs::File::from_std(file);
        file.write_all(&secret_key.to_bytes())
            .await
            .context("unable to write keyfile")?;
        file.flush().await?;
        drop(file);

        tokio::fs::rename(temp_file_path, key_path)
            .await
            .context("failed to rename keyfile")?;

        Ok(secret_key)
    }
}
