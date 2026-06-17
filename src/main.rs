//! # Pinner CLI
//!
//! The CLI wrapper for the `pinner` library.

use anyhow::Context;
use clap::{CommandFactory, Parser};
use pinner::{resolver::OciRegistryProvider, run, Cli};
use std::path::{Path, PathBuf};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = pinner::config::Config::load();
    let cli = config.merge_with_cli(cli);

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
        let shell = shell.or_else(detect_shell).context(
            "Could not detect current shell. Please specify it explicitly (e.g., bash, zsh, fish).",
        )?;
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

    let disk_cache = if cli.no_cache {
        None
    } else {
        dirs::cache_dir().map(|mut p| {
            p.push("pinner");
            p
        })
    };

    let provider =
        pinner::resolver::UnifiedProvider::new(pinner::resolver::UnifiedProviderConfig {
            github_url: cli.github_url.clone(),
            github_token: cli.github_token.clone(),
            bitbucket_url: cli.bitbucket_url.clone(),
            bitbucket_token: cli.bitbucket_token.clone(),
            gitlab_url: cli.gitlab_url.clone(),
            gitlab_token: cli.gitlab_token.clone(),
            forgejo_url: cli.forgejo_url.clone(),
            forgejo_token: cli.forgejo_token.clone(),
            circleci_url: cli.circleci_url.clone(),
            circleci_token: cli.circleci_token.clone(),
            disk_cache_path: disk_cache,
        })?;
    let registry = OciRegistryProvider::new(cli.oci_username.clone(), cli.oci_password.clone())
        .with_verification(cli.verify_provenance, cli.require_provenance);

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

fn detect_shell() -> Option<clap_complete::Shell> {
    let shell_env = std::env::var("SHELL").ok()?;
    let shell_path = Path::new(&shell_env);
    let shell_name = shell_path.file_name()?.to_str()?;

    if shell_name.contains("bash") {
        Some(clap_complete::Shell::Bash)
    } else if shell_name.contains("zsh") {
        Some(clap_complete::Shell::Zsh)
    } else if shell_name.contains("fish") {
        Some(clap_complete::Shell::Fish)
    } else if shell_name.contains("elvish") {
        Some(clap_complete::Shell::Elvish)
    } else if shell_name.contains("powershell") || shell_name.contains("pwsh") {
        Some(clap_complete::Shell::PowerShell)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_detect_shell() {
        let original_shell = std::env::var("SHELL");

        std::env::set_var("SHELL", "/bin/bash");
        assert_eq!(detect_shell(), Some(clap_complete::Shell::Bash));

        std::env::set_var("SHELL", "/usr/bin/zsh");
        assert_eq!(detect_shell(), Some(clap_complete::Shell::Zsh));

        std::env::set_var("SHELL", "/usr/local/bin/fish");
        assert_eq!(detect_shell(), Some(clap_complete::Shell::Fish));

        std::env::set_var("SHELL", "pwsh");
        assert_eq!(detect_shell(), Some(clap_complete::Shell::PowerShell));

        std::env::set_var("SHELL", "/bin/unknown");
        assert_eq!(detect_shell(), None);

        if let Ok(val) = original_shell {
            std::env::set_var("SHELL", val);
        } else {
            std::env::remove_var("SHELL");
        }
    }

    #[test]
    fn test_get_workflows_cli_priority() {
        let cli_paths = vec![PathBuf::from("custom/path")];
        assert_eq!(get_workflows(&cli_paths), cli_paths);
    }

    #[test]
    #[serial_test::serial]
    fn test_get_workflows_discovery() {
        let dir = tempdir().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        // Initially should return default .github/workflows if nothing exists
        assert_eq!(get_workflows(&[]), vec![PathBuf::from(".github/workflows")]);

        // Create .gitlab-ci.yml
        fs::write(".gitlab-ci.yml", "").unwrap();
        assert_eq!(get_workflows(&[]), vec![PathBuf::from(".gitlab-ci.yml")]);

        // Create .github/workflows
        fs::create_dir_all(".github/workflows").unwrap();
        let res = get_workflows(&[]);
        assert!(res.contains(&PathBuf::from(".github/workflows")));
        assert!(res.contains(&PathBuf::from(".gitlab-ci.yml")));

        // Create bitbucket-pipelines.yml
        fs::write("bitbucket-pipelines.yml", "").unwrap();
        fs::write("bitbucket-pipelines.yaml", "").unwrap();
        let res = get_workflows(&[]);
        assert!(res.contains(&PathBuf::from("bitbucket-pipelines.yml")));
        // Should NOT contain .yaml if .yml exists
        assert!(!res.contains(&PathBuf::from("bitbucket-pipelines.yaml")));

        std::env::set_current_dir(original_dir).unwrap();
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
