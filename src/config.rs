//! Configuration management for Pinner.
//!
//! This module handles loading configuration from files (e.g., `.pinner.toml`)
//! and environment variables, merging them with CLI arguments.

use figment::{
    providers::{Env, Format, Toml, Yaml},
    Figment,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::cli::{Cli, OutputFormat, UpgradeStrategy};

/// Configuration for Pinner, typically loaded from a file or environment.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Config {
    /// Workflow files or directories to process.
    pub workflows: Option<Vec<PathBuf>>,
    /// Automatically confirm all replacements without prompting.
    pub yes: Option<bool>,
    /// Suppress all console output except for critical errors.
    pub quiet: Option<bool>,
    /// Print verbose output for debugging.
    pub verbose: Option<bool>,
    /// Print what would be changed without actually modifying any files.
    pub dry_run: Option<bool>,
    /// GitHub API Token for authentication.
    pub github_token: Option<String>,
    /// Bitbucket API Token for authentication.
    pub bitbucket_token: Option<String>,
    /// GitLab API Token for authentication.
    pub gitlab_token: Option<String>,
    /// Forgejo/Gitea API Token for authentication.
    pub forgejo_token: Option<String>,
    /// Output results in the specified format.
    pub format: Option<OutputFormat>,
    /// Base URL for the GitHub API.
    pub github_url: Option<String>,
    /// Base URL for the Bitbucket API.
    pub bitbucket_url: Option<String>,
    /// Base URL for the GitLab API.
    pub gitlab_url: Option<String>,
    /// Base URL for the Forgejo/Gitea API.
    pub forgejo_url: Option<String>,
    /// Strategy to use when upgrading actions.
    pub upgrade_strategy: Option<UpgradeStrategy>,
    /// Number of concurrent API requests to make.
    pub concurrency: Option<usize>,
    /// Actions or images to ignore.
    pub ignore: Option<Vec<String>>,
    /// Username for OCI registry authentication.
    pub oci_username: Option<String>,
    /// Password for OCI registry authentication.
    pub oci_password: Option<String>,
}

impl Config {
    /// Loads configuration from default locations and environment variables.
    pub fn load() -> Self {
        Figment::new()
            .merge(Toml::file(".pinner.toml"))
            .merge(Yaml::file(".pinner.yaml"))
            .merge(Yaml::file(".pinner.yml"))
            .merge(Env::prefixed("PINNER_"))
            .extract()
            .unwrap_or_else(|_| Config::default())
    }

    /// Merges this configuration with CLI arguments, with CLI taking precedence.
    pub fn merge_with_cli(self, mut cli: Cli) -> Cli {
        if cli.workflows.is_empty() {
            if let Some(workflows) = self.workflows {
                cli.workflows = workflows;
            }
        }

        if !cli.yes {
            if let Some(yes) = self.yes {
                cli.yes = yes;
            }
        }

        if !cli.quiet {
            if let Some(quiet) = self.quiet {
                cli.quiet = quiet;
            }
        }

        if !cli.verbose {
            if let Some(verbose) = self.verbose {
                cli.verbose = verbose;
            }
        }

        if !cli.dry_run {
            if let Some(dry_run) = self.dry_run {
                cli.dry_run = dry_run;
            }
        }

        if cli.github_token.is_none() {
            cli.github_token = self.github_token;
        }

        if cli.bitbucket_token.is_none() {
            cli.bitbucket_token = self.bitbucket_token;
        }

        if cli.gitlab_token.is_none() {
            cli.gitlab_token = self.gitlab_token;
        }

        if cli.forgejo_token.is_none() {
            cli.forgejo_token = self.forgejo_token;
        }

        // Only override if CLI has the default value
        if cli.github_url == "https://api.github.com" {
            if let Some(url) = self.github_url {
                cli.github_url = url;
            }
        }

        if cli.bitbucket_url == "https://api.bitbucket.org/2.0" {
            if let Some(url) = self.bitbucket_url {
                cli.bitbucket_url = url;
            }
        }

        if cli.gitlab_url == "https://gitlab.com" {
            if let Some(url) = self.gitlab_url {
                cli.gitlab_url = url;
            }
        }

        if cli.forgejo_url == "https://codeberg.org" {
            if let Some(url) = self.forgejo_url {
                cli.forgejo_url = url;
            }
        }

        if cli.format == OutputFormat::Text && !cli.json {
            if let Some(format) = self.format {
                cli.format = format;
            }
        }

        if cli.upgrade_strategy == UpgradeStrategy::Latest {
            if let Some(strategy) = self.upgrade_strategy {
                cli.upgrade_strategy = strategy;
            }
        }

        if cli.concurrency.is_none() {
            cli.concurrency = self.concurrency;
        }

        if cli.ignore.is_empty() {
            if let Some(ignore) = self.ignore {
                cli.ignore = ignore;
            }
        }

        if cli.oci_username.is_none() {
            cli.oci_username = self.oci_username;
        }

        if cli.oci_password.is_none() {
            cli.oci_password = self.oci_password;
        }

        cli
    }
}
