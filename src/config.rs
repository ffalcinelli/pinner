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

use crate::cli::{Cli, Commands, OutputFormat, UpgradeStrategy};

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
    /// Automatically check the OSV database during verification.
    pub check_osv: Option<bool>,
    /// Fail verification if any dependency is not explicitly vetted in the configuration.
    pub strict: Option<bool>,
    /// Username for OCI registry authentication.
    pub oci_username: Option<String>,
    /// Password for OCI registry authentication.
    pub oci_password: Option<String>,
    /// Force offline mode, preventing any network requests.
    pub offline: Option<bool>,
    /// Disable persistent disk caching.
    pub no_cache: Option<bool>,
    /// Cache TTL in seconds.
    pub cache_ttl: Option<u64>,
}

impl Config {
    /// Loads configuration from default locations and environment variables.
    ///
    /// Precedence (from lowest to highest):
    /// 1. Global config files
    /// 2. Local `.pinner.toml`
    /// 3. Local `.pinner.yaml` or `.pinner.yml`
    /// 4. Environment variables prefixed with `PINNER_` (e.g., `PINNER_YES=true`).
    pub fn load() -> Self {
        let mut global = Config::load_global();
        let local = Figment::new()
            .merge(Toml::file(".pinner.toml"))
            .merge(Yaml::file(".pinner.yaml"))
            .merge(Yaml::file(".pinner.yml"))
            .merge(Env::prefixed("PINNER_"))
            .extract()
            .unwrap_or_else(|_| Config::default());
        global.merge_all(local);
        global
    }

    /// Loads configuration from global user locations.
    ///
    /// Locations checked (from lowest to highest precedence):
    /// 1. `dirs::cache_dir()/pinner/config.toml`
    /// 2. `dirs::config_dir()/pinner/config.toml`
    /// 3. `dirs::home_dir()/.pinner.toml`
    pub fn load_global() -> Self {
        let is_test = cfg!(test)
            || std::env::current_exe()
                .map(|path| path.to_string_lossy().contains("/deps/"))
                .unwrap_or(false);
        if is_test && std::env::var("PINNER_TEST_ALLOW_GLOBAL").is_err() {
            return Config::default();
        }

        let mut global = Config::default();

        if let Some(mut p) = dirs::cache_dir() {
            p.push("pinner");
            p.push("config.toml");
            if p.exists() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if let Ok(cfg) = toml::from_str::<Config>(&content) {
                        global.merge_all(cfg);
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
                        global.merge_all(cfg);
                    }
                }
            }
        }

        if let Some(mut p) = dirs::home_dir() {
            p.push(".pinner.toml");
            if p.exists() {
                if let Ok(content) = std::fs::read_to_string(&p) {
                    if let Ok(cfg) = toml::from_str::<Config>(&content) {
                        global.merge_all(cfg);
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

    /// Merges all configuration options from another configuration into this one.
    pub fn merge_all(&mut self, other: Config) {
        if other.workflows.is_some() {
            self.workflows = other.workflows.clone();
        }
        if other.yes.is_some() {
            self.yes = other.yes;
        }
        if other.quiet.is_some() {
            self.quiet = other.quiet;
        }
        if other.verbose.is_some() {
            self.verbose = other.verbose;
        }
        if other.dry_run.is_some() {
            self.dry_run = other.dry_run;
        }
        if other.github_token.is_some() {
            self.github_token = other.github_token.clone();
        }
        if other.bitbucket_token.is_some() {
            self.bitbucket_token = other.bitbucket_token.clone();
        }
        if other.gitlab_token.is_some() {
            self.gitlab_token = other.gitlab_token.clone();
        }
        if other.forgejo_token.is_some() {
            self.forgejo_token = other.forgejo_token.clone();
        }
        if other.circleci_token.is_some() {
            self.circleci_token = other.circleci_token.clone();
        }
        if other.format.is_some() {
            self.format = other.format.clone();
        }
        if other.github_url.is_some() {
            self.github_url = other.github_url.clone();
        }
        if other.bitbucket_url.is_some() {
            self.bitbucket_url = other.bitbucket_url.clone();
        }
        if other.gitlab_url.is_some() {
            self.gitlab_url = other.gitlab_url.clone();
        }
        if other.forgejo_url.is_some() {
            self.forgejo_url = other.forgejo_url.clone();
        }
        if other.circleci_url.is_some() {
            self.circleci_url = other.circleci_url.clone();
        }
        if other.upgrade_strategy.is_some() {
            self.upgrade_strategy = other.upgrade_strategy.clone();
        }
        if other.concurrency.is_some() {
            self.concurrency = other.concurrency;
        }
        if other.ignore.is_some() {
            self.ignore = other.ignore.clone();
        }
        if other.no_security_feedback.is_some() {
            self.no_security_feedback = other.no_security_feedback;
        }
        if other.check_osv.is_some() {
            self.check_osv = other.check_osv;
        }
        if other.strict.is_some() {
            self.strict = other.strict;
        }
        if other.oci_username.is_some() {
            self.oci_username = other.oci_username.clone();
        }
        if other.oci_password.is_some() {
            self.oci_password = other.oci_password.clone();
        }
        if other.offline.is_some() {
            self.offline = other.offline;
        }
        if other.no_cache.is_some() {
            self.no_cache = other.no_cache;
        }
        if other.cache_ttl.is_some() {
            self.cache_ttl = other.cache_ttl;
        }
        self.merge_lists(other);
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

        if !cli.no_cache {
            if let Some(no_cache) = self.no_cache {
                cli.no_cache = no_cache;
            }
        }

        if cli.cache_ttl.is_none() {
            cli.cache_ttl = self.cache_ttl;
        }

        if let Commands::Verify { check_osv, strict } = &mut cli.command {
            if !*check_osv {
                if let Some(val) = self.check_osv {
                    *check_osv = val;
                }
            }
            if !*strict {
                if let Some(val) = self.strict {
                    *strict = val;
                }
            }
        }

        if let Commands::Upgrade {
            upgrade_strategy, ..
        }
        | Commands::Scan { upgrade_strategy } = &mut cli.command
        {
            if *upgrade_strategy == UpgradeStrategy::Latest {
                if let Some(strategy) = self.upgrade_strategy {
                    *upgrade_strategy = strategy;
                }
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
        if cli.format == OutputFormat::Text {
            if let Some(format) = self.format {
                cli.format = format;
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

    /// Serializes the configuration to a pretty TOML string, formatting the `vetted`
    /// and `compromised` security lists as compact inline table arrays.
    pub fn to_formatted_string(&self) -> Result<String, crate::error::PinnerError> {
        let mut temp_config = self.clone();
        temp_config.vetted = None;
        temp_config.compromised = None;

        let mut toml_str = toml::to_string_pretty(&temp_config)
            .map_err(|e| crate::error::PinnerError::Config(e.to_string()))?;

        // Ensure there is a trailing newline if not empty
        if !toml_str.ends_with('\n') && !toml_str.is_empty() {
            toml_str.push('\n');
        }

        if let Some(ref vetted) = self.vetted {
            if !toml_str.is_empty() {
                toml_str.push('\n');
            }
            toml_str.push_str("vetted = ");
            toml_str.push_str(&format_security_list(vetted));
            toml_str.push('\n');
        }

        if let Some(ref compromised) = self.compromised {
            if !toml_str.is_empty() {
                toml_str.push('\n');
            }
            toml_str.push_str("compromised = ");
            toml_str.push_str(&format_security_list(compromised));
            toml_str.push('\n');
        }

        Ok(toml_str)
    }
}

fn format_security_list(list: &[SecurityEntry]) -> String {
    if list.is_empty() {
        return "[]".to_string();
    }
    let mut s = "[\n".to_string();
    for (i, entry) in list.iter().enumerate() {
        let mut parts = Vec::new();
        parts.push(format!("ref = \"{}\"", entry.reference));
        if let Some(ref tag) = entry.tag {
            parts.push(format!("tag = \"{}\"", tag));
        }
        if let Some(ref ts) = entry.timestamp {
            parts.push(format!("timestamp = \"{}\"", ts));
        }
        s.push_str(&format!("    {{ {} }}", parts.join(", ")));
        if i < list.len() - 1 {
            s.push_str(",\n");
        } else {
            s.push('\n');
        }
    }
    s.push(']');
    s
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
            cache_ttl: None,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };

        let merged = config.merge_with_cli(cli);
        assert_eq!(merged.github_url, "https://api.github.com");
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
            command: Commands::Upgrade {
                interactive: false,
                upgrade_strategy: UpgradeStrategy::Latest,
            },
            workflows: vec![],
            yes: false,
            quiet: false,
            verbose: false,
            no_cache: false,
            cache_ttl: None,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
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
        if let Commands::Upgrade {
            upgrade_strategy, ..
        } = merged.command
        {
            assert_eq!(upgrade_strategy, UpgradeStrategy::Major);
        } else {
            panic!("Expected Commands::Upgrade");
        }
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
            command: Commands::Upgrade {
                interactive: false,
                upgrade_strategy: UpgradeStrategy::Commit,
            },
            workflows: vec![PathBuf::from("cli_wf")],
            yes: true,
            quiet: false,
            verbose: false,
            no_cache: false,
            cache_ttl: None,
            offline: false,
            dry_run: false,
            github_token: Some("cli_token".into()),
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            github_url: "https://cli.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
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
        if let Commands::Upgrade {
            upgrade_strategy, ..
        } = merged.command
        {
            assert_eq!(upgrade_strategy, UpgradeStrategy::Commit);
        } else {
            panic!("Expected Commands::Upgrade");
        }
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
            cache_ttl: None,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };
        let merged = config.merge_with_cli(cli);
        assert!(merged.offline);
    }

    #[test]
    #[serial_test::serial]
    fn test_load_global_with_files() {
        use std::env;
        use tempfile::tempdir;

        let tmp = tempdir().unwrap();
        let home_path = tmp.path().join("home");
        let config_path = tmp.path().join("config");
        let cache_path = tmp.path().join("cache");

        std::fs::create_dir_all(&home_path).unwrap();
        std::fs::create_dir_all(&config_path).unwrap();
        std::fs::create_dir_all(&cache_path).unwrap();

        // Write config files
        let cache_config = cache_path.join("pinner").join("config.toml");
        std::fs::create_dir_all(cache_config.parent().unwrap()).unwrap();
        std::fs::write(&cache_config, "vetted = ['cache_vet']\n").unwrap();

        let config_config = config_path.join("pinner").join("config.toml");
        std::fs::create_dir_all(config_config.parent().unwrap()).unwrap();
        std::fs::write(&config_config, "vetted = ['config_vet']\n").unwrap();

        let home_config = home_path.join(".pinner.toml");
        std::fs::write(&home_config, "vetted = ['home_vet']\n").unwrap();

        // Save original env vars
        let orig_home = env::var_os("HOME");
        let orig_config = env::var_os("XDG_CONFIG_HOME");
        let orig_cache = env::var_os("XDG_CACHE_HOME");

        // Set env vars to our temp paths
        env::set_var("HOME", &home_path);
        env::set_var("XDG_CONFIG_HOME", &config_path);
        env::set_var("XDG_CACHE_HOME", &cache_path);
        env::set_var("PINNER_TEST_ALLOW_GLOBAL", "true");

        let config = Config::load_global();

        env::remove_var("PINNER_TEST_ALLOW_GLOBAL");

        // Restore env vars
        if let Some(val) = orig_home {
            env::set_var("HOME", val);
        } else {
            env::remove_var("HOME");
        }
        if let Some(val) = orig_config {
            env::set_var("XDG_CONFIG_HOME", val);
        } else {
            env::remove_var("XDG_CONFIG_HOME");
        }
        if let Some(val) = orig_cache {
            env::set_var("XDG_CACHE_HOME", val);
        } else {
            env::remove_var("XDG_CACHE_HOME");
        }

        let vetted_refs: Vec<String> = config
            .vetted
            .unwrap()
            .into_iter()
            .map(|e| e.reference)
            .collect();
        assert!(vetted_refs.contains(&"cache_vet".to_string()));
        assert!(vetted_refs.contains(&"config_vet".to_string()));
        assert!(vetted_refs.contains(&"home_vet".to_string()));
    }

    #[test]
    fn test_merge_with_cli_other_fields() {
        let config = Config {
            check_osv: Some(true),
            strict: Some(true),
            bitbucket_url: Some("https://my-bitbucket".to_string()),
            gitlab_url: Some("https://my-gitlab".to_string()),
            forgejo_url: Some("https://my-forgejo".to_string()),
            circleci_url: Some("https://my-circleci".to_string()),
            format: Some(OutputFormat::Json),
            ..Default::default()
        };
        let cli = Cli {
            command: Commands::Verify {
                check_osv: false,
                strict: false,
            },
            workflows: vec![],
            yes: false,
            quiet: false,
            verbose: false,
            no_cache: false,
            cache_ttl: None,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };
        let merged = config.merge_with_cli(cli);
        if let Commands::Verify { check_osv, strict } = merged.command {
            assert!(check_osv);
            assert!(strict);
        } else {
            panic!("Expected Commands::Verify");
        }
        assert_eq!(merged.bitbucket_url, "https://my-bitbucket");
        assert_eq!(merged.gitlab_url, "https://my-gitlab");
        assert_eq!(merged.forgejo_url, "https://my-forgejo");
        assert_eq!(merged.circleci_url, "https://my-circleci");
        assert_eq!(merged.format, OutputFormat::Json);
    }

    #[test]
    fn test_security_entry_from_string() {
        let entry = SecurityEntry::from("hello".to_string());
        assert_eq!(entry.reference, "hello");
    }

    #[test]
    fn test_security_entry_deserialize_failures_and_ignored() {
        // Ignored field test
        let toml_str = r#"
            ref = "my-ref"
            unknown_field = 42
        "#;
        let entry: SecurityEntry = toml::from_str(toml_str).unwrap();
        assert_eq!(entry.reference, "my-ref");

        // Expecting/invalid type test
        let res: Result<SecurityEntry, _> = toml::from_str("123");
        assert!(res.is_err());
    }

    #[test]
    fn test_to_formatted_string() {
        let config = Config {
            vetted: Some(vec![
                SecurityEntry {
                    reference: "actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10"
                        .to_string(),
                    tag: Some("v6.0.3".to_string()),
                    timestamp: Some("2026-06-19T08:37:29Z".to_string()),
                },
                SecurityEntry {
                    reference:
                        "taiki-e/create-gh-release-action@eba8ea96c86cca8a37f1b56e94b4d13301fba651"
                            .to_string(),
                    tag: Some("v1.11.0".to_string()),
                    timestamp: Some("2026-06-19T08:37:29Z".to_string()),
                },
            ]),
            yes: Some(false),
            concurrency: Some(10),
            ..Default::default()
        };

        let toml_str = config.to_formatted_string().unwrap();
        println!("Serialized:\n{}", toml_str);

        assert!(toml_str.contains("vetted = ["));
        assert!(toml_str
            .contains("ref = \"actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10\""));
        assert!(toml_str.contains("tag = \"v6.0.3\""));
        assert!(toml_str.contains("timestamp = \"2026-06-19T08:37:29Z\""));
        assert!(!toml_str.contains("[[vetted]]"));
    }

    #[test]
    fn test_merge_cache_settings() {
        let config = Config {
            no_cache: Some(true),
            cache_ttl: Some(7200),
            ..Default::default()
        };
        let cli = Cli {
            command: Commands::Pin,
            workflows: vec![],
            yes: false,
            quiet: false,
            verbose: false,
            no_cache: false,
            cache_ttl: None,
            offline: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            circleci_token: None,
            format: OutputFormat::Text,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            circleci_url: "https://circleci.com/graphql-unstable".to_string(),
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };
        let merged = config.merge_with_cli(cli);
        assert!(merged.no_cache);
        assert_eq!(merged.cache_ttl, Some(7200));
    }
}
