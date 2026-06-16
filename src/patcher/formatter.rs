use crate::cli::OutputFormat;
use crate::core::UpdateResult;
use colored::Colorize;
use serde::Serialize;
use similar::{ChangeTag, TextDiff};

#[derive(Serialize)]
pub struct JsonOutput {
    pub updates: Vec<UpdateResult>,
}

pub struct Formatter {
    pub format: OutputFormat,
    pub quiet: bool,
}

impl Formatter {
    pub fn new(format: OutputFormat, quiet: bool) -> Self {
        Self { format, quiet }
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
