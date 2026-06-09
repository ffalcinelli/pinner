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

    // Determine the workflows directory to process
    let workflows_to_process = if cli.workflows.is_empty() {
        vec![Path::new(".github/workflows").to_path_buf()]
    } else {
        cli.workflows.clone()
    };

    // Execute the requested command
    run(cli, github, workflows_to_process)
        .await
        .context("Failed to run pinner")?;

    Ok(())
}
