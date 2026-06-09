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
//!     let ops = Operations::new(Arc::new(github), true, false, false);
//!     ops.pin(&[PathBuf::from(".github/workflows")]).await.unwrap();
//! }
//! ```

use async_trait::async_trait;
use clap::{Parser, Subcommand};
use colored::Colorize;
use futures::future::join_all;
use ignore::WalkBuilder;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde::Deserialize;
use similar::{ChangeTag, TextDiff};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
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
}

/// Command line arguments structure.
#[derive(Parser)]
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
    let ops = Operations::new(Arc::new(github), cli.yes, cli.quiet, cli.dry_run);
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
}

/// Default implementation of [`GithubProvider`] using `reqwest`.
pub struct ReqwestGithubProvider {
    client: ClientWithMiddleware,
    base_url: String,
}

#[cfg(not(tarpaulin))]
impl Default for ReqwestGithubProvider {
    fn default() -> Self {
        Self::new("https://api.github.com".to_string())
    }
}

impl ReqwestGithubProvider {
    /// Creates a new [`ReqwestGithubProvider`] with the specified base URL.
    pub fn new(base_url: String) -> Self {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, HeaderValue::from_static("pinner"));
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            if let Ok(auth) = HeaderValue::from_str(&format!("Bearer {}", token)) {
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

        Self { client, base_url }
    }
}

#[async_trait]
impl GithubProvider for ReqwestGithubProvider {
    async fn get_commit_sha(&self, action: &str, tag: &str) -> Result<String, PinnerError> {
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
            Ok(rel.tag_name)
        } else if resp.status().as_u16() == 404 {
            Ok("main".to_string())
        } else {
            Err(PinnerError::Api(format!(
                "HTTP {}: Could not fetch latest release for {}",
                resp.status(),
                action
            )))
        }
    }
}

/// Orchestrator for pinning operations.
pub struct Operations<G: GithubProvider> {
    github: Arc<G>,
    yes: bool,
    quiet: bool,
    dry_run: bool,
}

struct UpdateTask {
    path: PathBuf,
    start: usize,
    end: usize,
    action: String,
    current_tag: Option<String>,
}

struct UpdateResult {
    task: UpdateTask,
    new_sha: String,
    new_tag: Option<String>,
}

impl<G: GithubProvider + 'static> Operations<G> {
    pub fn new(github: Arc<G>, yes: bool, quiet: bool, dry_run: bool) -> Self {
        Self {
            github,
            yes,
            quiet,
            dry_run,
        }
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
        let mut futs = Vec::new();
        for task in tasks {
            let f_clone = f.clone();
            futs.push(async move {
                if let Some((sha, tag)) = f_clone(&task.action, task.current_tag.as_deref()).await {
                    Some(UpdateResult {
                        task,
                        new_sha: sha,
                        new_tag: tag,
                    })
                } else {
                    None
                }
            });
        }

        let results: Vec<UpdateResult> = join_all(futs).await.into_iter().flatten().collect();

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

                let new_comment = if let Some(t) = res.new_tag {
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
            }

            if !changes.is_empty() && !self.quiet {
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
                        std::io::stdout().flush().unwrap();
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
            } else if !changes.is_empty() && self.yes {
                fs::write(&path, new_content)?;
            }
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

        let p = ReqwestGithubProvider::new(s.url());
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

        let ops = Operations::new(Arc::new(mock), true, false, false);
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
        let ops2 = Operations::new(Arc::new(mock2), true, false, false);
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
        let p = ReqwestGithubProvider::new(s.url());

        let _m = s
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(404)
            .create_async()
            .await;
        assert!(p.get_commit_sha("o/r", "v1").await.is_err());

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
    }

    #[tokio::test]
    async fn test_operations_set() {
        let mock = MockGithubProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let ops = Operations::new(Arc::new(mock), true, true, false);
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

        let ops = Operations::new(Arc::new(mock), true, false, true);
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

        let ops = Operations::new(Arc::new(mock), true, true, false);
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

        let ops = Operations::new(Arc::new(mock), true, true, false);
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
        };
        run(cli_set, mock2, vec![f.clone()]).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("o/r@sethash"));
    }

    #[tokio::test]
    async fn test_error_path_not_found() {
        let mock = MockGithubProvider::new();
        let ops = Operations::new(Arc::new(mock), true, true, false);
        let res = ops.pin(&[PathBuf::from("/non/existent/path")]).await;
        assert!(matches!(res, Err(PinnerError::PathNotFound(_))));
    }

    #[tokio::test]
    async fn test_github_provider_token() {
        std::env::set_var("GITHUB_TOKEN", "test-token");
        let p = ReqwestGithubProvider::new("http://localhost".into());
        assert_eq!(p.base_url, "http://localhost");
        std::env::remove_var("GITHUB_TOKEN");
    }
}
