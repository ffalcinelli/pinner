//! # Pinner Library
//!
//! `pinner` is a library for hash-pinning GitHub Actions in workflow files.
//! It provides tools to scan YAML workflows and replace mutable tags with immutable commit SHAs.
//!
//! ## Core Components
//! - [`Operations`]: The main orchestrator for pinning, upgrading, and setting action hashes.
//! - [`GithubProvider`]: A trait for fetching commit SHAs and latest releases from GitHub.
//! - [`ReqwestGithubProvider`]: The default implementation of [`GithubProvider`] using `reqwest`.
//!
//! ## Example
//! ```no_run
//! use pinner::{Operations, ReqwestGithubProvider, Cli, Commands};
//! use std::sync::Arc;
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() {
//!     let github = ReqwestGithubProvider::default();
//!     let ops = Operations::new(Arc::new(github), true, false, false, false);
//!     ops.pin(&[PathBuf::from(".github/workflows")]).await.unwrap();
//! }
//! ```

use async_trait::async_trait;
use clap::{Parser, Subcommand};
use colored::Colorize;
use futures::stream::{self, StreamExt};
use ignore::WalkBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use moka::future::Cache;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tree_sitter::{Node, Parser as TSParser};

#[cfg(test)]
use mockall::automock;

/// Custom error type for Pinner operations.
#[derive(Error, Debug)]
pub enum PinnerError {
    /// IO-related errors (file reading, writing, etc.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// Errors returned by the GitHub API or HTTP client.
    #[error("API error: {0}")]
    Api(String),
    /// Errors during YAML parsing (tree-sitter).
    #[error("Parse error: {0}")]
    Parse(String),
    /// Specified workflow path not found.
    #[error("Path not found: {0}")]
    PathNotFound(String),
    /// Errors from the `ignore` crate during directory walking.
    #[error("Ignore error: {0}")]
    Ignore(#[from] ignore::Error),
    /// Config file errors
    #[error("Config error: {0}")]
    Config(String),
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
    /// Print diff without modifying files
    #[arg(short, long, global = true)]
    pub dry_run: bool,
    /// GitHub API Token
    #[arg(short, long, global = true, env = "GITHUB_TOKEN")]
    pub token: Option<String>,
    /// Output results as JSON
    #[arg(long, global = true)]
    pub json: bool,
}

/// Subcommands for the Pinner CLI.
#[derive(Subcommand, Debug, PartialEq)]
pub enum Commands {
    /// Pin all actions to their current commit SHAs.
    Pin,
    /// Upgrade all actions to their latest releases.
    Upgrade,
    /// Set a specific action to a specific commit SHA.
    Set {
        /// Action name (e.g., actions/checkout)
        action: String,
        /// Commit SHA-1 hash
        hash: String,
    },
}

/// Runs the Pinner CLI logic.
///
/// # Arguments
/// * `cli` - Parsed command line arguments.
/// * `github` - An implementation of [`GithubProvider`].
/// * `paths` - Paths to workflow files or directories to process.
pub async fn run<G: GithubProvider + 'static>(
    cli: Cli,
    github: G,
    paths: Vec<PathBuf>,
) -> Result<(), PinnerError> {
    let ops = Operations::new(Arc::new(github), cli.yes, cli.quiet, cli.dry_run, cli.json);
    match cli.command {
        Commands::Pin => ops.pin(&paths).await,
        Commands::Upgrade => ops.upgrade(&paths).await,
        Commands::Set { action, hash } => ops.set(&paths, &action, &hash).await,
    }
}

#[derive(Debug, Deserialize)]
struct RefResponse {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseResponse {
    tag_name: String,
}

/// Trait for interacting with the GitHub API.
#[cfg_attr(test, automock)]
#[async_trait]
pub trait GithubProvider: Send + Sync {
    /// Fetches the commit SHA for a given action and tag/branch.
    async fn get_commit_sha(&self, action: &str, tag: &str) -> Result<String, PinnerError>;
    /// Fetches the latest release tag for a given action.
    async fn get_latest_release(&self, action: &str) -> Result<String, PinnerError>;
    /// Fetches the default branch for a given action.
    async fn get_default_branch(&self, action: &str) -> Result<String, PinnerError>;
}

#[derive(Debug, Deserialize)]
struct RepoResponse {
    default_branch: String,
}

/// Default implementation of [`GithubProvider`] using `reqwest`.
pub struct ReqwestGithubProvider {
    client: ClientWithMiddleware,
    base_url: String,
    sha_cache: Cache<(String, String), String>,
    release_cache: Cache<String, String>,
    branch_cache: Cache<String, String>,
}

#[cfg(not(tarpaulin))]
impl Default for ReqwestGithubProvider {
    fn default() -> Self {
        Self::new("https://api.github.com".to_string(), None)
    }
}

impl ReqwestGithubProvider {
    /// Creates a new [`ReqwestGithubProvider`] with the specified base URL and optional token.
    pub fn new(base_url: String, token: Option<String>) -> Self {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, HeaderValue::from_static("pinner"));

        let token = token
            .or_else(|| std::env::var("GITHUB_TOKEN").ok())
            .or_else(Self::try_gh_cli_token);

        if let Some(t) = token {
            if let Ok(auth) = HeaderValue::from_str(&format!("Bearer {}", t)) {
                h.insert(AUTHORIZATION, auth);
            }
        }

        let reqwest_client = reqwest::Client::builder()
            .default_headers(h)
            .build()
            .unwrap();

        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let client = ClientBuilder::new(reqwest_client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Self {
            client,
            base_url,
            sha_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(Duration::from_secs(3600))
                .build(),
            release_cache: Cache::builder()
                .max_capacity(500)
                .time_to_live(Duration::from_secs(3600))
                .build(),
            branch_cache: Cache::builder()
                .max_capacity(500)
                .time_to_live(Duration::from_secs(3600))
                .build(),
        }
    }

    fn try_gh_cli_token() -> Option<String> {
        let config_path = dirs::config_dir()?.join("gh/hosts.yml");
        if config_path.exists() {
            let content = fs::read_to_string(config_path).ok()?;
            let docs: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
            return docs
                .get("github.com")?
                .get("oauth_token")?
                .as_str()?
                .to_string()
                .into();
        }
        None
    }
}

#[async_trait]
impl GithubProvider for ReqwestGithubProvider {
    async fn get_commit_sha(&self, action: &str, tag: &str) -> Result<String, PinnerError> {
        let key = (action.to_string(), tag.to_string());
        if let Some(sha) = self.sha_cache.get(&key).await {
            return Ok(sha);
        }

        let url = format!("{}/repos/{}/commits/{}", self.base_url, action, tag);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;
        if resp.status().is_success() {
            let res: RefResponse = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            self.sha_cache.insert(key, res.sha.clone()).await;
            Ok(res.sha)
        } else {
            Err(PinnerError::Api(format!(
                "HTTP {}: Could not resolve ref '{}' for {}",
                resp.status(),
                tag,
                action
            )))
        }
    }

    async fn get_latest_release(&self, action: &str) -> Result<String, PinnerError> {
        if let Some(tag) = self.release_cache.get(&action.to_string()).await {
            return Ok(tag);
        }

        let url = format!("{}/repos/{}/releases/latest", self.base_url, action);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;
        if resp.status().is_success() {
            let rel: ReleaseResponse = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            self.release_cache
                .insert(action.to_string(), rel.tag_name.clone())
                .await;
            Ok(rel.tag_name)
        } else if resp.status().as_u16() == 404 {
            let default_branch = self.get_default_branch(action).await?;
            Ok(default_branch)
        } else {
            Err(PinnerError::Api(format!(
                "HTTP {}: Could not fetch latest release for {}",
                resp.status(),
                action
            )))
        }
    }

    async fn get_default_branch(&self, action: &str) -> Result<String, PinnerError> {
        if let Some(branch) = self.branch_cache.get(&action.to_string()).await {
            return Ok(branch);
        }

        let url = format!("{}/repos/{}", self.base_url, action);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let repo: RepoResponse = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            self.branch_cache
                .insert(action.to_string(), repo.default_branch.clone())
                .await;
            Ok(repo.default_branch)
        } else {
            Ok("main".to_string())
        }
    }
}

/// Configuration for Pinner.
#[derive(Debug, Deserialize, Default)]
pub struct Config {
    /// List of actions to ignore.
    pub ignore_actions: Vec<String>,
}

/// Orchestrator for pinning operations.
pub struct Operations<G: GithubProvider> {
    github: Arc<G>,
    yes: bool,
    quiet: bool,
    dry_run: bool,
    json: bool,
    config: Config,
}

struct UpdateTask {
    path: PathBuf,
    start: usize,
    end: usize,
    action: String,
    current_tag: Option<String>,
}

#[derive(Serialize)]
struct UpdateResult {
    #[serde(skip)]
    task: UpdateTask,
    action: String,
    path: PathBuf,
    old_tag: Option<String>,
    new_sha: String,
    new_tag: Option<String>,
}

#[derive(Serialize)]
struct JsonOutput {
    updates: Vec<UpdateResult>,
}

impl<G: GithubProvider + 'static> Operations<G> {
    pub fn new(github: Arc<G>, yes: bool, quiet: bool, dry_run: bool, json: bool) -> Self {
        let config = Self::load_config().unwrap_or_default();
        Self {
            github,
            yes,
            quiet,
            dry_run,
            json,
            config,
        }
    }

    fn load_config() -> Result<Config, PinnerError> {
        Self::load_config_from_path(Path::new(".pinner.toml"))
    }

    fn load_config_from_path(path: &Path) -> Result<Config, PinnerError> {
        let content = fs::read_to_string(path).unwrap_or_default();
        let config: Config = toml::from_str(&content)
            .map_err(|e| PinnerError::Config(format!("Failed to parse .pinner.toml: {}", e)))?;
        Ok(config)
    }

    pub async fn pin(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let github = self.github.clone();
        self.process(paths, move |action, tag| {
            let (a, t) = (action.to_string(), tag.map(|s| s.to_string()));
            let github = github.clone();
            async move {
                if let Some(ver) = t {
                    if ver.len() != 40 {
                        if let Ok(sha) = github.get_commit_sha(&a, &ver).await {
                            return Some((sha, Some(ver)));
                        }
                    }
                }
                None
            }
        })
        .await
    }

    pub async fn set(
        &self,
        paths: &[PathBuf],
        action: &str,
        hash: &str,
    ) -> Result<(), PinnerError> {
        let (a, h) = (action.to_string(), hash.to_string());
        self.process(paths, move |act, _| {
            let (a, h, act_owned) = (a.clone(), h.clone(), act.to_string());
            async move {
                if act_owned == a {
                    Some((h, None))
                } else {
                    None
                }
            }
        })
        .await
    }

    pub async fn upgrade(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let github = self.github.clone();
        self.process(paths, move |a, _| {
            let a = a.to_string();
            let github = github.clone();
            async move {
                if let Ok(tag) = github.get_latest_release(&a).await {
                    if let Ok(sha) = github.get_commit_sha(&a, &tag).await {
                        return Some((sha, Some(tag)));
                    }
                }
                None
            }
        })
        .await
    }

    async fn process<F, Fut>(&self, paths: &[PathBuf], f: F) -> Result<(), PinnerError>
    where
        F: Fn(&str, Option<&str>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Option<(String, Option<String>)>> + Send,
    {
        let mut parser = TSParser::new();
        parser
            .set_language(tree_sitter_yaml::language())
            .map_err(|e| PinnerError::Parse(e.to_string()))?;

        let mut tasks = Vec::new();

        for path in paths {
            if !path.exists() {
                return Err(PinnerError::PathNotFound(path.display().to_string()));
            }

            for entry in WalkBuilder::new(path).build() {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
                    let content = fs::read_to_string(path)?;
                    let tree = parser.parse(&content, None).ok_or_else(|| {
                        PinnerError::Parse(format!("Failed to parse {}", path.display()))
                    })?;

                    let mut uses_nodes = Vec::new();
                    self.find_uses_nodes(tree.root_node(), content.as_bytes(), &mut uses_nodes);

                    for (start, end, val) in uses_nodes {
                        let parts: Vec<&str> = val.split('@').collect();
                        let action = parts[0];

                        if self.config.ignore_actions.iter().any(|a| a == action) {
                            continue;
                        }

                        let tag = parts.get(1).copied();
                        tasks.push(UpdateTask {
                            path: path.to_path_buf(),
                            start,
                            end,
                            action: action.to_string(),
                            current_tag: tag.map(|s| s.to_string()),
                        });
                    }
                }
            }
        }

        let f = std::sync::Arc::new(f);
        let pb = if !self.quiet && !self.json && !tasks.is_empty() {
            let pb = ProgressBar::new(tasks.len() as u64);
            pb.set_style(
                ProgressStyle::default_bar()
                    .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
                    .expect("Failed to parse progress bar template")
                    .progress_chars("#>-"),
            );
            Some(pb)
        } else {
            None
        };

        let futs = tasks.into_iter().map(|task| {
            let f_clone = f.clone();
            let pb = pb.clone();
            async move {
                let res = if let Some((sha, tag)) =
                    f_clone(&task.action, task.current_tag.as_deref()).await
                {
                    Some(UpdateResult {
                        action: task.action.clone(),
                        path: task.path.clone(),
                        old_tag: task.current_tag.clone(),
                        task,
                        new_sha: sha,
                        new_tag: tag,
                    })
                } else {
                    None
                };
                if let Some(p) = pb {
                    p.inc(1);
                }
                res
            }
        });

        let results: Vec<UpdateResult> = stream::iter(futs)
            .buffer_unordered(10) // Bound concurrency to 10
            .filter_map(|res| async { res })
            .collect()
            .await;

        if let Some(p) = pb {
            p.finish_and_clear();
        }

        // Group results by file
        let mut file_results: std::collections::HashMap<PathBuf, Vec<UpdateResult>> =
            std::collections::HashMap::new();
        for res in results {
            file_results
                .entry(res.task.path.clone())
                .or_default()
                .push(res);
        }

        let comment_regex =
            Regex::new(r"^#\s*(v\d[a-zA-Z0-9.\-_]*|main|\d[a-zA-Z0-9.\-_]*)\s*").unwrap();

        let mut all_json_updates = Vec::new();

        for (path, mut updates) in file_results {
            let content = fs::read_to_string(&path)?;
            let mut new_content = content.clone();
            let mut changes = Vec::new();

            // Sort updates from back to front to preserve offsets
            updates.sort_by_key(|a| std::cmp::Reverse(a.task.start));

            for res in updates {
                let line_end = content[res.task.end..]
                    .find('\n')
                    .map(|pos| res.task.end + pos)
                    .unwrap_or(content.len());
                let old_val_with_suffix = &content[res.task.start..line_end];
                let suffix = &content[res.task.end..line_end];

                let mut final_suffix = suffix.trim_start().to_string();
                if let Some(mat) = comment_regex.find(&final_suffix) {
                    final_suffix = final_suffix[mat.end()..].trim_start().to_string();
                    if final_suffix.starts_with('#') {
                        final_suffix = final_suffix[1..].trim_start().to_string();
                    }
                }

                let new_comment = if let Some(t) = &res.new_tag {
                    format!(" # {}", t)
                } else {
                    "".to_string()
                };
                let extra_suffix = if final_suffix.is_empty() {
                    "".to_string()
                } else if final_suffix.starts_with('#') {
                    format!(" {}", final_suffix)
                } else {
                    format!(" # {}", final_suffix)
                };

                let new_val = format!(
                    "{}@{}{}{}",
                    res.task.action, res.new_sha, new_comment, extra_suffix
                );

                if old_val_with_suffix == new_val {
                    continue;
                }

                changes.push((old_val_with_suffix.to_string(), new_val.clone()));
                new_content.replace_range(res.task.start..line_end, &new_val);
                if self.json {
                    all_json_updates.push(res);
                }
            }

            if !changes.is_empty() && !self.quiet && !self.json {
                println!("\n{} {}", "File:".bold(), path.display().to_string().cyan());
                if self.dry_run {
                    self.print_diff(&content, &new_content);
                } else {
                    for (old, new_ln) in &changes {
                        self.print_inline_diff(old, new_ln);
                    }
                    let mut should_write = self.yes;
                    if !should_write {
                        use std::io::Write;
                        print!(
                            "{} {}? [y/N]: ",
                            "Apply changes to".bold(),
                            path.display().to_string().cyan()
                        );
                        let _ = std::io::stdout().flush();
                        let mut input = String::new();
                        if std::io::stdin().read_line(&mut input).is_ok() {
                            let input = input.trim().to_lowercase();
                            should_write = input == "y" || input == "yes";
                        }
                    }
                    if should_write {
                        fs::write(&path, new_content)?;
                        println!("{}", "✔ Updated successfully".green());
                    } else {
                        println!("{}", "✘ Skipped".yellow());
                    }
                }
            } else if !changes.is_empty() && (self.yes || self.json) && !self.dry_run {
                fs::write(&path, new_content)?;
            }
        }

        if self.json {
            let output = JsonOutput {
                updates: all_json_updates,
            };
            println!("{}", serde_json::to_string_pretty(&output).expect("Failed to serialize JSON output"));
        }

        Ok(())
    }

    fn print_diff(&self, old: &str, new: &str) {
        let diff = TextDiff::from_lines(old, new);
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-".red(),
                ChangeTag::Insert => "+".green(),
                ChangeTag::Equal => " ".normal(),
            };
            print!("{}{}", sign, change);
        }
    }

    fn print_inline_diff(&self, old: &str, new: &str) {
        let old_trimmed = old.trim();
        let new_trimmed = new.trim();
        let diff = TextDiff::from_words(old_trimmed, new_trimmed);

        print!("  {} ", "-".red());
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Delete => print!("{}", change.value().red()),
                ChangeTag::Equal => print!("{}", change.value().dimmed()),
                ChangeTag::Insert => {}
            }
        }
        println!();

        print!("  {} ", "+".green());
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Insert => print!("{}", change.value().green().bold()),
                ChangeTag::Equal => print!("{}", change.value().yellow()),
                ChangeTag::Delete => {}
            }
        }
        println!();
    }

    fn find_uses_nodes(
        &self,
        node: Node,
        content: &[u8],
        results: &mut Vec<(usize, usize, String)>,
    ) {
        if node.kind() == "block_mapping_pair" {
            let mut cursor = node.walk();
            let mut key = None;
            let mut val = None;
            for child in node.children(&mut cursor) {
                if child.kind() == "flow_node" || child.kind() == "plain_scalar" {
                    let text = child.utf8_text(content).unwrap_or("");
                    if text == "uses" {
                        key = Some(child);
                    } else if key.is_some() {
                        val = Some(child);
                        break;
                    }
                } else if child.kind() == "block_node" && key.is_some() {
                    val = Some(child);
                    break;
                }
            }
            if let (Some(_), Some(v)) = (key, val) {
                let mut v_node = v;
                while v_node.child_count() > 0 && v_node.kind() != "plain_scalar" {
                    if let Some(c) = v_node.child(0) {
                        v_node = c;
                    } else {
                        break;
                    }
                }

                results.push((
                    v_node.start_byte(),
                    v_node.end_byte(),
                    v_node.utf8_text(content).unwrap_or("").to_string(),
                ));
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.find_uses_nodes(child, content, results);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_all() {
        let mut s = Server::new_async().await;
        let _m = s
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(200)
            .with_body(r#"{"sha":"a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"}"#)
            .create_async()
            .await;
        let _m2 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(200)
            .with_body(r#"{"tag_name":"v2"}"#)
            .create_async()
            .await;
        let _m3 = s
            .mock("GET", "/repos/o/r/commits/v2")
            .with_status(200)
            .with_body(r#"{"sha":"692973e3d937129bcbf40652eb9f2f61becf3332"}"#)
            .create_async()
            .await;

        let p = ReqwestGithubProvider::new(s.url(), None);
        assert!(p.get_commit_sha("o/r", "v1").await.is_ok());
        assert_eq!(p.get_latest_release("o/r").await.unwrap(), "v2");

        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".into()));
        mock.expect_get_latest_release()
            .returning(|_| Ok("v2".into()));

        let dir = tempdir().unwrap();
        let wd = dir.path().join("w");
        fs::create_dir_all(&wd).unwrap();
        fs::write(wd.join("f.yml"), "uses: o/r@v1").unwrap();
        fs::write(wd.join("untagged.yml"), "uses: actions/checkout").unwrap();
        fs::write(wd.join("with_comment.yml"), "uses: o/r@v1 # keep me").unwrap();
        fs::write(
            wd.join("already_pinned.yml"),
            "uses: o/r@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1",
        )
        .unwrap();

        let ops = Operations::new(Arc::new(mock), true, false, false, false);
        ops.pin(std::slice::from_ref(&wd)).await.unwrap();

        assert!(fs::read_to_string(wd.join("f.yml"))
            .unwrap()
            .contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));

        let with_comment = fs::read_to_string(wd.join("with_comment.yml")).unwrap();
        assert!(with_comment.contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1 # keep me"));

        let already_pinned = fs::read_to_string(wd.join("already_pinned.yml")).unwrap();
        assert!(already_pinned.contains("uses: o/r@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));
        // Check that it didn't add double comments or mess up
        assert!(!already_pinned.contains("# v1 # v1"));

        let mut mock2 = MockGithubProvider::new();
        mock2
            .expect_get_latest_release()
            .returning(|_| Ok("v3".into()));
        mock2
            .expect_get_commit_sha()
            .returning(|_, _| Ok("692973e3d937129bcbf40652eb9f2f61becf3332".into()));
        let ops2 = Operations::new(Arc::new(mock2), true, false, false, false);
        ops2.upgrade(std::slice::from_ref(&wd)).await.unwrap();
        let ut = fs::read_to_string(wd.join("untagged.yml")).unwrap();
        assert!(ut.contains("actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332 # v3"));

        let mut mock3 = MockGithubProvider::new();
        mock3
            .expect_get_commit_sha()
            .returning(|_, _| Ok("s".into()));
        run(
            Cli {
                command: Commands::Pin,
                workflows: vec![],
                yes: true,
                quiet: true,
                dry_run: false,
                token: None,
                json: false,
            },
            mock3,
            vec![wd.clone()],
        )
        .await
        .unwrap();
        assert!(run(
            Cli {
                command: Commands::Pin,
                workflows: vec![],
                yes: true,
                quiet: true,
                dry_run: false,
                token: None,
                json: false,
            },
            MockGithubProvider::new(),
            vec![PathBuf::from("/n")]
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn test_github_provider_errors() {
        let mut s = Server::new_async().await;
        let p = ReqwestGithubProvider::new(s.url(), None);

        let _m = s
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(404)
            .create_async()
            .await;
        assert!(p.get_commit_sha("o/r", "v1").await.is_err());

        let _m_500 = s
            .mock("GET", "/repos/o/r/commits/v500")
            .with_status(500)
            .create_async()
            .await;
        assert!(p.get_commit_sha("o/r", "v500").await.is_err());

        let _m_bad_json = s
            .mock("GET", "/repos/o/r/commits/vbad")
            .with_status(200)
            .with_body("not valid json")
            .create_async()
            .await;
        assert!(p.get_commit_sha("o/r", "vbad").await.is_err());

        let p_bad_url = ReqwestGithubProvider::new("http://127.0.0.1:0".to_string(), None);
        assert!(p_bad_url.get_commit_sha("o/r", "v1").await.is_err());

        let _m2 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(500)
            .create_async()
            .await;
        assert!(p.get_latest_release("o/r").await.is_err());

        let _m3 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(404)
            .create_async()
            .await;
        assert_eq!(p.get_latest_release("o/r").await.unwrap(), "main");

        let _m4 = s
            .mock("GET", "/repos/o/r2/releases/latest")
            .with_status(404)
            .create_async()
            .await;
        let _m5 = s
            .mock("GET", "/repos/o/r2")
            .with_status(200)
            .with_body(r#"{"default_branch":"develop"}"#)
            .create_async()
            .await;
        assert_eq!(p.get_latest_release("o/r2").await.unwrap(), "develop");
    }

    #[tokio::test]
    async fn test_operations_set() {
        let mock = MockGithubProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let ops = Operations::new(Arc::new(mock), true, true, false, false);
        ops.set(std::slice::from_ref(&f), "o/r", "newhash")
            .await
            .unwrap();

        assert!(fs::read_to_string(&f).unwrap().contains("o/r@newhash"));
    }

    #[tokio::test]
    async fn test_operations_dry_run() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok("newhash".into()));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let ops = Operations::new(Arc::new(mock), true, false, true, false);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        assert_eq!(fs::read_to_string(&f).unwrap(), "uses: o/r@v1");
    }

    #[tokio::test]
    async fn test_operations_quiet() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok("newhash".into()));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let ops = Operations::new(Arc::new(mock), true, true, false, false);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        assert!(fs::read_to_string(&f).unwrap().contains("newhash"));
    }

    #[tokio::test]
    async fn test_find_uses_nodes_nested() {
        let mock = MockGithubProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        // Test with different indentation and structures
        fs::write(
            &f,
            "
jobs:
  test:
    steps:
      - name: Checkout
        uses: actions/checkout@v3
      - name: Custom
        uses: 
          owner/repo@v1
",
        )
        .unwrap();

        let ops = Operations::new(Arc::new(mock), true, true, false, false);
        // We don't care about actual replacement here, just that it doesn't crash
        // and find_uses_nodes works.
        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let content = fs::read_to_string(&f).unwrap();
        let tree = parser.parse(&content, None).unwrap();
        let mut results = Vec::new();
        ops.find_uses_nodes(tree.root_node(), content.as_bytes(), &mut results);

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|(_, _, v)| v == "actions/checkout@v3"));
        assert!(results.iter().any(|(_, _, v)| v == "owner/repo@v1"));
    }

    #[tokio::test]
    async fn test_run_subcommands() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok("h".into()));
        mock.expect_get_latest_release()
            .returning(|_| Ok("v2".into()));

        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let cli_upgrade = Cli {
            command: Commands::Upgrade,
            workflows: vec![],
            yes: true,
            quiet: true,
            dry_run: false,
            token: None,
            json: false,
        };
        run(cli_upgrade, mock, vec![f.clone()]).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("o/r@h # v2"));

        let mock2 = MockGithubProvider::new();
        let cli_set = Cli {
            command: Commands::Set {
                action: "o/r".into(),
                hash: "sethash".into(),
            },
            workflows: vec![],
            yes: true,
            quiet: true,
            dry_run: false,
            token: None,
            json: false,
        };
        run(cli_set, mock2, vec![f.clone()]).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("o/r@sethash"));
    }

    #[tokio::test]
    async fn test_error_path_not_found() {
        let mock = MockGithubProvider::new();
        let ops = Operations::new(Arc::new(mock), true, true, false, false);
        let res = ops.pin(&[PathBuf::from("/non/existent/path")]).await;
        assert!(matches!(res, Err(PinnerError::PathNotFound(_))));
    }

    #[tokio::test]
    async fn test_operations_config_ignore() {
        let mock = MockGithubProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: ignore/me@v1\nuses: keep/me@v1").unwrap();

        let mut config = Config::default();
        config.ignore_actions.push("ignore/me".to_string());

        let ops = Operations {
            github: Arc::new(mock),
            yes: true,
            quiet: true,
            dry_run: false,
            json: false,
            config,
        };

        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let content = fs::read_to_string(&f).unwrap();
        let tree = parser.parse(&content, None).unwrap();
        let mut results = Vec::new();
        ops.find_uses_nodes(tree.root_node(), content.as_bytes(), &mut results);

        // find_uses_nodes still finds it, but process should skip it.
        assert_eq!(results.len(), 2);

        // We can't easily test process skipping without a lot of mocking,
        // but let's at least test load_config.
        let config_file = dir.path().join(".pinner.toml");
        fs::write(&config_file, "ignore_actions = [\"a\", \"b\"]").unwrap();

        let loaded =
            Operations::<ReqwestGithubProvider>::load_config_from_path(&config_file).unwrap();

        assert_eq!(loaded.ignore_actions, vec!["a", "b"]);
    }

    #[test]
    fn test_load_config_invalid_toml() {
        let dir = tempdir().unwrap();
        let config_file = dir.path().join(".pinner.toml");
        // Write invalid TOML (missing closing bracket)
        fs::write(&config_file, "ignore_actions = [\"a\", \"b\"").unwrap();

        let result = Operations::<ReqwestGithubProvider>::load_config_from_path(&config_file);

        assert!(result.is_err());
        match result.unwrap_err() {
            PinnerError::Config(msg) => {
                assert!(msg.contains("Failed to parse"));
            }
            _ => panic!("Expected PinnerError::Config"),
        }
    }

    #[tokio::test]
    async fn test_github_provider_token() {
        std::env::set_var("GITHUB_TOKEN", "test-token");
        let p = ReqwestGithubProvider::new("http://localhost".into(), None);
        assert_eq!(p.base_url, "http://localhost");
        std::env::remove_var("GITHUB_TOKEN");
    }
}
