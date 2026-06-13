use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum UpgradeStrategy {
    /// Upgrade to the latest available version
    Latest,
    /// Upgrade only within the current major version
    Major,
    /// Upgrade only within the current minor version
    Minor,
    /// Upgrade to the latest commit on the default branch
    Commit,
}

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    /// Workflow files or directories to process
    #[arg(
        short,
        long,
        global = true,
        help = "Workflow files or directories to process"
    )]
    pub workflows: Vec<PathBuf>,
    /// Automatically confirm all replacements
    #[arg(short, long, global = true)]
    pub yes: bool,
    /// Suppress all console output
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Print verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,
    /// Print diff without modifying files
    #[arg(short, long, global = true)]
    pub dry_run: bool,
    /// GitHub API Token
    #[arg(short, long, global = true, env = "GITHUB_TOKEN")]
    pub token: Option<String>,
    /// Output results as JSON
    #[arg(long, global = true)]
    pub json: bool,
    /// GitHub API URL (for GitHub Enterprise)
    #[arg(long, global = true, env = "GITHUB_URL")]
    pub github_url: Option<String>,
    /// Upgrade strategy (only for upgrade command)
    #[arg(long, global = true, default_value = "latest")]
    pub upgrade_strategy: UpgradeStrategy,
}

impl Cli {
    /// Returns true if the quiet flag is set.
    pub fn quiet(&self) -> bool {
        self.quiet
    }
}

/// Subcommands for the Pinner CLI.
#[derive(Subcommand, Debug, PartialEq)]
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
        let cli = Cli::try_parse_from(&["pinner", "pin"]).unwrap();
        assert_eq!(cli.command, Commands::Pin);
        assert!(!cli.yes);
        assert!(!cli.quiet());
        assert!(!cli.dry_run);
        assert!(!cli.json);
    }

    #[test]
    fn test_cli_verify() {
        let cli = Cli::try_parse_from(&["pinner", "verify"]).unwrap();
        assert_eq!(cli.command, Commands::Verify);
    }
    #[test]
    fn test_cli_flags() {
        let cli =
            Cli::try_parse_from(&["pinner", "-y", "-q", "--dry-run", "--json", "pin"]).unwrap();
        assert_eq!(cli.command, Commands::Pin);
        assert!(cli.yes);
        assert!(cli.quiet);
        assert!(cli.dry_run);
        assert!(cli.json);
    }

    #[test]
    fn test_cli_upgrade() {
        let cli = Cli::try_parse_from(&["pinner", "upgrade"]).unwrap();
        assert_eq!(cli.command, Commands::Upgrade);
    }

    #[test]
    fn test_cli_set() {
        let cli = Cli::try_parse_from(&[
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
            Cli::try_parse_from(&["pinner", "-w", "dir1", "--workflows", "dir2", "pin"]).unwrap();
        assert_eq!(
            cli.workflows,
            vec![PathBuf::from("dir1"), PathBuf::from("dir2")]
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_cli_token_env() {
        std::env::set_var("GITHUB_TOKEN", "test_token");
        let cli = Cli::try_parse_from(&["pinner", "pin"]).unwrap();
        assert_eq!(cli.token, Some("test_token".to_string()));
        std::env::remove_var("GITHUB_TOKEN");
    }
}
