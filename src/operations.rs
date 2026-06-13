use crate::error::PinnerError;
use crate::github::{ActionName, CommitSha, GithubProvider};
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ignore_actions: HashSet::new(),
            concurrency: default_concurrency(),
        }
    }
}

/// Orchestrator for pinning operations.
pub struct Operations<G: GithubProvider> {
    github: Arc<G>,
    yes: bool,
    quiet: bool,
    dry_run: bool,
    json: bool,
    pub config: Config,
}

pub struct UpdateTask {
    pub path: PathBuf,
    pub start: usize,
    pub end: usize,
    pub action: ActionName,
    pub current_tag: Option<String>,
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
        self.process(paths, move |action, tag| {
            let a = ActionName::from(action);
            let t = tag.map(|s| s.to_string());
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
        self.process(paths, move |a, _| {
            let a = ActionName::from(a);
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

                    for (start, end, val) in uses_nodes {
                        let parts: Vec<&str> = val.split('@').collect();
                        let action = ActionName::from(parts[0]);

                        if self.config.ignore_actions.contains(&action) {
                            continue;
                        }

                        let tag = parts.get(1).copied();
                        tasks.push(UpdateTask {
                            path: path.to_path_buf(),
                            start,
                            end,
                            action,
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
                if let Some(mat) = COMMENT_REGEX.find(&final_suffix) {
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
                    self.print_diff(content, &new_content);
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

        let mut ops = Operations::new(Arc::new(mock), true, true, false, false);
        ops.config = config;

        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("ignore/me@v1"));
        assert!(content.contains("keep/me@newhash"));
    }

    #[test]
    fn test_operations_diff_methods_sync() {
        let mock = MockGithubProvider::new();
        let ops = Operations::new(Arc::new(mock), true, true, false, false);

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
