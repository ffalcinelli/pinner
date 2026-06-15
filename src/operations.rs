//! Core logic for pinning and upgrading actions.
//!
//! This module contains the [`Operations`] struct, which is the primary orchestrator
//! for finding, fetching, and replacing action tags in YAML files.

use crate::cli::{OutputFormat, UpgradeStrategy};
use crate::error::PinnerError;
use crate::providers::{DependencyName, DependencyRef, RemoteProvider};
use crate::registry::RegistryProvider;
use crate::yaml::{find_uses_nodes, CiProvider};
use colored::Colorize;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use futures::stream::{self, StreamExt};
use ignore::WalkBuilder;
use indicatif::{ProgressBar, ProgressStyle};
use regex::Regex;
use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use tree_sitter::Parser as TSParser;

fn default_concurrency() -> usize {
    10
}

/// Configuration for Pinner, typically loaded from a `.pinner.toml` file.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    /// List of actions or image patterns to ignore.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Number of concurrent API requests to make.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Base URL for the GitHub API.
    pub github_url: Option<String>,
    /// Base URL for the Bitbucket API.
    pub bitbucket_url: Option<String>,
    /// Base URL for the GitLab API.
    pub gitlab_url: Option<String>,
    /// Base URL for the Forgejo/Gitea API.
    pub forgejo_url: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore: Vec::new(),
            concurrency: default_concurrency(),
            github_url: None,
            bitbucket_url: None,
            gitlab_url: None,
            forgejo_url: None,
        }
    }
}

/// Orchestrator for pinning and upgrading operations.
pub struct Operations<G: RemoteProvider, R: RegistryProvider> {
    github: Arc<G>,
    registry: Arc<R>,
    yes: bool,
    quiet: bool,
    dry_run: bool,
    format: OutputFormat,
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
    pub format: OutputFormat,
    pub upgrade_strategy: UpgradeStrategy,
    pub concurrency: Option<usize>,
    pub ignore: Vec<String>,
}

impl<G: RemoteProvider + 'static, R: RegistryProvider + 'static> Operations<G, R> {
    pub fn new(github: Arc<G>, registry: Arc<R>, options: OperationsOptions) -> Self {
        let mut config = Self::load_config().unwrap_or_default();

        // Override config with CLI options
        if let Some(c) = options.concurrency {
            config.concurrency = c;
        }
        if !options.ignore.is_empty() {
            config.ignore.extend(options.ignore);
        }

        Self {
            github,
            registry,
            yes: options.yes,
            quiet: options.quiet,
            dry_run: options.dry_run,
            format: options.format,
            upgrade_strategy: options.upgrade_strategy,
            config,
            #[cfg(test)]
            force_confirm: None,
        }
    }

    /// Loads the configuration using figment (File -> Env -> Defaults).
    fn load_config() -> Result<Config, PinnerError> {
        let figment = Figment::new()
            .merge(Toml::file(".pinner.toml"))
            .merge(Env::prefixed("PINNER_"));

        figment
            .extract()
            .map_err(|e| PinnerError::Config(format!("Failed to load configuration: {}", e)))
    }

    pub fn load_config_from_path(path: &Path) -> Result<Config, PinnerError> {
        Figment::new()
            .merge(Toml::file(path))
            .extract()
            .map_err(|e| {
                PinnerError::Config(format!("Failed to load config from {:?}: {}", path, e))
            })
    }

    /// Returns true if the action should be ignored.
    fn is_ignored(&self, action: &DependencyName) -> bool {
        self.config
            .ignore
            .iter()
            .any(|pattern| action.0.contains(pattern))
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
        let mut all_paths = Vec::new();

        for path in paths {
            if !path.exists() {
                continue;
            }

            for entry in WalkBuilder::new(path).build() {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
                    all_paths.push(path.to_path_buf());
                }
            }
        }

        use rayon::prelude::*;
        let unpinned: Vec<(PathBuf, String)> = all_paths
            .into_par_iter()
            .map_init(
                || {
                    let mut parser = TSParser::new();
                    parser
                        .set_language(tree_sitter_yaml::language())
                        .expect("Failed to load YAML grammar");
                    parser
                },
                |parser, path| {
                    let content = fs::read_to_string(&path)?;

                    let tree = parser.parse(&content, None).ok_or_else(|| {
                        PinnerError::Parse(format!("Failed to parse {}", path.display()))
                    })?;

                    let provider = CiProvider::from_path(&path);
                    let uses_nodes =
                        find_uses_nodes(tree.root_node(), content.as_bytes(), provider);

                    let mut local_unpinned = Vec::new();
                    for node in uses_nodes {
                        if node.key == "include" || node.key == "project" {
                            continue;
                        }
                        if node.value.starts_with("./") {
                            continue;
                        }

                        let (action_part, tag) =
                            node.value.split_once('@').unwrap_or((&node.value, ""));
                        let action = DependencyName::from(action_part);

                        if self.is_ignored(&action) {
                            continue;
                        }

                        let is_pinned = if tag.is_empty() {
                            false
                        } else {
                            (tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit()))
                                || (node.value.contains("@sha256:")
                                    && node
                                        .value
                                        .split_once("@sha256:")
                                        .is_some_and(|(_, s)| s.len() == 64))
                        };

                        if !is_pinned {
                            local_unpinned.push((path.clone(), node.value));
                        }
                    }
                    Ok(local_unpinned)
                },
            )
            .collect::<Result<Vec<Vec<(PathBuf, String)>>, PinnerError>>()?
            .into_iter()
            .flatten()
            .collect();

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
        let mut all_paths = Vec::new();

        for path in paths {
            if !path.exists() {
                return Err(PinnerError::PathNotFound(path.display().to_string()));
            }

            for entry in WalkBuilder::new(path).build() {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
                    all_paths.push(path.to_path_buf());
                }
            }
        }

        use rayon::prelude::*;
        type CollectResult = Result<(Vec<UpdateTask>, (PathBuf, String)), PinnerError>;
        let results: Vec<CollectResult> = all_paths
            .into_par_iter()
            .map_init(
                || {
                    let mut parser = TSParser::new();
                    parser
                        .set_language(tree_sitter_yaml::language())
                        .expect("Failed to load YAML grammar");
                    parser
                },
                |parser, path| {
                    let content = fs::read_to_string(&path)?;

                    let tree = parser.parse(&content, None).ok_or_else(|| {
                        PinnerError::Parse(format!("Failed to parse {}", path.display()))
                    })?;

                    let provider = CiProvider::from_path(&path);
                    let uses_nodes =
                        find_uses_nodes(tree.root_node(), content.as_bytes(), provider);

                    let mut tasks = Vec::new();
                    for node in uses_nodes {
                        if node.key == "include" || node.key == "project" {
                            continue;
                        }
                        if node.value.starts_with("./") {
                            continue;
                        }
                        let (action_part, tag) = if let Some((a, t)) = node.value.split_once('@') {
                            (a, Some(t))
                        } else if node.value.starts_with("docker://") && node.value.contains(':') {
                            let last_colon = node.value.rfind(':').unwrap();
                            (
                                &node.value[..last_colon],
                                Some(&node.value[last_colon + 1..]),
                            )
                        } else if let Some((a, t)) = node.value.split_once(':') {
                            (a, Some(t))
                        } else {
                            (node.value.as_str(), None)
                        };

                        let action = DependencyName::from(action_part);

                        if self.is_ignored(&action) {
                            continue;
                        }

                        tasks.push(UpdateTask {
                            path: path.clone(),
                            start: node.start,
                            end: node.end,
                            action,
                            current_tag: tag.map(|s| s.to_string()),
                            comment: node.comment,
                            key: node.key,
                        });
                    }
                    Ok((tasks, (path, content)))
                },
            )
            .collect();

        let mut final_tasks = Vec::new();
        let mut final_file_contents = std::collections::HashMap::new();

        for res in results {
            let (tasks, (path, content)) = res?;
            final_tasks.extend(tasks);
            final_file_contents.insert(path, content);
        }

        Ok((final_tasks, final_file_contents))
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
        let is_structured =
            self.format == OutputFormat::Json || self.format == OutputFormat::Markdown;
        let pb = if !self.quiet && !is_structured && !tasks.is_empty() {
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
                    Err(e) if e.is_fatal() => Some(Err(e)),
                    Err(e) => {
                        if !self.quiet {
                            eprintln!(
                                "{} Skipping action due to error: {}",
                                "Warning:".yellow(),
                                e
                            );
                        }
                        None
                    }
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
                if self.format == OutputFormat::Json || self.format == OutputFormat::Markdown {
                    all_json_updates.push(res);
                }
            }

            if !changes.is_empty()
                && !self.quiet
                && self.format != OutputFormat::Json
                && self.format != OutputFormat::Markdown
            {
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
            } else if !changes.is_empty()
                && (self.yes
                    || self.format == OutputFormat::Json
                    || self.format == OutputFormat::Markdown)
                && !self.dry_run
            {
                fs::write(&path, new_content)?;
            }
        }

        match self.format {
            OutputFormat::Json => {
                let output = JsonOutput {
                    updates: all_json_updates,
                };
                println!(
                    "{}",
                    serde_json::to_string_pretty(&output).expect("Failed to serialize JSON output")
                );
            }
            OutputFormat::Markdown => {
                println!("\n# Pinner Update Summary\n");
                println!("| File | Action | Old Ref | New SHA |");
                println!("|------|--------|---------|---------|");
                for res in all_json_updates {
                    println!(
                        "| `{}` | `{}` | `{}` | `{}` |",
                        res.path.display(),
                        res.action,
                        res.old_tag.as_deref().unwrap_or("-"),
                        res.new_sha
                    );
                }
            }
            _ => {}
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
    ///         format: pinner::cli::OutputFormat::Text,
    ///         upgrade_strategy: UpgradeStrategy::Latest,
    ///         concurrency: None,
    ///         ignore: vec![],
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
    use crate::providers::{BranchName, DependencyRef, MockRemoteProvider};
    use crate::registry::{MockRegistryProvider, OciRegistryProvider};
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_operations_config_overrides() {
        let mock = MockRemoteProvider::new();
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: Some(5),
                ignore: vec!["actions/checkout".into()],
            },
        );
        assert_eq!(ops.config.concurrency, 5);
        assert!(ops.config.ignore.contains(&"actions/checkout".to_string()));
    }

    #[test]
    fn test_load_config_from_path_error() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("invalid.toml");
        fs::write(&f, "invalid = toml = format").unwrap();
        let res = Operations::<MockRemoteProvider, OciRegistryProvider>::load_config_from_path(&f);
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_operations_upgrade_strategy_commit() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v1").unwrap();

        let mut mock = MockRemoteProvider::new();
        mock.expect_get_default_branch()
            .returning(|_, _| Ok(BranchName("develop".to_string())));
        mock.expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::GitSha("developsha".to_string())));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Commit,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.upgrade(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("uses: actions/checkout@developsha # develop"));
    }

    #[tokio::test]
    async fn test_operations_non_fatal_error_skipping() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v1").unwrap();

        let mut mock = MockRemoteProvider::new();
        // Return a non-fatal API error
        mock.expect_get_latest_release()
            .returning(|_, _| Err(PinnerError::Api("404 Not Found".into())));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: false, // Show warning
                dry_run: false,
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        // Should not fail the whole operation
        ops.upgrade(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("uses: actions/checkout@v1")); // Unchanged
    }

    #[tokio::test]
    async fn test_operations_image_fallback_latest() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "image: nginx:latest").unwrap();

        let mut registry = MockRegistryProvider::new();
        registry
            .expect_resolve_digest()
            .with(
                mockall::predicate::eq("nginx"),
                mockall::predicate::eq("latest"),
            )
            .returning(|_, _| Ok("sha256:latest".to_string()));

        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(registry),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("image: nginx@sha256:latest # latest"));
    }

    #[tokio::test]
    async fn test_operations_ignore_actions() {
        let mut mock = MockRemoteProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: ignore/me@v1\nuses: keep/me@v1").unwrap();

        let mut config = Config::default();
        config.ignore.push("ignore/me".to_string());

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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
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
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        let res = ops.pin(&[PathBuf::from("/non/existent/path/12345")]).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_operations_set() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v1\nuses: other/action@v2").unwrap();

        let mock = MockRemoteProvider::new();
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.set(
            std::slice::from_ref(&f),
            "actions/checkout",
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        )
        .await
        .unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"));
        assert!(content.contains("uses: other/action@v2"));
    }

    #[tokio::test]
    async fn test_operations_upgrade_latest() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v1").unwrap();

        let mut mock = MockRemoteProvider::new();
        mock.expect_get_latest_release()
            .returning(|_, _| Ok("v2".to_string()));
        mock.expect_get_commit_sha()
            .returning(|_, tag, _| Ok(DependencyRef::GitSha(format!("{}sha", tag))));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.upgrade(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("uses: actions/checkout@v2sha # v2"));
    }

    #[tokio::test]
    async fn test_operations_upgrade_major() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v1.0.0").unwrap();

        let mut mock = MockRemoteProvider::new();
        mock.expect_list_tags()
            .returning(|_, _| Ok(vec!["v1.1.0".to_string(), "v2.0.0".to_string()]));
        mock.expect_get_commit_sha()
            .returning(|_, tag, _| Ok(DependencyRef::GitSha(format!("{}sha", tag))));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Major,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.upgrade(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("uses: actions/checkout@v1.1.0sha # v1.1.0"));
        assert!(!content.contains("v2.0.0"));
    }
}
