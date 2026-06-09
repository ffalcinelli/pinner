use anyhow::Context;
use clap::Parser;
use pinner::{run, Cli, ReqwestGithubProvider};
use std::path::Path;

#[cfg(not(tarpaulin))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let github = ReqwestGithubProvider::default();
    let workflows_dir = Path::new(".github/workflows");
    run(cli, github, workflows_dir)
        .await
        .context("Failed to run pinner")?;
    Ok(())
}
