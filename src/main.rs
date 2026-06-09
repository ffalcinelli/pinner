//! # Pinner CLI
//!
//! The CLI wrapper for the `pinner` library.

use anyhow::Context;
use clap::Parser;
use pinner::{run, Cli, ReqwestGithubProvider};
use std::path::Path;

#[cfg(not(tarpaulin))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Initialize the default GitHub provider
    let github = ReqwestGithubProvider::default();

    // Set the default workflows directory
    let workflows_dir = Path::new(".github/workflows");

    // Execute the requested command
    run(cli, github, workflows_dir)
        .await
        .context("Failed to run pinner")?;

    Ok(())
}
