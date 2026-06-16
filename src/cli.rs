//! CLI argument parsing and command definitions.
//!
//! This module uses `clap` to define the command-line interface for Pinner.
//! It includes the main [`Cli`] struct and the [`Commands`] enum for subcommands.

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Strategy for upgrading actions to newer versions.
#[derive(ValueEnum, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpgradeStrategy {
    /// Upgrade to the latest available version (default).
    Latest,
    /// Upgrade only within the current major version (e.g., v1.x.x -> v1.y.y).
    Major,
    /// Upgrade only within the current minor version (e.g., v1.1.x -> v1.1.y).
    Minor,
    /// Upgrade to the latest commit on the default branch.
    Commit,
}

/// Format for the output results.
#[derive(ValueEnum, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Standard text output (default).
    #[default]
    Text,
    /// JSON format.
    Json,
    /// Markdown table format.
    Markdown,
}

/// The main command-line interface for Pinner.
#[derive(Parser, Clone)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Subcommand to execute.
    #[command(subcommand)]
    pub command: Commands,
    /// Workflow files or directories to process.
    #[arg(
        short,
        long,
        global = true,
        help = "Workflow files or directories to process"
    )]
    pub workflows: Vec<PathBuf>,
    /// Automatically confirm all replacements without prompting.
    #[arg(short, long, global = true)]
    pub yes: bool,
    /// Suppress all console output except for critical errors.
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Print verbose output for debugging.
    #[arg(short, long, global = true)]
    pub verbose: bool,
    /// Print what would be changed without actually modifying any files.
    #[arg(short, long, global = true)]
    pub dry_run: bool,
    /// GitHub API Token for authentication.
    #[arg(long, global = true, env = "GITHUB_TOKEN")]
    pub github_token: Option<String>,
    /// Bitbucket API Token for authentication.
    #[arg(long, global = true, env = "BITBUCKET_TOKEN")]
    pub bitbucket_token: Option<String>,
    /// GitLab API Token for authentication.
    #[arg(long, global = true, env = "GITLAB_TOKEN")]
    pub gitlab_token: Option<String>,
    /// Forgejo/Gitea API Token for authentication.
    #[arg(long, global = true, env = "FORGEJO_TOKEN")]
    pub forgejo_token: Option<String>,
    /// Output results in the specified format.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
    /// Output results in JSON format (deprecated, use --format json).
    #[arg(long, global = true)]
    pub json: bool,
    /// Base URL for the GitHub API (defaults to public GitHub).
    #[arg(
        long,
        global = true,
        env = "PINNER_GITHUB_URL",
        default_value = "https://api.github.com"
    )]
    pub github_url: String,
    /// Base URL for the Bitbucket API.
    #[arg(
        long,
        global = true,
        env = "PINNER_BITBUCKET_URL",
        default_value = "https://api.bitbucket.org/2.0"
    )]
    pub bitbucket_url: String,
    /// Base URL for the GitLab API.
    #[arg(
        long,
        global = true,
        env = "PINNER_GITLAB_URL",
        default_value = "https://gitlab.com"
    )]
    pub gitlab_url: String,
    /// Base URL for the Forgejo/Gitea API.
    #[arg(
        long,
        global = true,
        env = "PINNER_FORGEJO_URL",
        default_value = "https://codeberg.org"
    )]
    pub forgejo_url: String,
    /// Strategy to use when upgrading actions.
    #[arg(
        long,
        global = true,
        env = "PINNER_UPGRADE_STRATEGY",
        default_value = "latest"
    )]
    pub upgrade_strategy: UpgradeStrategy,
    /// Number of concurrent API requests to make.
    #[arg(long, global = true, env = "PINNER_CONCURRENCY")]
    pub concurrency: Option<usize>,
    /// Actions or images to ignore (e.g., "actions/checkout").
    #[arg(long, global = true, env = "PINNER_IGNORE", value_delimiter = ',')]
    pub ignore: Vec<String>,
    /// Username for OCI registry authentication.
    #[arg(long, global = true, env = "PINNER_OCI_USERNAME")]
    pub oci_username: Option<String>,
    /// Password for OCI registry authentication.
    #[arg(long, global = true, env = "PINNER_OCI_PASSWORD")]
    pub oci_password: Option<String>,
}

impl Cli {
    /// Returns true if the quiet flag is set.
    pub fn quiet(&self) -> bool {
        self.quiet
    }

    /// Returns the effective output format, considering the deprecated --json flag.
    pub fn output_format(&self) -> OutputFormat {
        if self.json {
            OutputFormat::Json
        } else {
            self.format.clone()
        }
    }
}

/// Subcommands for the Pinner CLI.
#[derive(Subcommand, Debug, PartialEq, Clone)]
pub enum Commands {
    /// Pin all actions to their current commit SHAs.
    Pin,
    /// Upgrade all actions to their latest releases.
    Upgrade,
    /// Verify that all actions are pinned to commit SHAs.
    Verify,
    /// Set a specific action to a specific commit SHA.
    Set {
        /// Action name (e.g., actions/checkout)
        action: String,
        /// Commit SHA-1 hash
        hash: String,
    },
    /// Install a pre-commit hook that runs pinner verify.
    InstallHook,
    /// Generate shell completions.
    GenerateCompletion {
        /// Shell to generate completions for
        shell: clap_complete::Shell,
    },
}
#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_cli_pin_basic() {
        let cli = Cli::try_parse_from(["pinner", "pin"]).unwrap();
        assert_eq!(cli.command, Commands::Pin);
        assert!(!cli.yes);
        assert!(!cli.quiet());
        assert!(!cli.dry_run);
        assert!(!cli.json);
    }

    #[test]
    fn test_cli_verify() {
        let cli = Cli::try_parse_from(["pinner", "verify"]).unwrap();
        assert_eq!(cli.command, Commands::Verify);
    }
    #[test]
    fn test_cli_flags() {
        let cli =
            Cli::try_parse_from(["pinner", "-y", "-q", "--dry-run", "--json", "pin"]).unwrap();
        assert_eq!(cli.command, Commands::Pin);
        assert!(cli.yes);
        assert!(cli.quiet);
        assert!(cli.dry_run);
        assert!(cli.json);
    }

    #[test]
    fn test_cli_upgrade() {
        let cli = Cli::try_parse_from(["pinner", "upgrade"]).unwrap();
        assert_eq!(cli.command, Commands::Upgrade);
    }

    #[test]
    fn test_cli_set() {
        let cli = Cli::try_parse_from([
            "pinner",
            "set",
            "actions/checkout",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        ])
        .unwrap();
        assert_eq!(
            cli.command,
            Commands::Set {
                action: "actions/checkout".into(),
                hash: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".into(),
            }
        );
    }

    #[test]
    fn test_cli_workflows() {
        let cli =
            Cli::try_parse_from(["pinner", "-w", "dir1", "--workflows", "dir2", "pin"]).unwrap();
        assert_eq!(
            cli.workflows,
            vec![PathBuf::from("dir1"), PathBuf::from("dir2")]
        );
    }

    #[test]
    fn test_cli_methods() {
        let cli = Cli {
            command: Commands::Pin,
            workflows: vec![],
            yes: false,
            quiet: true,
            verbose: false,
            dry_run: false,
            json: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            oci_username: None,
            oci_password: None,
            format: OutputFormat::Text,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            upgrade_strategy: UpgradeStrategy::Latest,
            concurrency: None,
            ignore: vec![],
        };
        assert!(cli.quiet());
        assert_eq!(cli.output_format(), OutputFormat::Text);

        let mut cli_json = cli.clone();
        cli_json.json = true;
        assert_eq!(cli_json.output_format(), OutputFormat::Json);
    }

    #[test]
    #[serial_test::serial]
    fn test_cli_token_env() {
        std::env::set_var("GITHUB_TOKEN", "test_token");
        let cli = Cli::try_parse_from(["pinner", "pin"]).unwrap();
        assert_eq!(cli.github_token, Some("test_token".to_string()));
        std::env::remove_var("GITHUB_TOKEN");
    }
}
