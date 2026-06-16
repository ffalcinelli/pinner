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
}

impl Formatter {
    /// Creates a new `Formatter`.
    pub fn new(format: OutputFormat, quiet: bool) -> Self {
        Self { format, quiet }
    }

    /// Generates a standard line-based diff between two strings.
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

    /// Generates a word-level inline diff for a single change.
    ///
    /// This is particularly useful for showing exactly which part of a URI or hash changed.
    pub fn format_inline_diff(&self, old: &str, new: &str) -> String {
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

    #[test]
    fn test_format_diff() {
        let formatter = Formatter::new(OutputFormat::Text, false);
        let old = "line1\nline2\n";
        let new = "line1\nline3\n";
        let diff = formatter.format_diff(old, new);
        assert!(diff.contains("line2"));
        assert!(diff.contains("line3"));
    }

    #[test]
    fn test_format_inline_diff() {
        let formatter = Formatter::new(OutputFormat::Text, false);
        let old = "actions/checkout@v2";
        let new = "actions/checkout@hash";
        let diff = formatter.format_inline_diff(old, new);
        assert!(diff.contains("-"));
        assert!(diff.contains("+"));
        assert!(diff.contains("v2"));
        assert!(diff.contains("hash"));
    }
}
