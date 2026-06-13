//! # Pinner CLI
//!
//! The CLI wrapper for the `pinner` library.

use anyhow::Context;
use clap::Parser;
use pinner::{run, Cli, ReqwestGithubProvider};
use std::path::{Path, PathBuf};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Initialize the default GitHub provider
    let github =
        ReqwestGithubProvider::new("https://api.github.com".to_string(), cli.token.clone());

    // Determine the workflows directory to process
    let workflows_to_process = get_workflows(&cli.workflows);

    // Execute the requested command
    run(cli, github, workflows_to_process)
        .await
        .context("Failed to run pinner")?;

    Ok(())
}

pub fn get_workflows(cli_workflows: &[PathBuf]) -> Vec<PathBuf> {
    if cli_workflows.is_empty() {
        vec![Path::new(".github/workflows").to_path_buf()]
    } else {
        cli_workflows.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_workflows() {
        assert_eq!(get_workflows(&[]), vec![PathBuf::from(".github/workflows")]);
        assert_eq!(
            get_workflows(&[PathBuf::from("dir")]),
            vec![PathBuf::from("dir")]
        );
    }
}
