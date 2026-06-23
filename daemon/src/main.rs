//! learnings-daemon.
//!
//! Phase 1: boot the iroh node and print this machine's NodeId.
//! Phase 2: `selftest` files a sample learning and reads it back (proves the store works).

mod learnings;
mod node;

use std::path::PathBuf;

use anyhow::Result;

use learnings::Learnings;
use node::Iroh;

#[tokio::main]
async fn main() -> Result<()> {
    // All node data (identity + stored content) lives under ./data
    let data_dir = PathBuf::from("data");
    println!("Starting learnings-daemon node (data dir: {}) ...", data_dir.display());

    let node = Iroh::new(data_dir).await?;

    println!();
    println!("Node is up.");
    println!("  NodeId: {}", node.endpoint_id());

    if std::env::args().any(|a| a == "selftest") {
        run_selftest(node.clone()).await?;
    } else {
        println!();
        println!("  (run with `selftest` to file a sample learning — the Phase 2 demo)");
        println!("  (pairing with a teammate comes in Phase 3.)");
    }

    node.shutdown().await?;
    Ok(())
}

/// Phase 2 demonstration: file a learning, read it back, and show de-duplication.
async fn run_selftest(node: Iroh) -> Result<()> {
    println!();
    println!("=== Phase 2 self-test: file a learning, then read it back ===");

    // No ticket → start a fresh notebook just for this demo.
    let store = Learnings::new(None, node).await?;

    let title = "Always use the X helper, never bare Y".to_string();
    let body = "Bare Y skips validation and caused a bug. The X helper validates first.".to_string();
    let tags = vec!["gotcha".to_string(), "api".to_string()];

    let learning = store
        .add(title.clone(), body.clone(), tags.clone(), "harshitha".to_string())
        .await?;
    println!();
    println!("Filed a learning. Its id is a fingerprint of the content:");
    println!("  {}", learning.id);

    let all = store.list().await?;
    println!();
    println!("The notebook now contains {} learning(s):", all.len());
    for l in &all {
        println!("{}", serde_json::to_string_pretty(l)?);
    }

    // Add the SAME content again → same fingerprint id → still one learning (dedup).
    let again = store.add(title, body, tags, "harshitha".to_string()).await?;
    let count = store.list().await?.len();
    println!();
    println!(
        "Re-added identical content → same id? {} → notebook still holds {} learning(s) (dedup works).",
        again.id == learning.id,
        count
    );

    Ok(())
}
