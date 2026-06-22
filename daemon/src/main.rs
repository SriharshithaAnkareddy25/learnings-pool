//! learnings-daemon.
//!
//! Phase 1: boot the iroh node and print this machine's NodeId (its permanent network
//! identity). No pairing or syncing yet — that arrives in later phases.

mod node;

use std::path::PathBuf;

use anyhow::Result;

use node::Iroh;

#[tokio::main]
async fn main() -> Result<()> {
    // All node data (identity + stored content) lives under ./data
    let data_dir = PathBuf::from("data");
    println!("Starting learnings-daemon node (data dir: {}) ...", data_dir.display());

    let node = Iroh::new(data_dir).await?;

    println!();
    println!("Node is up.");
    println!("  NodeId (this machine's permanent network identity):");
    println!("    {}", node.endpoint_id());
    println!();
    println!("  This identity is saved on disk, so it stays the same across restarts.");
    println!("  (Connecting to your teammate — pairing — comes in Phase 3.)");

    node.shutdown().await?;
    Ok(())
}
