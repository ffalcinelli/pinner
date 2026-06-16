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

pub struct Patcher {
    pub formatter: Formatter,
    pub ui: Arc<dyn UserInterface>,
    pub dry_run: bool,
}

pub struct FilePatch {
    pub path: PathBuf,
    pub original_content: String,
    pub new_content: String,
    pub changes: Vec<(String, String)>,
    pub results: Vec<UpdateResult>,
}

impl Patcher {
    pub fn new(formatter: Formatter, ui: Arc<dyn UserInterface>, dry_run: bool) -> Self {
        Self {
            formatter,
            ui,
            dry_run,
        }
    }

    /// Calculates patches for each file based on update results.
    /// This is a pure computation without I/O or UI side effects.
    pub fn calculate_patches(
        &self,
        results: Vec<UpdateResult>,
        file_contents: &HashMap<PathBuf, String>,
    ) -> Result<Vec<FilePatch>, PinnerError> {
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

    /// Applies patches to files, handling UI (diffs, prompts) and I/O.
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
                        self.formatter
                            .format_diff(&patch.original_content, &patch.new_content)
                    );
                } else {
                    for (old, new) in &patch.changes {
                        print!("{}", self.formatter.format_inline_diff(old, new));
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
                fs::write(&patch.path, patch.new_content)?;
                all_results.extend(patch.results);
            } else if self.dry_run {
                // In dry-run but non-text mode (e.g. JSON), we still want the results
                all_results.extend(patch.results);
            }
        }

        self.formatter.print_results(&all_results);

        Ok(())
    }

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
        let formatter = Formatter::new(OutputFormat::Text, false);
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
}
