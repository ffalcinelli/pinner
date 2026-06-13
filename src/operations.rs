use crate::cli::UpgradeStrategy;
use crate::error::PinnerError;
use crate::github::{ActionName, CommitSha, GithubProvider};
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

/// Configuration for Pinner.
#[derive(Debug, Deserialize)]
pub struct Config {
    /// List of actions to ignore.
    #[serde(default)]
    pub ignore_actions: HashSet<ActionName>,
    /// Number of concurrent GitHub API requests.
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Custom GitHub API URL.
    #[serde(default)]
    pub github_url: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore_actions: HashSet::new(),
            concurrency: default_concurrency(),
            github_url: None,
        }
    }
}

/// Orchestrator for pinning operations.
pub struct Operations<G: GithubProvider, R: RegistryProvider> {
    github: Arc<G>,
    registry: Arc<R>,
    yes: bool,
    quiet: bool,
    dry_run: bool,
    json: bool,
    upgrade_strategy: UpgradeStrategy,
    pub config: Config,
    #[cfg(test)]
    pub force_confirm: Option<bool>,
}

pub struct UpdateTask {
    pub path: PathBuf,
    pub start: usize,
    pub end: usize,
    pub action: ActionName,
    pub current_tag: Option<String>,
    pub comment: Option<String>,
}

#[derive(Serialize)]
pub struct UpdateResult {
    #[serde(skip)]
    pub task: UpdateTask,
    pub action: ActionName,
    pub path: PathBuf,
    pub old_tag: Option<String>,
    pub new_sha: CommitSha,
    pub new_tag: Option<String>,
}

#[derive(Serialize)]
pub struct JsonOutput {
    pub updates: Vec<UpdateResult>,
}

impl<G: GithubProvider + 'static, R: RegistryProvider + 'static> Operations<G, R> {
    pub fn new(
        github: Arc<G>,
        registry: Arc<R>,
        yes: bool,
        quiet: bool,
        dry_run: bool,
        json: bool,
        upgrade_strategy: UpgradeStrategy,
    ) -> Self {
        let config = Self::load_config().unwrap_or_default();
        Self {
            github,
            registry,
            yes,
            quiet,
            dry_run,
            json,
            upgrade_strategy,
            config,
            #[cfg(test)]
            force_confirm: None,
        }
    }

    fn load_config() -> Result<Config, PinnerError> {
        Self::load_config_from_path(Path::new(".pinner.toml"))
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
        self.process(paths, move |action, tag| {
            let a = ActionName::from(action);
            let t = tag.map(|s| s.to_string());
            let github = github.clone();
            let registry = registry.clone();
            async move {
                if let Some(ver) = t {
                    if a.0.starts_with("docker://") {
                        if !ver.starts_with("sha256:") {
                            let image = a.0.trim_start_matches("docker://");
                            if let Ok(digest) = registry.resolve_digest(image, &ver).await {
                                return Some((CommitSha(digest), Some(ver)));
                            }
                        }
                    } else if ver.len() != 40 {
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
        let a = ActionName::from(action);
        let h = CommitSha::from(hash.to_string());
        self.process(paths, move |act, _| {
            let (a, h, act_owned) = (a.clone(), h.clone(), ActionName::from(act));
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
        let strategy = self.upgrade_strategy.clone();
        self.process(paths, move |a, current_tag| {
            let a = ActionName::from(a);
            let github = github.clone();
            let strategy = strategy.clone();
            let current_tag = current_tag.map(|s| s.to_string());
            async move {
                if a.0.starts_with("docker://") {
                    return None;
                }

                if strategy == UpgradeStrategy::Commit {
                    if let Ok(branch) = github.get_default_branch(&a).await {
                        if let Ok(sha) = github.get_commit_sha(&a, &branch.0).await {
                            return Some((sha, Some(branch.0)));
                        }
                    }
                    return None;
                }

                let latest_tag = if strategy == UpgradeStrategy::Latest {
                    github.get_latest_release(&a).await.ok()
                } else {
                    let tags = github.list_tags(&a).await.unwrap_or_default();
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
                    if let Ok(sha) = github.get_commit_sha(&a, &tag).await {
                        return Some((sha, Some(tag)));
                    }
                }
                None
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

                    for (_, _, val, _) in uses_nodes {
                        if val.starts_with("./") {
                            continue;
                        }

                        let parts: Vec<&str> = val.split('@').collect();
                        let action = ActionName::from(parts[0]);

                        if self.config.ignore_actions.contains(&action) {
                            continue;
                        }

                        let tag = parts.get(1);
                        let is_pinned = tag.is_some_and(|t| {
                            (t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit()))
                                || (val.contains("@sha256:")
                                    && val.split("@sha256:").nth(1).is_some_and(|s| s.len() == 64))
                        });

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
            return Err(PinnerError::Api(
                "Some actions are not pinned to a SHA".into(),
            ));
        }

        if !self.quiet {
            println!("{}", "✔ All actions are correctly pinned!".green().bold());
        }
        Ok(())
    }

    async fn process<F, Fut>(&self, paths: &[PathBuf], f: F) -> Result<(), PinnerError>
    where
        F: Fn(&str, Option<&str>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Option<(CommitSha, Option<String>)>> + Send,
    {
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

                    for (start, end, val, comment) in uses_nodes {
                        if val.starts_with("./") {
                            continue;
                        }
                        let (action_part, tag) = if val.contains('@') {
                            let parts: Vec<&str> = val.split('@').collect();
                            (parts[0], parts.get(1).copied())
                        } else if val.starts_with("docker://") && val.contains(':') {
                            let last_colon = val.rfind(':').unwrap();
                            (&val[..last_colon], Some(&val[last_colon + 1..]))
                        } else if val.contains(':') {
                            let parts: Vec<&str> = val.split(':').collect();
                            (parts[0], parts.get(1).copied())
                        } else {
                            (val.as_str(), None)
                        };

                        let action = ActionName::from(action_part);

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
                    f_clone(&task.action.0, task.current_tag.as_deref()).await
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
            .buffer_unordered(self.config.concurrency)
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
                    if res.action.0.starts_with("docker://") && t.starts_with("sha256:") {
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

                let separator = "@";

                let new_val = format!(
                    "{}{}{}{}{}",
                    res.task.action, separator, res.new_sha, new_comment, extra_suffix
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
                    self.print_diff(content, &new_content);
                } else {
                    for (old, new_ln) in &changes {
                        self.print_inline_diff(old, new_ln);
                    }
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

    pub fn print_diff(&self, old: &str, new: &str) {
        print!("{}", self.format_diff(old, new));
    }

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
    use crate::github::MockGithubProvider;
    use crate::registry::OciRegistryProvider;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_operations_ignore_actions() {
        let mut mock = MockGithubProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: ignore/me@v1\nuses: keep/me@v1").unwrap();

        let mut config = Config::default();
        config.ignore_actions.insert(ActionName::from("ignore/me"));

        mock.expect_get_commit_sha()
            .with(
                mockall::predicate::eq(ActionName::from("keep/me")),
                mockall::predicate::eq("v1"),
            )
            .returning(|_, _| Ok(CommitSha("newhash".into())));

        let mock_reg = OciRegistryProvider::new();
        let mut ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            true,
            true,
            false,
            false,
            UpgradeStrategy::Latest,
        );
        ops.config = config;

        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("ignore/me@v1"));
        assert!(content.contains("keep/me@newhash"));
    }

    #[test]
    fn test_operations_diff_methods_sync() {
        let mock = MockGithubProvider::new();
        let mock_reg = OciRegistryProvider::new();
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            true,
            true,
            false,
            false,
            UpgradeStrategy::Latest,
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
}
