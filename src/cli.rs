//! CLI argument parsing and command definitions.
//!
//! This module uses `clap` to define the command-line interface for Pinner.
//! It includes the main [`Cli`] struct and the [`Commands`] enum for subcommands.

use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Strategy for upgrading actions to newer versions.
///
/// It determines which tags are considered "newer" during an upgrade operation.
#[derive(ValueEnum, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpgradeStrategy {
    /// Upgrade to the latest available version (default).
    Latest,
    /// Upgrade only within the current major version (e.g., v1.x.x -> v1.y.y).
    /// This follows semver and avoids breaking changes.
    Major,
    /// Upgrade only within the current minor version (e.g., v1.1.x -> v1.1.y).
    /// This is the most conservative upgrade strategy.
    Minor,
    /// Upgrade to the latest commit on the default branch (ignoring tags).
    Commit,
}

/// Format for the output results.
#[derive(ValueEnum, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Standard text output with colors and diffs (default).
    #[default]
    Text,
    /// Machine-readable JSON format.
    Json,
    /// Markdown table format suitable for PR comments.
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
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,
    /// Print verbose output for debugging.
    #[arg(short, long, global = true, conflicts_with = "quiet")]
    pub verbose: bool,
    /// Disable persistent disk caching.
    #[arg(long, global = true, env = "PINNER_NO_CACHE")]
    pub no_cache: bool,
    /// Force offline mode, preventing any network requests.
    #[arg(long, global = true, env = "PINNER_OFFLINE")]
    pub offline: bool,

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
    /// CircleCI API Token for authentication.
    #[arg(long, global = true, env = "CIRCLECI_TOKEN")]
    pub circleci_token: Option<String>,
    /// Output results in the specified format.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
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
    /// Base URL for the CircleCI GraphQL API.
    #[arg(
        long,
        global = true,
        env = "PINNER_CIRCLECI_URL",
        default_value = "https://circleci.com/graphql-unstable"
    )]
    pub circleci_url: String,

    /// Number of concurrent API requests to make.
    #[arg(long, global = true, env = "PINNER_CONCURRENCY")]
    pub concurrency: Option<usize>,
    /// Actions or images to ignore (e.g., "actions/checkout").
    #[arg(long, global = true, env = "PINNER_IGNORE", value_delimiter = ',')]
    pub ignore: Vec<String>,

    /// Username for OCI registry authentication.
    #[arg(
        long,
        global = true,
        env = "PINNER_OCI_USERNAME",
        requires = "oci_password"
    )]
    pub oci_username: Option<String>,
    /// Password for OCI registry authentication.
    #[arg(
        long,
        global = true,
        env = "PINNER_OCI_PASSWORD",
        requires = "oci_username"
    )]
    pub oci_password: Option<String>,
}

impl Cli {
    /// Returns true if the quiet flag is set.
    pub fn quiet(&self) -> bool {
        self.quiet
    }
}

/// Subcommands for the Pinner CLI.
#[derive(Subcommand, Debug, PartialEq, Clone)]
pub enum Commands {
    /// Pin all actions to their current commit SHAs.
    Pin,
    /// Upgrade all actions to their latest releases.
    Upgrade {
        /// Interactively select which actions to upgrade.
        #[arg(short, long)]
        interactive: bool,
        /// Strategy to use when upgrading actions.
        #[arg(long, env = "PINNER_UPGRADE_STRATEGY", default_value = "latest")]
        upgrade_strategy: UpgradeStrategy,
    },
    /// Verify that all actions are pinned to commit SHAs.
    Verify {
        /// Also check the OSV database for known vulnerabilities and compromised hashes during verification.
        #[arg(long, env = "PINNER_CHECK_OSV", conflicts_with = "offline")]
        check_osv: bool,
        /// Fail verification if any dependency is not explicitly vetted in the configuration.
        #[arg(long, env = "PINNER_STRICT")]
        strict: bool,
    },
    /// Set a specific action to a specific commit SHA.
    Set {
        /// Action name (e.g., actions/checkout)
        action: String,
        /// Commit SHA-1 hash
        hash: String,
    },
    /// Install a pre-commit hook that runs pinner verify.
    InstallHook,
    /// Automatically initialize pinner configuration for this repository.
    Init,
    /// Export a Software Bill of Materials (SBOM) for all dependencies in the workflows.
    ExportSbom,
    /// Scan workflows and query OSV to identify compromised dependencies, updating .pinner.toml.
    Scan {
        /// Strategy to use when upgrading actions.
        #[arg(long, env = "PINNER_UPGRADE_STRATEGY", default_value = "latest")]
        upgrade_strategy: UpgradeStrategy,
    },
    /// Generate shell completions.
    GenerateCompletion {
        /// Shell to generate completions for. If omitted, attempts to detect from the SHELL environment variable.
        shell: Option<clap_complete::Shell>,
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
    }

    #[test]
    fn test_cli_verify() {
        let cli = Cli::try_parse_from(["pinner", "verify"]).unwrap();
        assert_eq!(
            cli.command,
            Commands::Verify {
                check_osv: false,
                strict: false
            }
        );
    }
    #[test]
    fn test_cli_flags() {
        let cli = Cli::try_parse_from(["pinner", "-y", "-q", "--dry-run", "pin"]).unwrap();
        assert_eq!(cli.command, Commands::Pin);
        assert!(cli.yes);
        assert!(cli.quiet);
        assert!(cli.dry_run);
    }

    #[test]
    fn test_cli_upgrade() {
        let cli = Cli::try_parse_from(["pinner", "upgrade"]).unwrap();
        assert_eq!(
            cli.command,
            Commands::Upgrade {
                interactive: false,
                upgrade_strategy: UpgradeStrategy::Latest
            }
        );
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
            no_cache: false,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            oci_username: None,
            oci_password: None,
            format: OutputFormat::Text,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            concurrency: None,
            ignore: vec![],
        };
        assert!(cli.quiet());
        assert_eq!(cli.format, OutputFormat::Text);
        assert!(!cli.offline);
    }

    #[test]
    fn test_cli_offline() {
        let cli = Cli::try_parse_from(["pinner", "--offline", "pin"]).unwrap();
        assert!(cli.offline);
    }

    #[test]
    fn test_cli_verify_options() {
        let cli = Cli::try_parse_from(["pinner", "verify", "--check-osv", "--strict"]).unwrap();
        assert_eq!(
            cli.command,
            Commands::Verify {
                check_osv: true,
                strict: true,
            }
        );
    }

    #[test]
    fn test_cli_quiet_verbose_conflict() {
        let res = Cli::try_parse_from(["pinner", "--quiet", "--verbose", "pin"]);
        assert!(res.is_err());
    }

    #[test]
    fn test_cli_oci_username_requires_password() {
        let res = Cli::try_parse_from(["pinner", "--oci-username", "foo", "pin"]);
        assert!(res.is_err());
    }

    #[test]
    fn test_cli_oci_password_requires_username() {
        let res = Cli::try_parse_from(["pinner", "--oci-password", "bar", "pin"]);
        assert!(res.is_err());
    }

    #[test]
    fn test_cli_oci_both_ok() {
        let cli = Cli::try_parse_from([
            "pinner",
            "--oci-username",
            "foo",
            "--oci-password",
            "bar",
            "pin",
        ])
        .unwrap();
        assert_eq!(cli.oci_username, Some("foo".to_string()));
        assert_eq!(cli.oci_password, Some("bar".to_string()));
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
