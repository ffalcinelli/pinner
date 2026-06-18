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
    /// CircleCI API Token for authentication.
    pub circleci_token: Option<String>,
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
    /// Base URL for the CircleCI GraphQL API.
    pub circleci_url: Option<String>,
    /// Strategy to use when upgrading actions.
    pub upgrade_strategy: Option<UpgradeStrategy>,
    /// Number of concurrent API requests to make.
    pub concurrency: Option<usize>,
    /// Actions or images to ignore.
    pub ignore: Option<Vec<String>>,
    /// Vetted dependency hashes or references.
    pub vetted: Option<Vec<SecurityEntry>>,
    /// Compromised dependency hashes or references.
    pub compromised: Option<Vec<SecurityEntry>>,
    /// Disable visual security feedback.
    pub no_security_feedback: Option<bool>,
    /// Username for OCI registry authentication.
    pub oci_username: Option<String>,
    /// Password for OCI registry authentication.
    pub oci_password: Option<String>,
    /// Force offline mode, preventing any network requests.
    pub offline: Option<bool>,
}

impl Config {
    /// Loads configuration from default locations and environment variables.
    ///
    /// Precedence (from lowest to highest):
    /// 1. Default values.
    /// 2. `.pinner.toml`
    /// 3. `.pinner.yaml` or `.pinner.yml`
    /// 4. Environment variables prefixed with `PINNER_` (e.g., `PINNER_YES=true`).
    pub fn load() -> Self {
        Figment::new()
            .merge(Toml::file(".pinner.toml"))
            .merge(Yaml::file(".pinner.yaml"))
            .merge(Yaml::file(".pinner.yml"))
            .merge(Env::prefixed("PINNER_"))
            .extract()
            .unwrap_or_else(|_| Config::default())
    }

    /// Loads configuration from global user locations.
    ///
    /// Locations checked (from lowest to highest precedence):
    /// 1. `dirs::cache_dir()/pinner/config.toml`
    /// 2. `dirs::config_dir()/pinner/config.toml`
    /// 3. `dirs::home_dir()/.pinner.toml`
    pub fn load_global() -> Self {
        let mut global = Config::default();

        if let Some(mut p) = dirs::cache_dir() {
            p.push("pinner");
            p.push("config.toml");
            if p.exists() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if let Ok(cfg) = toml::from_str::<Config>(&content) {
                        global.merge_lists(cfg);
                    }
                }
            }
        }

        if let Some(mut p) = dirs::config_dir() {
            p.push("pinner");
            p.push("config.toml");
            if p.exists() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if let Ok(cfg) = toml::from_str::<Config>(&content) {
                        global.merge_lists(cfg);
                    }
                }
            }
        }

        if let Some(mut p) = dirs::home_dir() {
            p.push(".pinner.toml");
            if p.exists() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if let Ok(cfg) = toml::from_str::<Config>(&content) {
                        global.merge_lists(cfg);
                    }
                }
            }
        }

        global
    }

    /// Merges security entry lists from another configuration into this one,
    /// avoiding duplicates.
    pub fn merge_lists(&mut self, other: Config) {
        if let Some(other_vetted) = other.vetted {
            let vetted = self.vetted.get_or_insert_with(Vec::new);
            for item in other_vetted {
                if !vetted.iter().any(|e| e.reference == item.reference) {
                    vetted.push(item);
                }
            }
        }
        if let Some(other_compromised) = other.compromised {
            let compromised = self.compromised.get_or_insert_with(Vec::new);
            for item in other_compromised {
                if !compromised.iter().any(|e| e.reference == item.reference) {
                    compromised.push(item);
                }
            }
        }
    }

    /// Merges this configuration with CLI arguments, with CLI taking precedence.
    ///
    /// This method ensures that explicitly provided CLI flags always override
    /// settings from configuration files or environment variables.
    pub fn merge_with_cli(self, mut cli: Cli) -> Cli {
        // Workflow paths: CLI > Config
        if cli.workflows.is_empty() {
            if let Some(workflows) = self.workflows {
                cli.workflows = workflows;
            }
        }

        // Boolean flags: CLI (if true) > Config
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

        if !cli.offline {
            if let Some(offline) = self.offline {
                cli.offline = offline;
            }
        }

        // API Tokens: CLI > Config
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

        if cli.circleci_token.is_none() {
            cli.circleci_token = self.circleci_token;
        }

        // API Base URLs: CLI (if non-default) > Config
        // We only override if the CLI still has the default public API values.
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

        if cli.circleci_url == "https://circleci.com/graphql-unstable" {
            if let Some(url) = self.circleci_url {
                cli.circleci_url = url;
            }
        }

        // Output format: CLI > Config
        if cli.format == OutputFormat::Text && !cli.json {
            if let Some(format) = self.format {
                cli.format = format;
            }
        }

        // Upgrade strategy: CLI > Config
        if cli.upgrade_strategy == UpgradeStrategy::Latest {
            if let Some(strategy) = self.upgrade_strategy {
                cli.upgrade_strategy = strategy;
            }
        }

        // Concurrency & Ignore: CLI > Config
        if cli.concurrency.is_none() {
            cli.concurrency = self.concurrency;
        }

        if cli.ignore.is_empty() {
            if let Some(ignore) = self.ignore {
                cli.ignore = ignore;
            }
        }

        // OCI Auth: CLI > Config
        if cli.oci_username.is_none() {
            cli.oci_username = self.oci_username;
        }

        if cli.oci_password.is_none() {
            cli.oci_password = self.oci_password;
        }

        cli
    }
}

/// Represents an entry in the vetted or compromised lists, which can include a version and timestamp.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SecurityEntry {
    /// The dependency reference (e.g. actions/checkout@sha).
    #[serde(rename = "ref")]
    pub reference: String,
    /// The tag version (e.g. v4.1.7).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The timestamp of insertion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl<'de> serde::Deserialize<'de> for SecurityEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SecurityEntryVisitor;

        impl<'de> serde::de::Visitor<'de> for SecurityEntryVisitor {
            type Value = SecurityEntry;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string or a map")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(SecurityEntry {
                    reference: value.to_string(),
                    tag: None,
                    timestamp: None,
                })
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: serde::de::MapAccess<'de>,
            {
                let mut reference = None;
                let mut tag = None;
                let mut timestamp = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "ref" => reference = Some(map.next_value()?),
                        "tag" => tag = Some(map.next_value()?),
                        "timestamp" => timestamp = Some(map.next_value()?),
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }

                let reference = reference.ok_or_else(|| serde::de::Error::missing_field("ref"))?;

                Ok(SecurityEntry {
                    reference,
                    tag,
                    timestamp,
                })
            }
        }

        deserializer.deserialize_any(SecurityEntryVisitor)
    }
}

impl From<String> for SecurityEntry {
    fn from(s: String) -> Self {
        SecurityEntry {
            reference: s,
            tag: None,
            timestamp: None,
        }
    }
}

impl From<&str> for SecurityEntry {
    fn from(s: &str) -> Self {
        SecurityEntry {
            reference: s.to_string(),
            tag: None,
            timestamp: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Commands;
    use std::path::PathBuf;

    #[test]
    fn test_merge_with_cli_defaults() {
        let config = Config::default();
        let cli = Cli {
            command: Commands::Pin,
            workflows: vec![],
            yes: false,
            quiet: false,
            verbose: false,
            no_cache: false,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            json: false,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            upgrade_strategy: UpgradeStrategy::Latest,
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };

        let merged = config.merge_with_cli(cli);
        assert_eq!(merged.github_url, "https://api.github.com");
        assert_eq!(merged.upgrade_strategy, UpgradeStrategy::Latest);
    }

    #[test]
    fn test_merge_with_cli_overrides() {
        let config = Config {
            workflows: Some(vec![PathBuf::from("config_wf")]),
            yes: Some(true),
            quiet: Some(true),
            verbose: Some(true),
            dry_run: Some(true),
            github_token: Some("config_token".into()),
            github_url: Some("https://config.github.com".into()),
            upgrade_strategy: Some(UpgradeStrategy::Major),
            concurrency: Some(10),
            ignore: Some(vec!["ignore1".into()]),
            vetted: Some(vec!["vet1".into()]),
            compromised: Some(vec!["comp1".into()]),
            ..Default::default()
        };

        let cli = Cli {
            command: Commands::Pin,
            workflows: vec![],
            yes: false,
            quiet: false,
            verbose: false,
            no_cache: false,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            json: false,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            upgrade_strategy: UpgradeStrategy::Latest,
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };

        let merged = config.merge_with_cli(cli);
        assert_eq!(merged.workflows, vec![PathBuf::from("config_wf")]);
        assert!(merged.yes);
        assert!(merged.quiet);
        assert!(merged.verbose);
        assert!(merged.dry_run);
        assert_eq!(merged.github_token, Some("config_token".into()));
        assert_eq!(merged.github_url, "https://config.github.com");
        assert_eq!(merged.upgrade_strategy, UpgradeStrategy::Major);
        assert_eq!(merged.concurrency, Some(10));
        assert_eq!(merged.ignore, vec!["ignore1".to_string()]);
    }

    #[test]
    fn test_cli_precedence() {
        let config = Config {
            workflows: Some(vec![PathBuf::from("config_wf")]),
            yes: Some(false),
            github_token: Some("config_token".into()),
            github_url: Some("https://config.github.com".into()),
            ..Default::default()
        };

        let cli = Cli {
            command: Commands::Pin,
            workflows: vec![PathBuf::from("cli_wf")],
            yes: true,
            quiet: false,
            verbose: false,
            no_cache: false,
            offline: false,
            dry_run: false,
            github_token: Some("cli_token".into()),
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            json: false,
            github_url: "https://cli.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            upgrade_strategy: UpgradeStrategy::Latest,
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };

        let merged = config.merge_with_cli(cli);
        assert_eq!(merged.workflows, vec![PathBuf::from("cli_wf")]);
        assert!(merged.yes);
        assert_eq!(merged.github_token, Some("cli_token".into()));
        assert_eq!(merged.github_url, "https://cli.github.com");
    }

    #[test]
    fn test_merge_lists() {
        let mut cfg1 = Config {
            vetted: Some(vec![SecurityEntry::from("vet1")]),
            compromised: Some(vec![SecurityEntry::from("comp1")]),
            ..Default::default()
        };
        let cfg2 = Config {
            vetted: Some(vec![
                SecurityEntry::from("vet1"),
                SecurityEntry::from("vet2"),
            ]),
            compromised: Some(vec![SecurityEntry::from("comp2")]),
            ..Default::default()
        };
        cfg1.merge_lists(cfg2);
        assert_eq!(cfg1.vetted.unwrap().len(), 2);
        assert_eq!(cfg1.compromised.unwrap().len(), 2);
    }

    #[test]
    fn test_config_offline_merging() {
        let config = Config {
            offline: Some(true),
            ..Default::default()
        };
        let cli = Cli {
            command: Commands::Pin,
            workflows: vec![],
            yes: false,
            quiet: false,
            verbose: false,
            no_cache: false,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            json: false,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            upgrade_strategy: UpgradeStrategy::Latest,
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };
        let merged = config.merge_with_cli(cli);
        assert!(merged.offline);
    }
}
