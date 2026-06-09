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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tree_sitter::{Node, Parser as TSParser};

#[cfg(test)]
use mockall::automock;

#[derive(Error, Debug)]
pub enum PinnerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("API error: {0}")]
    Api(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Directory not found: {0}")]
    DirectoryNotFound(String),
    #[error("Ignore error: {0}")]
    Ignore(#[from] ignore::Error),
}

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
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

#[derive(Subcommand, Debug, PartialEq)]
pub enum Commands {
    Pin,
    Upgrade,
    Set { action: String, hash: String },
}

pub async fn run<G: GithubProvider + 'static>(
    cli: Cli,
    github: G,
    workflows_dir: &Path,
) -> Result<(), PinnerError> {
    let ops = Operations::new(Arc::new(github), cli.yes, cli.quiet, cli.dry_run);
    match cli.command {
        Commands::Pin => ops.pin(workflows_dir).await,
        Commands::Upgrade => ops.upgrade(workflows_dir).await,
        Commands::Set { action, hash } => ops.set(workflows_dir, &action, &hash).await,
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

#[cfg_attr(test, automock)]
#[async_trait]
pub trait GithubProvider: Send + Sync {
    async fn get_commit_sha(&self, action: &str, tag: &str) -> Result<String, PinnerError>;
    async fn get_latest_release(&self, action: &str) -> Result<String, PinnerError>;
}

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

    pub async fn pin(&self, dir: &Path) -> Result<(), PinnerError> {
        let github = self.github.clone();
        self.process(dir, move |action, tag| {
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

    pub async fn set(&self, dir: &Path, action: &str, hash: &str) -> Result<(), PinnerError> {
        let (a, h) = (action.to_string(), hash.to_string());
        self.process(dir, move |act, _| {
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

    pub async fn upgrade(&self, dir: &Path) -> Result<(), PinnerError> {
        let github = self.github.clone();
        self.process(dir, move |a, _| {
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

    async fn process<F, Fut>(&self, dir: &Path, f: F) -> Result<(), PinnerError>
    where
        F: Fn(&str, Option<&str>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Option<(String, Option<String>)>> + Send,
    {
        if !dir.exists() {
            return Err(PinnerError::DirectoryNotFound(dir.display().to_string()));
        }

        let mut parser = TSParser::new();
        parser
            .set_language(tree_sitter_yaml::language())
            .map_err(|e| PinnerError::Parse(e.to_string()))?;

        let mut tasks = Vec::new();

        for entry in WalkBuilder::new(dir).build() {
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
                let old_full = &content[res.task.start..res.task.end];

                let line_end = content[res.task.end..]
                    .find('\n')
                    .map(|pos| res.task.end + pos)
                    .unwrap_or(content.len());
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
                } else {
                    format!(" # {}", final_suffix)
                };

                let new_val = format!(
                    "{}@{}{}{}",
                    res.task.action, res.new_sha, new_comment, extra_suffix
                );

                changes.push((old_full.to_string(), new_val.clone()));
                new_content.replace_range(res.task.start..line_end, &new_val);
            }

            if !changes.is_empty() && !self.quiet {
                println!("\n{} {}", "File:".bold(), path.display().to_string().cyan());
                if self.dry_run {
                    self.print_diff(&content, &new_content);
                } else {
                    for (old, new_ln) in &changes {
                        println!("  {} {}", "-".red(), old.trim().dimmed());
                        println!("  {} {}", "+".green(), new_ln.trim().yellow());
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

        let ops = Operations::new(Arc::new(mock), true, false, false);
        ops.pin(&wd).await.unwrap();
        assert!(fs::read_to_string(wd.join("f.yml"))
            .unwrap()
            .contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));

        let mut mock2 = MockGithubProvider::new();
        mock2
            .expect_get_latest_release()
            .returning(|_| Ok("v3".into()));
        mock2
            .expect_get_commit_sha()
            .returning(|_, _| Ok("692973e3d937129bcbf40652eb9f2f61becf3332".into()));
        let ops2 = Operations::new(Arc::new(mock2), true, false, false);
        ops2.upgrade(&wd).await.unwrap();
        let ut = fs::read_to_string(wd.join("untagged.yml")).unwrap();
        assert!(ut.contains("actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332 # v3"));

        let mut mock3 = MockGithubProvider::new();
        mock3
            .expect_get_commit_sha()
            .returning(|_, _| Ok("s".into()));
        run(
            Cli {
                command: Commands::Pin,
                yes: true,
                quiet: true,
                dry_run: false,
            },
            mock3,
            &wd,
        )
        .await
        .unwrap();
        assert!(run(
            Cli {
                command: Commands::Pin,
                yes: true,
                quiet: true,
                dry_run: false,
            },
            MockGithubProvider::new(),
            Path::new("/n")
        )
        .await
        .is_err());
    }
}
