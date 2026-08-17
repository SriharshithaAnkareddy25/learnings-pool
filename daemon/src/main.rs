//! learnings-daemon — sync PAI learnings between teammates over iroh.
//!
//! Phase 3: a real CLI — `id`, `pair create`, `pair join`, `add`, `list`, `watch`.
//! Run `learnings-daemon --help` to see commands.

mod api;
mod bridge;
mod cli;
mod learnings;
mod node;
mod retrieval;

use anyhow::Result;
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    cli::run(cli).await
}
