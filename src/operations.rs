//! Core logic for pinning and upgrading actions.
//!
//! This module contains the [`Operations`] struct, which is the primary orchestrator
//! for finding, fetching, and replacing action tags in YAML files.

use crate::cli::UpgradeStrategy;
use crate::error::PinnerError;
use crate::providers::{DependencyName, DependencyRef, RemoteProvider};
use crate::registry::RegistryProvider;
use crate::yaml::find_uses_nodes;
use colored::Colorize;
use futures::stream::{self, StreamExt};
use ignore::WalkBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use tree_sitter::Parser as TSParser;

fn default_concurrency() -> usize {
    10
}

/// Configuration for Pinner, typically loaded from a `.pinner.toml` file.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// List of actions to ignore during pinning or upgrading.
    #[serde(default)]
    pub ignore_actions: HashSet<DependencyName>,
    /// Number of concurrent API requests to make.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Custom API URL for the primary forge (e.g., GitHub Enterprise).
    #[serde(default, alias = "github_url")]
    pub api_url: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore_actions: HashSet::new(),
            concurrency: default_concurrency(),
            api_url: None,
        }
    }
}

/// Orchestrator for pinning and upgrading operations.
///
/// This struct holds the shared state and configuration needed to perform
/// updates across multiple files.
pub struct Operations<G: RemoteProvider, R: RegistryProvider> {
    github: Arc<G>,
    registry: Arc<R>,
    yes: bool,
    quiet: bool,
    dry_run: bool,
    json: bool,
    upgrade_strategy: UpgradeStrategy,
    /// The current configuration.
    pub config: Config,
    #[cfg(test)]
    pub force_confirm: Option<bool>,
}

/// Represents a specific location in a file that needs to be updated.
pub struct UpdateTask {
    /// Path to the file containing the dependency.
    pub path: PathBuf,
    /// Byte offset where the dependency value starts.
    pub start: usize,
    /// Byte offset where the dependency value ends.
    pub end: usize,
    /// The name of the action or dependency.
    pub action: DependencyName,
    /// The current tag or ref (if any).
    pub current_tag: Option<String>,
    /// Any existing comment following the dependency.
    pub comment: Option<String>,
    /// The YAML key used (e.g., "uses", "image", "pipe").
    pub key: String,
}

/// The result of a successful update operation.
#[derive(Serialize)]
pub struct UpdateResult {
    /// The task that was executed.
    #[serde(skip)]
    pub task: UpdateTask,
    /// The name of the updated action.
    pub action: DependencyName,
    /// The path to the modified file.
    pub path: PathBuf,
    /// The previous tag or ref.
    pub old_tag: Option<String>,
    /// The new immutable SHA or digest.
    pub new_sha: DependencyRef,
    /// The new tag (used as a comment for readability).
    pub new_tag: Option<String>,
}

#[derive(Serialize)]
pub struct JsonOutput {
    pub updates: Vec<UpdateResult>,
}

/// Options for configuring Operations.
pub struct OperationsOptions {
    pub yes: bool,
    pub quiet: bool,
    pub dry_run: bool,
    pub json: bool,
    pub upgrade_strategy: UpgradeStrategy,
    pub concurrency: Option<usize>,
}

impl<G: RemoteProvider + 'static, R: RegistryProvider + 'static> Operations<G, R> {
    pub fn new(github: Arc<G>, registry: Arc<R>, options: OperationsOptions) -> Self {
        let mut config = Self::load_config().unwrap_or_default();
        if let Some(c) = options.concurrency {
            config.concurrency = c;
        }
        Self {
            github,
            registry,
            yes: options.yes,
            quiet: options.quiet,
            dry_run: options.dry_run,
            json: options.json,
            upgrade_strategy: options.upgrade_strategy,
            config,
            #[cfg(test)]
            force_confirm: None,
        }
    }

    fn load_config() -> Result<Config, PinnerError> {
        let mut current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        loop {
            let config_path = current_dir.join(".pinner.toml");
            if config_path.exists() {
                return Self::load_config_from_path(&config_path);
            }
            if let Some(parent) = current_dir.parent() {
                current_dir = parent.to_path_buf();
            } else {
                break;
            }
        }
        Ok(Config::default())
    }

    pub fn load_config_from_path(path: &Path) -> Result<Config, PinnerError> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let content = fs::read_to_string(path).unwrap_or_default();
        let config: Config = toml::from_str(&content)
            .map_err(|e| PinnerError::Config(format!("Failed to parse .pinner.toml: {}", e)))?;
        Ok(config)
    }

    pub async fn pin(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let github = self.github.clone();
        let registry = self.registry.clone();
        self.process(paths, move |action, tag, key| {
            let a = action.clone();
            let t = tag.map(|s| s.to_string());
            let k = key.to_string();
            let github = github.clone();
            let registry = registry.clone();
            async move {
                if let Some(ver) = t {
                    if a.0.starts_with("docker://") || k == "image" {
                        if !ver.starts_with("sha256:") {
                            let image = a.0.trim_start_matches("docker://");
                            let digest = registry.resolve_digest(image, &ver).await?;
                            return Ok(Some((DependencyRef::from(digest), Some(ver))));
                        }
                    } else if ver.len() != 40 {
                        let sha = github.get_commit_sha(&a, &ver, &k).await?;
                        return Ok(Some((sha, Some(ver))));
                    }
                }
                Ok(None)
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
        let a = DependencyName::from(action);
        let h = DependencyRef::from(hash.to_string());
        self.process(paths, move |act, _, _| {
            let (a, h, act_owned) = (a.clone(), h.clone(), act.clone());
            async move {
                if act_owned == a {
                    Ok(Some((h, None)))
                } else {
                    Ok(None)
                }
            }
        })
        .await
    }

    pub async fn upgrade(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let github = self.github.clone();
        let registry = self.registry.clone();
        let strategy = self.upgrade_strategy.clone();
        self.process(paths, move |a, current_tag, key| {
            let a = a.clone();
            let k = key.to_string();
            let github = github.clone();
            let registry = registry.clone();
            let strategy = strategy.clone();
            let current_tag = current_tag.map(|s| s.to_string());
            async move {
                if a.0.starts_with("docker://") || k == "image" {
                    let image = a.0.trim_start_matches("docker://");
                    if let Some(ver) = &current_tag {
                        let digest = registry.resolve_digest(image, ver).await?;
                        return Ok(Some((DependencyRef::from(digest), Some(ver.clone()))));
                    } else {
                        // Fallback to latest tag for images if no tag specified
                        let digest = registry.resolve_digest(image, "latest").await?;
                        return Ok(Some((
                            DependencyRef::from(digest),
                            Some("latest".to_string()),
                        )));
                    }
                }

                if strategy == UpgradeStrategy::Commit {
                    let branch = github.get_default_branch(&a, &k).await?;
                    let sha = github.get_commit_sha(&a, &branch.0, &k).await?;
                    return Ok(Some((sha, Some(branch.0))));
                }

                let latest_tag = if strategy == UpgradeStrategy::Latest {
                    Some(github.get_latest_release(&a, &k).await?)
                } else {
                    let tags = github.list_tags(&a, &k).await?;
                    let current_tag = current_tag.as_deref().unwrap_or("");
                    let current_version =
                        semver::Version::parse(current_tag.trim_start_matches('v')).ok();

                    let mut filtered_tags: Vec<_> = tags
                        .into_iter()
                        .filter_map(|t| {
                            semver::Version::parse(t.trim_start_matches('v'))
                                .ok()
                                .map(|v| (t, v))
                        })
                        .collect();

                    filtered_tags.sort_by(|a, b| b.1.cmp(&a.1));

                    if let Some(cv) = current_version {
                        filtered_tags
                            .into_iter()
                            .find(|(_, v)| match strategy {
                                UpgradeStrategy::Major => v.major == cv.major && v > &cv,
                                UpgradeStrategy::Minor => {
                                    v.major == cv.major && v.minor == cv.minor && v > &cv
                                }
                                _ => false,
                            })
                            .map(|(t, _)| t)
                    } else {
                        None
                    }
                };

                if let Some(tag) = latest_tag {
                    if Some(&tag) != current_tag.as_ref() {
                        let sha = github.get_commit_sha(&a, &tag, &k).await?;
                        return Ok(Some((sha, Some(tag))));
                    }
                }

                Ok(None)
            }
        })
        .await
    }

    pub async fn verify(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let mut parser = TSParser::new();
        parser
            .set_language(tree_sitter_yaml::language())
            .map_err(|e| PinnerError::Parse(e.to_string()))?;

        let mut unpinned = Vec::new();

        for path in paths {
            if !path.exists() {
                continue;
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
                    find_uses_nodes(tree.root_node(), content.as_bytes(), &mut uses_nodes);

                    for (_, _, val, _, _) in uses_nodes {
                        if val.starts_with("./") {
                            continue;
                        }

                        let (action_part, tag) = val.split_once('@').unwrap_or((&val, ""));
                        let action = DependencyName::from(action_part);

                        if self.config.ignore_actions.contains(&action) {
                            continue;
                        }

                        let is_pinned = if tag.is_empty() {
                            false
                        } else {
                            (tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit()))
                                || (val.contains("@sha256:")
                                    && val
                                        .split_once("@sha256:")
                                        .is_some_and(|(_, s)| s.len() == 64))
                        };

                        if !is_pinned {
                            unpinned.push((path.to_path_buf(), val));
                        }
                    }
                }
            }
        }

        if !unpinned.is_empty() {
            if !self.quiet {
                println!(
                    "{}",
                    "Verification failed! Unpinned actions found:".red().bold()
                );
                for (path, action) in &unpinned {
                    println!(
                        "  {} in {}",
                        action.yellow(),
                        path.display().to_string().cyan()
                    );
                }
            }
            return Err(PinnerError::VerificationFailed(
                "Some actions are not pinned to a SHA".into(),
            ));
        }

        if !self.quiet {
            println!("{}", "✔ All actions are correctly pinned!".green().bold());
        }
        Ok(())
    }

    async fn collect_tasks(
        &self,
        paths: &[PathBuf],
    ) -> Result<(Vec<UpdateTask>, std::collections::HashMap<PathBuf, String>), PinnerError> {
        let mut parser = TSParser::new();
        parser
            .set_language(tree_sitter_yaml::language())
            .map_err(|e| PinnerError::Parse(e.to_string()))?;

        let mut tasks = Vec::new();
        let mut file_contents = std::collections::HashMap::new();

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
                    find_uses_nodes(tree.root_node(), content.as_bytes(), &mut uses_nodes);
                    file_contents.insert(path.to_path_buf(), content);

                    for (start, end, val, comment, key) in uses_nodes {
                        if key == "include" || key == "project" {
                            continue;
                        }
                        if val.starts_with("./") {
                            continue;
                        }
                        let (action_part, tag) = if let Some((a, t)) = val.split_once('@') {
                            (a, Some(t))
                        } else if val.starts_with("docker://") && val.contains(':') {
                            let last_colon = val.rfind(':').unwrap();
                            (&val[..last_colon], Some(&val[last_colon + 1..]))
                        } else if let Some((a, t)) = val.split_once(':') {
                            (a, Some(t))
                        } else {
                            (val.as_str(), None)
                        };

                        let action = DependencyName::from(action_part);

                        if self.config.ignore_actions.contains(&action) {
                            continue;
                        }

                        tasks.push(UpdateTask {
                            path: path.to_path_buf(),
                            start,
                            end,
                            action,
                            current_tag: tag.map(|s| s.to_string()),
                            comment,
                            key,
                        });
                    }
                }
            }
        }
        Ok((tasks, file_contents))
    }

    async fn execute_updates<F, Fut>(
        &self,
        tasks: Vec<UpdateTask>,
        f: Arc<F>,
    ) -> Result<Vec<UpdateResult>, PinnerError>
    where
        F: Fn(&DependencyName, Option<&str>, &str) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<
                Output = Result<Option<(DependencyRef, Option<String>)>, PinnerError>,
            > + Send,
    {
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
                let res = match f_clone(&task.action, task.current_tag.as_deref(), &task.key).await
                {
                    Ok(Some((sha, tag))) => Ok(Some(UpdateResult {
                        action: task.action.clone(),
                        path: task.path.clone(),
                        old_tag: task.current_tag.clone(),
                        task,
                        new_sha: sha,
                        new_tag: tag,
                    })),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                };
                if let Some(p) = pb {
                    p.inc(1);
                }
                res
            }
        });

        let results: Vec<Result<UpdateResult, PinnerError>> = stream::iter(futs)
            .buffer_unordered(self.config.concurrency)
            .filter_map(|res| async {
                match res {
                    Ok(Some(r)) => Some(Ok(r)),
                    Ok(None) => None,
                    Err(e) => Some(Err(e)),
                }
            })
            .collect()
            .await;

        if let Some(p) = pb {
            p.finish_and_clear();
        }

        results.into_iter().collect()
    }

    fn apply_changes(
        &self,
        results: Vec<UpdateResult>,
        file_contents: std::collections::HashMap<PathBuf, String>,
    ) -> Result<(), PinnerError> {
        // Group results by file
        let mut file_results: std::collections::HashMap<PathBuf, Vec<UpdateResult>> =
            std::collections::HashMap::new();
        for res in results {
            file_results
                .entry(res.task.path.clone())
                .or_default()
                .push(res);
        }

        static COMMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"^#\s*(v\d[a-zA-Z0-9.\-_]*|main|\d[a-zA-Z0-9.\-_]*)\s*").unwrap()
        });

        let mut all_json_updates = Vec::new();

        for (path, mut updates) in file_results {
            let content = file_contents
                .get(&path)
                .expect("File content should have been read during parsing");
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
                if let Some(parser_comment) = &res.task.comment {
                    // Use the comment captured by the parser if available
                    let c = parser_comment.trim_start_matches('#').trim();
                    if let Some(mat) = COMMENT_REGEX.find(parser_comment) {
                        let matched_comment = mat.as_str().trim_start_matches('#').trim();
                        if matched_comment == c {
                            final_suffix = "".to_string();
                        }
                    }
                } else if let Some(mat) = COMMENT_REGEX.find(&final_suffix) {
                    final_suffix = final_suffix[mat.end()..].trim_start().to_string();
                    if final_suffix.starts_with('#') {
                        final_suffix = final_suffix[1..].trim_start().to_string();
                    }
                }

                let new_comment = if let Some(t) = &res.new_tag {
                    let is_sha = (t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit()))
                        || t.starts_with("sha256:");
                    if is_sha {
                        "".to_string()
                    } else {
                        format!(" # {}", t)
                    }
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

                let new_val = if res.task.key == "ref" {
                    format!("{}{}{}", res.new_sha, new_comment, extra_suffix)
                } else {
                    let separator = if res.task.key == "pipe" { ":" } else { "@" };
                    format!(
                        "{}{}{}{}{}",
                        res.task.action, separator, res.new_sha, new_comment, extra_suffix
                    )
                };

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
                    self.print_diff(content, &new_content);
                } else {
                    for (old, new_ln) in &changes {
                        self.print_inline_diff(old, new_ln);
                    }
                    #[allow(unused_mut)]
                    let mut should_write = self.yes;
                    #[cfg(test)]
                    let mut mocked = false;
                    #[cfg(test)]
                    if let Some(force) = self.force_confirm {
                        should_write = force;
                        mocked = true;
                    }

                    #[cfg(not(test))]
                    let mocked = false;

                    if !should_write && !self.yes && !mocked {
                        #[cfg(not(tarpaulin))]
                        {
                            use dialoguer::Confirm;
                            should_write = Confirm::new()
                                .with_prompt(format!(
                                    "{} {}?",
                                    "Apply changes to".bold(),
                                    path.display().to_string().cyan()
                                ))
                                .default(false)
                                .interact()
                                .unwrap_or(false);
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
            println!(
                "{}",
                serde_json::to_string_pretty(&output).expect("Failed to serialize JSON output")
            );
        }

        Ok(())
    }

    async fn process<F, Fut>(&self, paths: &[PathBuf], f: F) -> Result<(), PinnerError>
    where
        F: Fn(&DependencyName, Option<&str>, &str) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<
                Output = Result<Option<(DependencyRef, Option<String>)>, PinnerError>,
            > + Send,
    {
        let (tasks, file_contents) = self.collect_tasks(paths).await?;
        let results = self.execute_updates(tasks, Arc::new(f)).await?;
        self.apply_changes(results, file_contents)
    }

    pub fn format_diff(&self, old: &str, new: &str) -> String {
        let mut out = String::new();
        let diff = TextDiff::from_lines(old, new);
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-".red(),
                ChangeTag::Insert => "+".green(),
                ChangeTag::Equal => " ".normal(),
            };
            out.push_str(&format!("{}{}", sign, change));
        }
        out
    }

    /// Prints a standard line-by-line diff.
    pub fn print_diff(&self, old: &str, new: &str) {
        print!("{}", self.format_diff(old, new));
    }

    /// Formats an inline diff for small changes (e.g., a single line).
    ///
    /// # Example
    ///
    /// ```
    /// use pinner::{Operations, OperationsOptions};
    /// use pinner::providers::ReqwestGithubProvider;
    /// use pinner::registry::OciRegistryProvider;
    /// use pinner::cli::UpgradeStrategy;
    /// use std::sync::Arc;
    ///
    /// let ops = Operations::new(
    ///     Arc::new(ReqwestGithubProvider::new("https://api.github.com".to_string(), None)),
    ///     Arc::new(OciRegistryProvider::new(None, None)),
    ///     OperationsOptions {
    ///         yes: true,
    ///         quiet: true,
    ///         dry_run: false,
    ///         json: false,
    ///         upgrade_strategy: UpgradeStrategy::Latest,
    ///         concurrency: None,
    ///     }
    /// );
    ///
    /// let diff = ops.format_inline_diff("actions/checkout@v2", "actions/checkout@hash");
    /// assert!(diff.contains("actions/checkout"));
    /// ```
    pub fn format_inline_diff(&self, old: &str, new: &str) -> String {
        let mut out = String::new();
        let old_trimmed = old.trim();
        let new_trimmed = new.trim();
        let diff = TextDiff::from_words(old_trimmed, new_trimmed);

        out.push_str(&format!("  {} ", "-".red()));
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Delete => out.push_str(&format!("{}", change.value().red())),
                ChangeTag::Equal => out.push_str(&format!("{}", change.value().dimmed())),
                ChangeTag::Insert => {}
            }
        }
        out.push('\n');

        out.push_str(&format!("  {} ", "+".green()));
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Insert => out.push_str(&format!("{}", change.value().green().bold())),
                ChangeTag::Equal => out.push_str(&format!("{}", change.value().yellow())),
                ChangeTag::Delete => {}
            }
        }
        out.push('\n');
        out
    }

    pub fn print_inline_diff(&self, old: &str, new: &str) {
        print!("{}", self.format_inline_diff(old, new));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::MockRemoteProvider;
    use crate::registry::OciRegistryProvider;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_operations_ignore_actions() {
        let mut mock = MockRemoteProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: ignore/me@v1\nuses: keep/me@v1").unwrap();

        let mut config = Config::default();
        config
            .ignore_actions
            .insert(DependencyName::from("ignore/me"));

        mock.expect_get_commit_sha()
            .with(
                mockall::predicate::eq(DependencyName::from("keep/me")),
                mockall::predicate::eq("v1"),
                mockall::predicate::eq("uses"),
            )
            .returning(|_, _, _| Ok(DependencyRef::from("newhash".to_string())));

        let mock_reg = OciRegistryProvider::new(None, None);
        let mut ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );
        ops.config = config;

        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("ignore/me@v1"));
        assert!(content.contains("keep/me@newhash"));
    }

    #[test]
    fn test_operations_diff_methods_sync() {
        let mock = MockRemoteProvider::new();
        let mock_reg = OciRegistryProvider::new(None, None);
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );

        let d = ops.format_diff("line1\n", "line2\n");
        assert!(d.contains("line1"));
        assert!(d.contains("line2"));

        let id = ops.format_inline_diff("old value", "new value");
        assert!(id.contains("old"));
        assert!(id.contains("new"));

        ops.print_diff("a\n", "b\n");
        ops.print_inline_diff("a", "b");
    }

    #[tokio::test]
    async fn test_operations_decomposition() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );

        let (tasks, file_contents) = ops.collect_tasks(std::slice::from_ref(&f)).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].action.to_string(), "o/r");
        assert!(file_contents.contains_key(&f));

        let res = UpdateResult {
            action: DependencyName::from("o/r"),
            path: f.clone(),
            old_tag: Some("v1".to_string()),
            task: tasks.into_iter().next().unwrap(),
            new_sha: DependencyRef::from("newhash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        ops.apply_changes(vec![res], file_contents).unwrap();
        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("o/r@newhash # v2"));
    }

    #[tokio::test]
    async fn test_verify_fail_and_pass() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("unpinned.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );

        let res = ops.verify(std::slice::from_ref(&f)).await;
        assert!(res.is_err()); // Should fail because @v3 is not a SHA

        fs::write(
            &f,
            "uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        )
        .unwrap();
        let res = ops.verify(std::slice::from_ref(&f)).await;
        assert!(res.is_ok());

        // Test docker pinned
        fs::write(
            &f,
            "image: alpine@sha256:1234567890123456789012345678901234567890123456789012345678901234",
        )
        .unwrap();
        let res = ops.verify(std::slice::from_ref(&f)).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_apply_changes_edge_cases() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1 # keep me").unwrap();

        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );

        let (tasks, file_contents) = ops.collect_tasks(std::slice::from_ref(&f)).await.unwrap();
        let res = UpdateResult {
            action: DependencyName::from("o/r"),
            path: f.clone(),
            old_tag: Some("v1".to_string()),
            task: tasks.into_iter().next().unwrap(),
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        ops.apply_changes(vec![res], file_contents).unwrap();
        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("o/r@hash # v2 # keep me"));
    }

    #[tokio::test]
    async fn test_apply_changes_comment_regex() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1 # v1").unwrap(); // Comment matches the vX pattern

        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );

        let (tasks, file_contents) = ops.collect_tasks(std::slice::from_ref(&f)).await.unwrap();
        let res = UpdateResult {
            action: DependencyName::from("o/r"),
            path: f.clone(),
            old_tag: Some("v1".to_string()),
            task: tasks.into_iter().next().unwrap(),
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        ops.apply_changes(vec![res], file_contents).unwrap();
        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("o/r@hash # v2"));
        assert!(!content.contains("# v1"));
    }

    #[tokio::test]
    async fn test_apply_changes_no_redundant_sha_comment() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("config.yml");
        // CircleCI style: image: user/repo@sha256:hash # comment
        fs::write(&f, "image: cimg/base@sha256:35e5e29930ab565475a4f2aa9b4124998ed67dbc7b0e2dd5f420a4189d08d0d2 # stable").unwrap();

        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );

        let (tasks, file_contents) = ops.collect_tasks(std::slice::from_ref(&f)).await.unwrap();
        assert_eq!(tasks.len(), 1);
        let task = tasks.into_iter().next().unwrap();

        let res = UpdateResult {
            action: DependencyName::from("cimg/base"),
            path: f.clone(),
            old_tag: task.current_tag.clone(),
            task,
            new_sha: DependencyRef::from(
                "sha256:35e5e29930ab565475a4f2aa9b4124998ed67dbc7b0e2dd5f420a4189d08d0d2"
                    .to_string(),
            ),
            new_tag: Some(
                "sha256:35e5e29930ab565475a4f2aa9b4124998ed67dbc7b0e2dd5f420a4189d08d0d2"
                    .to_string(),
            ),
        };

        ops.apply_changes(vec![res], file_contents).unwrap();
        let content = fs::read_to_string(&f).unwrap();
        assert!(!content.contains(
            "# sha256:35e5e29930ab565475a4f2aa9b4124998ed67dbc7b0e2dd5f420a4189d08d0d2 # stable"
        ));
        assert!(content.contains("cimg/base@sha256:35e5e29930ab565475a4f2aa9b4124998ed67dbc7b0e2dd5f420a4189d08d0d2 # stable"));
    }

    #[test]
    fn test_load_config_traversal() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let config_path = dir.path().join(".pinner.toml");
        fs::write(&config_path, "concurrency = 42").unwrap();

        // Change current directory to sub
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(&sub).unwrap();

        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );

        assert_eq!(ops.config.concurrency, 42);
        std::env::set_current_dir(original_dir).unwrap();
    }

    #[tokio::test]
    async fn test_operations_path_not_found() {
        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                json: false,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
            },
        );
        let res = ops.pin(&[PathBuf::from("/non/existent/path/12345")]).await;
        assert!(res.is_err());
    }
}
