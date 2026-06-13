//! # Pinner CLI
//!
//! The CLI wrapper for the `pinner` library.

use anyhow::Context;
use clap::{CommandFactory, Parser};
use pinner::{run, Cli, OciRegistryProvider, Operations, ReqwestGithubProvider};
use std::path::{Path, PathBuf};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    run_app(cli).await
}

pub async fn run_app(cli: Cli) -> anyhow::Result<()> {
    if let pinner::Commands::GenerateCompletion { shell } = cli.command {
        let mut cmd = Cli::command();
        clap_complete::generate(shell, &mut cmd, "pinner", &mut std::io::stdout());
        return Ok(());
    }

    // Initialize tracing (only if not already initialized by tests)
    let filter = if cli.quiet {
        EnvFilter::new("off")
    } else if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    let _ = tracing_subscriber::registry()
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(filter)
        .try_init();

    // Initialize the default GitHub provider
    let github_url = cli
        .github_url
        .clone()
        .or_else(|| {
            Operations::<ReqwestGithubProvider, OciRegistryProvider>::load_config_from_path(
                std::path::Path::new(".pinner.toml"),
            )
            .ok()
            .and_then(|c| c.github_url)
        })
        .unwrap_or_else(|| "https://api.github.com".to_string());

    let github = ReqwestGithubProvider::new(github_url, cli.token.clone());
    let registry = OciRegistryProvider::new();

    // Determine the workflows directory to process
    let workflows_to_process = get_workflows(&cli.workflows);

    // Execute the requested command
    run(cli, github, registry, workflows_to_process)
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

    #[tokio::test]
    async fn test_run_app_completion() {
        let cli = Cli::try_parse_from(["pinner", "generate-completion", "bash"]).unwrap();
        // This will print to stdout, but should cover the code path
        run_app(cli).await.unwrap();
    }

    #[test]
    fn test_get_workflows() {
        assert_eq!(get_workflows(&[]), vec![PathBuf::from(".github/workflows")]);
        assert_eq!(
            get_workflows(&[PathBuf::from("dir")]),
            vec![PathBuf::from("dir")]
        );
    }

    #[tokio::test]
    async fn test_run_app_quiet() {
        let cli = Cli::try_parse_from(["pinner", "-q", "verify"]).unwrap();
        let _ = run_app(cli).await;
    }

    #[tokio::test]
    async fn test_run_app_verbose() {
        let cli = Cli::try_parse_from(["pinner", "-v", "verify"]).unwrap();
        let _ = run_app(cli).await;
    }
}
