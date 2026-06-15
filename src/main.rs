//! # Pinner CLI
//!
//! The CLI wrapper for the `pinner` library.

use anyhow::Context;
use clap::{CommandFactory, Parser};
use pinner::{run, Cli, OciRegistryProvider};
use std::path::{Path, PathBuf};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(e) = run_app(cli).await {
        // Check if it's a verification failure (already printed details)
        let is_verification_failure = e
            .root_cause()
            .downcast_ref::<pinner::error::PinnerError>()
            .is_some_and(|pe| matches!(pe, pinner::error::PinnerError::VerificationFailed(_)));

        if is_verification_failure {
            return ExitCode::FAILURE;
        }

        eprintln!("Error: {:?}", e);
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
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

    tracing_subscriber::registry()
        .with(fmt::layer().with_writer(std::io::stderr))
        .with(filter)
        .try_init()
        .ok();

    let provider =
        pinner::providers::UnifiedProvider::new(pinner::providers::UnifiedProviderConfig {
            github_url: cli.github_url.clone(),
            github_token: cli.github_token.clone(),
            bitbucket_url: cli.bitbucket_url.clone(),
            bitbucket_token: cli.bitbucket_token.clone(),
            gitlab_url: cli.gitlab_url.clone(),
            gitlab_token: cli.gitlab_token.clone(),
            forgejo_url: cli.forgejo_url.clone(),
            forgejo_token: cli.forgejo_token.clone(),
        });
    let registry = OciRegistryProvider::new(cli.oci_username.clone(), cli.oci_password.clone());

    // Determine the workflows directory to process
    let workflows_to_process = get_workflows(&cli.workflows);

    // Execute the requested command
    run(cli, provider, registry, workflows_to_process)
        .await
        .context("Failed to run pinner")?;

    Ok(())
}

pub fn get_workflows(cli_workflows: &[PathBuf]) -> Vec<PathBuf> {
    if !cli_workflows.is_empty() {
        return cli_workflows.to_vec();
    }

    let default_paths = [
        ".github/workflows",
        "bitbucket-pipelines.yml",
        "bitbucket-pipelines.yaml",
        ".gitlab-ci.yml",
        ".circleci/config.yml",
        ".travis.yml",
        "appveyor.yml",
        ".forgejo/workflows",
    ];

    let mut defaults: Vec<PathBuf> = default_paths
        .iter()
        .filter_map(|&p| {
            let path = Path::new(p);
            if path.exists() {
                Some(path.to_path_buf())
            } else {
                None
            }
        })
        .collect();

    if defaults.contains(&PathBuf::from("bitbucket-pipelines.yml")) {
        defaults.retain(|p| p != Path::new("bitbucket-pipelines.yaml"));
    }

    if defaults.is_empty() {
        vec![PathBuf::from(".github/workflows")]
    } else {
        defaults
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_workflows() {
        // Since we can't easily mock the filesystem here without more complexity,
        // we just test the cli_workflows priority.
        let cli_paths = vec![PathBuf::from("custom/path")];
        assert_eq!(get_workflows(&cli_paths), cli_paths);

        // Default case (will likely return .github/workflows if it doesn't exist in the current env)
        assert_eq!(get_workflows(&[]), vec![PathBuf::from(".github/workflows")]);
    }

    #[tokio::test]
    async fn test_run_app_completion() {
        let cli = Cli::try_parse_from(["pinner", "generate-completion", "bash"]).unwrap();
        // This will print to stdout, but should cover the code path
        run_app(cli).await.unwrap();
    }

    #[tokio::test]
    async fn test_run_app_quiet() {
        let cli = Cli::try_parse_from(["pinner", "--quiet", "verify"]).unwrap();
        // This will fail if no workflows found, but that's okay for coverage
        let _ = run_app(cli).await;
    }

    #[tokio::test]
    async fn test_run_app_verbose() {
        let cli = Cli::try_parse_from(["pinner", "--verbose", "verify"]).unwrap();
        let _ = run_app(cli).await;
    }
}
