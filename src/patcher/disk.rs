use crate::core::UpdateResult;
use crate::error::PinnerError;
use crate::patcher::formatter::Formatter;
use crate::patcher::mutator::apply_update;
use colored::Colorize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub struct Patcher {
    pub formatter: Formatter,
    pub yes: bool,
    pub dry_run: bool,
    #[cfg(test)]
    pub force_confirm: Option<bool>,
}

impl Patcher {
    pub fn new(formatter: Formatter, yes: bool, dry_run: bool) -> Self {
        Self {
            formatter,
            yes,
            dry_run,
            #[cfg(test)]
            force_confirm: None,
        }
    }

    pub async fn apply_changes(
        &self,
        results: Vec<UpdateResult>,
        file_contents: HashMap<PathBuf, String>,
    ) -> Result<(), PinnerError> {
        let mut file_results: HashMap<PathBuf, Vec<UpdateResult>> = HashMap::new();
        for res in results {
            file_results
                .entry(res.task.path.clone())
                .or_default()
                .push(res);
        }

        let mut all_structured_updates = Vec::new();

        for (path, mut updates) in file_results {
            let content = file_contents.get(&path).ok_or_else(|| {
                PinnerError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Content for file {} not found", path.display()),
                ))
            })?;

            let mut new_content = content.clone();
            let mut changes = Vec::new();

            updates.sort_by_key(|a| std::cmp::Reverse(a.task.start));

            for res in updates {
                if let Some((old, new)) = apply_update(&mut new_content, &res)? {
                    changes.push((old, new));
                    all_structured_updates.push(res);
                }
            }

            if !changes.is_empty()
                && !self.formatter.quiet
                && self.formatter.format == crate::cli::OutputFormat::Text
            {
                println!("\n{} {}", "File:".bold(), path.display().to_string().cyan());
                if self.dry_run {
                    print!("{}", self.formatter.format_diff(content, &new_content));
                } else {
                    for (old, new) in &changes {
                        print!("{}", self.formatter.format_inline_diff(old, new));
                    }

                    let mut should_write = self.yes;
                    #[cfg(test)]
                    if let Some(force) = self.force_confirm {
                        should_write = force;
                    }

                    #[cfg(not(test))]
                    if !should_write {
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

                    if should_write {
                        fs::write(&path, new_content)?;
                        println!("{}", "✔ Updated successfully".green());
                    } else {
                        println!("{}", "✘ Skipped".yellow());
                    }
                }
            } else if !changes.is_empty() && !self.dry_run {
                fs::write(&path, new_content)?;
            }
        }

        self.formatter.print_results(&all_structured_updates);

        Ok(())
    }
}
