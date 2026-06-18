use crate::cli::OutputFormat;
use crate::core::UpdateResult;
use colored::Colorize;
use serde::Serialize;
use similar::{ChangeTag, TextDiff};

/// Represents the summary of updates in JSON format.
#[derive(Serialize)]
pub struct JsonOutput {
    /// List of all successful updates.
    pub updates: Vec<UpdateResult>,
}

/// Security status of a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashSecurityStatus {
    /// Hash is explicitly vetted/trusted.
    Vetted,
    /// Hash is explicitly marked as compromised.
    Compromised,
    /// Hash is not checked/verified yet.
    NotChecked,
}

/// Helper to generate visual security labels.
pub fn format_security_status(status: HashSecurityStatus, show: bool) -> String {
    if !show {
        return String::new();
    }
    match status {
        HashSecurityStatus::Vetted => format!("{}", " [✓ vetted]".green().bold()),
        HashSecurityStatus::Compromised => format!("{}", " [✗ compromised]".red().bold()),
        HashSecurityStatus::NotChecked => format!("{}", " [? not checked]".yellow()),
    }
}

/// The `Formatter` is responsible for generating human-readable and machine-readable output.
///
/// It handles:
/// - Line-by-line diffs (using `similar`).
/// - Inline (word-level) diffs for more surgical feedback.
/// - JSON and Markdown summary reports.
pub struct Formatter {
    /// The desired output format (Text, JSON, Markdown).
    pub format: OutputFormat,
    /// If true, suppresses most console output.
    pub quiet: bool,
    /// Vetted dependency hashes or references.
    pub vetted: Vec<String>,
    /// Compromised dependency hashes or references.
    pub compromised: Vec<String>,
    /// Show visual security feedback.
    pub show_security_feedback: bool,
}

impl Formatter {
    /// Creates a new `Formatter`.
    pub fn new(
        format: OutputFormat,
        quiet: bool,
        vetted: Vec<String>,
        compromised: Vec<String>,
        show_security_feedback: bool,
    ) -> Self {
        Self {
            format,
            quiet,
            vetted,
            compromised,
            show_security_feedback,
        }
    }

    /// Helper to check security status of a hash.
    pub fn check_hash_security(&self, action: &str, hash: &str) -> HashSecurityStatus {
        let full_ref_with_at = format!("{}@{}", action, hash);
        let full_ref_with_docker = if action.starts_with("docker://") {
            format!("{}@{}", action.trim_start_matches("docker://"), hash)
        } else {
            format!("docker://{}@{}", action, hash)
        };

        let is_match = |list: &[String]| {
            list.iter().any(|item| {
                item == hash
                    || item == action
                    || item == &full_ref_with_at
                    || item == &full_ref_with_docker
                    || (action.starts_with("docker://")
                        && item == &format!("{}@{}", action.trim_start_matches("docker://"), hash))
                    || (!action.starts_with("docker://")
                        && item == &format!("docker://{}@{}", action, hash))
            })
        };

        if is_match(&self.compromised) {
            HashSecurityStatus::Compromised
        } else if is_match(&self.vetted) {
            HashSecurityStatus::Vetted
        } else {
            HashSecurityStatus::NotChecked
        }
    }

    /// Generates a standard line-based diff between two strings.
    pub fn format_diff(&self, old: &str, new: &str, results: &[UpdateResult]) -> String {
        let mut out = String::new();
        let diff = TextDiff::from_lines(old, new);
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-".red(),
                ChangeTag::Insert => "+".green(),
                ChangeTag::Equal => " ".normal(),
            };
            let mut line_str = change.to_string();
            if change.tag() == ChangeTag::Insert {
                for res in results {
                    let sha_str = res.new_sha.to_string();
                    if line_str.contains(&sha_str) {
                        let status = self.check_hash_security(&res.action.to_string(), &sha_str);
                        let status_str =
                            format_security_status(status, self.show_security_feedback);
                        let trimmed_len = line_str.trim_end().len();
                        let newline = &line_str[trimmed_len..];
                        if !status_str.is_empty() {
                            line_str =
                                format!("{}{}{}", &line_str[..trimmed_len], status_str, newline);
                        }
                        break;
                    }
                }
            }
            out.push_str(&format!("{}{}", sign, line_str));
        }
        out
    }

    /// Generates a word-level inline diff for a single change.
    ///
    /// This is particularly useful for showing exactly which part of a URI or hash changed.
    pub fn format_inline_diff(&self, old: &str, new: &str, status: HashSecurityStatus) -> String {
        let mut out = String::new();
        let old_trimmed = old.trim();
        let new_trimmed = new.trim();
        // Use word-level diffing for high granularity.
        let diff = TextDiff::from_words(old_trimmed, new_trimmed);

        // First line: removals
        out.push_str(&format!("  {} ", "-".red()));
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Delete => out.push_str(&format!("{}", change.value().red())),
                ChangeTag::Equal => out.push_str(&format!("{}", change.value().dimmed())),
                ChangeTag::Insert => {}
            }
        }
        out.push('\n');

        // Second line: additions
        out.push_str(&format!("  {} ", "+".green()));
        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Insert => out.push_str(&format!("{}", change.value().green().bold())),
                ChangeTag::Equal => out.push_str(&format!("{}", change.value().yellow())),
                ChangeTag::Delete => {}
            }
        }
        let status_str = format_security_status(status, self.show_security_feedback);
        out.push_str(&status_str);
        out.push('\n');
        out
    }

    /// Prints a summary of all updates in the configured format.
    pub fn print_results(&self, results: &[UpdateResult]) {
        match self.format {
            OutputFormat::Json => {
                let output = JsonOutput {
                    updates: results.to_vec(),
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
                for res in results {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::OutputFormat;
    use crate::core::{DependencyRef, UpdateTask};
    use std::path::PathBuf;

    #[test]
    fn test_format_diff() {
        let formatter = Formatter::new(
            OutputFormat::Text,
            false,
            vec!["hash3".to_string()],
            vec![],
            true,
        );
        let old = "line1\nuses: actions/checkout@v2\n";
        let new = "line1\nuses: actions/checkout@hash3\n";

        let res = UpdateResult {
            task: UpdateTask::default(),
            action: "actions/checkout".into(),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v2".to_string()),
            new_sha: DependencyRef::GitSha("hash3".to_string()),
            new_tag: Some("v2".to_string()),
        };

        let diff = formatter.format_diff(old, new, &[res]);
        assert!(diff.contains("uses: actions/checkout@v2"));
        assert!(diff.contains("uses: actions/checkout@hash3"));
        assert!(diff.contains("[✓ vetted]"));
    }

    #[test]
    fn test_format_inline_diff() {
        let formatter = Formatter::new(OutputFormat::Text, false, vec![], vec![], true);
        let old = "actions/checkout@v2";
        let new = "actions/checkout@hash";
        let diff = formatter.format_inline_diff(old, new, HashSecurityStatus::NotChecked);
        assert!(diff.contains("-"));
        assert!(diff.contains("+"));
        assert!(diff.contains("v2"));
        assert!(diff.contains("hash"));
        assert!(diff.contains("[? not checked]"));
    }

    #[test]
    fn test_check_hash_security() {
        let formatter = Formatter::new(
            OutputFormat::Text,
            false,
            vec![
                "vetted_hash".to_string(),
                "actions/checkout@vetted_ref_hash".to_string(),
            ],
            vec![
                "comp_hash".to_string(),
                "actions/checkout@comp_ref_hash".to_string(),
            ],
            true,
        );

        assert_eq!(
            formatter.check_hash_security("actions/checkout", "vetted_hash"),
            HashSecurityStatus::Vetted
        );
        assert_eq!(
            formatter.check_hash_security("actions/checkout", "vetted_ref_hash"),
            HashSecurityStatus::Vetted
        );
        assert_eq!(
            formatter.check_hash_security("actions/checkout", "comp_hash"),
            HashSecurityStatus::Compromised
        );
        assert_eq!(
            formatter.check_hash_security("actions/checkout", "comp_ref_hash"),
            HashSecurityStatus::Compromised
        );
        assert_eq!(
            formatter.check_hash_security("actions/checkout", "other_hash"),
            HashSecurityStatus::NotChecked
        );
    }

    #[test]
    fn test_format_diff_disabled_feedback() {
        let formatter = Formatter::new(
            OutputFormat::Text,
            false,
            vec!["hash3".to_string()],
            vec![],
            false,
        );
        let old = "line1\nuses: actions/checkout@v2\n";
        let new = "line1\nuses: actions/checkout@hash3\n";

        let res = UpdateResult {
            task: UpdateTask::default(),
            action: "actions/checkout".into(),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v2".to_string()),
            new_sha: DependencyRef::GitSha("hash3".to_string()),
            new_tag: Some("v2".to_string()),
        };

        let diff = formatter.format_diff(old, new, &[res]);
        assert!(diff.contains("uses: actions/checkout@v2"));
        assert!(diff.contains("uses: actions/checkout@hash3"));
        assert!(!diff.contains("[✓ vetted]"));
    }
}
