use crate::core::UpdateResult;
use crate::error::PinnerError;
use crate::patcher::formatter::Formatter;
use crate::patcher::mutator::apply_update;
use crate::patcher::ui::UserInterface;
use colored::Colorize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// The `Patcher` coordinates the application of updates to the file system.
///
/// It handles patch calculation, user confirmation (via `UserInterface`), and
/// writing the modified content back to disk.
pub struct Patcher {
    /// Formatter for diffs and results.
    pub formatter: Formatter,
    /// Interface for interacting with the user.
    pub ui: Arc<dyn UserInterface>,
    /// If true, no changes will be written to disk.
    pub dry_run: bool,
}

/// Represents a set of changes to be applied to a single file.
pub struct FilePatch {
    /// Path to the file.
    pub path: PathBuf,
    /// Content of the file before any changes.
    pub original_content: String,
    /// Content of the file after all changes are applied.
    pub new_content: String,
    /// A list of individual string replacements (old, new).
    pub changes: Vec<(String, String)>,
    /// The update results that were applied.
    pub results: Vec<UpdateResult>,
}

impl Patcher {
    /// Creates a new `Patcher`.
    pub fn new(formatter: Formatter, ui: Arc<dyn UserInterface>, dry_run: bool) -> Self {
        Self {
            formatter,
            ui,
            dry_run,
        }
    }

    /// Calculates patches for each file based on update results.
    ///
    /// This is a pure computation without I/O or UI side effects.
    /// Crucially, it sorts updates by their start offset in reverse order.
    /// This ensures that applying an update (which might change the string length)
    /// does not invalidate the byte offsets of subsequent updates in the same file.
    pub fn calculate_patches(
        &self,
        results: Vec<UpdateResult>,
        file_contents: &HashMap<PathBuf, String>,
    ) -> Result<Vec<FilePatch>, PinnerError> {
        // Group results by file path.
        let mut file_results: HashMap<PathBuf, Vec<UpdateResult>> = HashMap::new();
        for res in results {
            file_results
                .entry(res.task.path.clone())
                .or_default()
                .push(res);
        }

        let mut patches = Vec::new();

        for (path, mut updates) in file_results {
            let content = file_contents.get(&path).ok_or_else(|| {
                PinnerError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Content for file {} not found", path.display()),
                ))
            })?;

            let mut new_content = content.clone();
            let mut changes = Vec::new();
            let mut applied_results = Vec::new();

            // Sort in reverse order of start byte to keep offsets valid during mutation.
            updates.sort_by_key(|a| std::cmp::Reverse(a.task.start));

            for res in updates {
                if let Some((old, new)) = apply_update(&mut new_content, &res)? {
                    changes.push((old, new));
                    applied_results.push(res);
                }
            }

            if !changes.is_empty() {
                patches.push(FilePatch {
                    path,
                    original_content: content.clone(),
                    new_content,
                    changes,
                    results: applied_results,
                });
            }
        }

        Ok(patches)
    }

    /// Applies calculated patches to the file system.
    ///
    /// Depending on configuration, it may:
    /// - Print diffs to the console.
    /// - Prompt the user for confirmation.
    /// - Write changes to disk (unless `dry_run` is true).
    pub async fn apply_patches(&self, patches: Vec<FilePatch>) -> Result<(), PinnerError> {
        let mut all_results = Vec::new();

        for patch in patches {
            if !self.formatter.quiet && self.formatter.format == crate::cli::OutputFormat::Text {
                println!(
                    "\n{} {}",
                    "File:".bold(),
                    patch.path.display().to_string().cyan()
                );

                if self.dry_run {
                    print!(
                        "{}",
                        self.formatter.format_diff(
                            &patch.original_content,
                            &patch.new_content,
                            &patch.results
                        )
                    );
                    // In dry-run, we still want to report the "would-be" results.
                    all_results.extend(patch.results);
                } else {
                    for (i, (old, new)) in patch.changes.iter().enumerate() {
                        let res = &patch.results[i];
                        let status = self
                            .formatter
                            .check_hash_security(&res.action.to_string(), &res.new_sha.to_string());
                        print!("{}", self.formatter.format_inline_diff(old, new, status));
                    }

                    if self.ui.confirm_patch(&patch.path) {
                        fs::write(&patch.path, patch.new_content)?;
                        self.ui.report_success(&patch.path);
                        all_results.extend(patch.results);
                    } else {
                        self.ui.report_skipped(&patch.path);
                    }
                }
            } else if !self.dry_run {
                // Non-text mode (e.g. JSON) or quiet mode: apply silently.
                fs::write(&patch.path, patch.new_content)?;
                all_results.extend(patch.results);
            } else if self.dry_run {
                // In dry-run but non-text mode (e.g. JSON), we still want the results.
                all_results.extend(patch.results);
            }
        }

        self.formatter.print_results(&all_results);

        Ok(())
    }

    /// High-level entry point to calculate and apply changes in one go.
    pub async fn apply_changes(
        &self,
        results: Vec<UpdateResult>,
        file_contents: HashMap<PathBuf, String>,
    ) -> Result<(), PinnerError> {
        let patches = self.calculate_patches(results, &file_contents)?;
        self.apply_patches(patches).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::OutputFormat;
    use crate::core::{CiProvider, DependencyRef, UpdateTask};
    use crate::patcher::ui::TestUi;

    #[test]
    fn test_calculate_patches() {
        let formatter = Formatter::new(OutputFormat::Text, false, vec![], vec![], true);
        let ui = Arc::new(TestUi { response: true });
        let patcher = Patcher::new(formatter, ui, false);

        let path = PathBuf::from("ci.yml");
        let content = "uses: actions/checkout@v3".to_string();
        let mut file_contents = HashMap::new();
        file_contents.insert(path.clone(), content.clone());

        let task = UpdateTask {
            path: path.clone(),
            start: 6,
            end: 25,
            line: 1,
            column: 7,
            action: "actions/checkout".into(),
            current_tag: Some("v3".to_string()),
            comment: None,
            key: "uses".to_string(),
            provider: CiProvider::GitHub,
        };

        let result = UpdateResult {
            task,
            action: "actions/checkout".into(),
            path: path.clone(),
            old_tag: Some("v3".to_string()),
            new_sha: DependencyRef::GitSha("hashv3".to_string()),
            new_tag: Some("v3".to_string()),
        };

        let patches = patcher
            .calculate_patches(vec![result], &file_contents)
            .unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].path, path);
        assert!(patches[0]
            .new_content
            .contains("actions/checkout@hashv3 # v3"));
    }

    #[test]
    fn test_calculate_patches_multiple_updates() {
        let formatter = Formatter::new(OutputFormat::Text, false, vec![], vec![], true);
        let ui = Arc::new(TestUi { response: true });
        let patcher = Patcher::new(formatter, ui, false);

        let path = PathBuf::from("ci.yml");
        // Two dependencies on two lines
        let content = "uses: a/b@v1\nuses: c/d@v2".to_string();
        let mut file_contents = HashMap::new();
        file_contents.insert(path.clone(), content.clone());

        let res1 = UpdateResult {
            task: UpdateTask {
                path: path.clone(),
                start: 6,
                end: 12,
                action: "a/b".into(),
                current_tag: Some("v1".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
            action: "a/b".into(),
            path: path.clone(),
            old_tag: Some("v1".to_string()),
            new_sha: DependencyRef::GitSha("sha1".to_string()),
            new_tag: Some("v1".to_string()),
        };

        let res2 = UpdateResult {
            task: UpdateTask {
                path: path.clone(),
                start: 19,
                end: 25,
                action: "c/d".into(),
                current_tag: Some("v2".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
            action: "c/d".into(),
            path: path.clone(),
            old_tag: Some("v2".to_string()),
            new_sha: DependencyRef::GitSha("sha2".to_string()),
            new_tag: Some("v2".to_string()),
        };

        let patches = patcher
            .calculate_patches(vec![res1, res2], &file_contents)
            .unwrap();
        assert_eq!(patches.len(), 1);
        assert!(patches[0].new_content.contains("a/b@sha1 # v1"));
        assert!(patches[0].new_content.contains("c/d@sha2 # v2"));
    }
}
