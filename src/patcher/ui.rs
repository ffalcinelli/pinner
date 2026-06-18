use colored::Colorize;
use dialoguer::Confirm;
use std::path::Path;

/// Trait for user interactions during the patching process.
///
/// This abstraction allows for different interaction styles (e.g., interactive console,
/// non-interactive CI, or automated testing).
pub trait UserInterface: Send + Sync {
    /// Asks the user for confirmation before applying a patch to a file.
    fn confirm_patch(&self, path: &Path) -> bool;
    /// Reports success after updating a file.
    fn report_success(&self, path: &Path);
    /// Reports that a file was skipped (user declined confirmation).
    fn report_skipped(&self, path: &Path);
    /// Interactively prompts the user to select which upgrades to apply.
    fn prompt_upgrade(
        &self,
        results: Vec<crate::core::UpdateResult>,
    ) -> Result<Vec<crate::core::UpdateResult>, crate::error::PinnerError>;
}

/// A [`UserInterface`] implementation for the console using `dialoguer`.
pub struct ConsoleUi {
    /// If true, automatically confirms all patches without prompting.
    pub yes: bool,
}

impl ConsoleUi {
    /// Creates a new `ConsoleUi`.
    pub fn new(yes: bool) -> Self {
        Self { yes }
    }
}

impl UserInterface for ConsoleUi {
    fn confirm_patch(&self, path: &Path) -> bool {
        if self.yes {
            return true;
        }

        Confirm::new()
            .with_prompt(format!(
                "{} {}?",
                "Apply changes to".bold(),
                path.display().to_string().cyan()
            ))
            .default(false)
            .interact()
            .unwrap_or(false)
    }

    fn report_success(&self, _path: &Path) {
        println!("{}", "✔ Updated successfully".green());
    }

    fn report_skipped(&self, _path: &Path) {
        println!("{}", "✘ Skipped".yellow());
    }

    fn prompt_upgrade(
        &self,
        mut results: Vec<crate::core::UpdateResult>,
    ) -> Result<Vec<crate::core::UpdateResult>, crate::error::PinnerError> {
        if results.is_empty() {
            println!("{}", "✔ No upgrades found.".green().bold());
            return Ok(results);
        }

        // Filter out results where new_tag is None (no upgrade available)
        // or new_tag matches old_tag
        results.retain(|r| r.new_tag.is_some() && r.new_tag != r.old_tag);

        if results.is_empty() {
            println!(
                "{}",
                "✔ All dependencies are already up to date.".green().bold()
            );
            return Ok(results);
        }

        let mut items = Vec::new();
        for r in &results {
            let old = r.old_tag.as_deref().unwrap_or("latest");
            let new = r.new_tag.as_deref().unwrap_or("unknown");
            items.push(format!(
                "{}@{} -> {} ({})",
                r.action.to_string().yellow(),
                old.magenta(),
                new.green(),
                r.path.display().to_string().cyan()
            ));
        }

        let chosen = dialoguer::MultiSelect::new()
            .with_prompt("Select dependencies to upgrade (Space to toggle, Enter to confirm)")
            .items(&items)
            .defaults(&vec![true; items.len()])
            .interact()
            .map_err(|e| crate::error::PinnerError::Config(e.to_string()))?;

        let mut final_results = Vec::new();
        for idx in chosen {
            final_results.push(results[idx].clone());
        }

        Ok(final_results)
    }
}

/// A [`UserInterface`] implementation for testing that always returns a fixed value.
#[cfg(test)]
pub struct TestUi {
    pub response: bool,
}

#[cfg(test)]
impl UserInterface for TestUi {
    fn confirm_patch(&self, _path: &Path) -> bool {
        self.response
    }

    fn report_success(&self, _path: &Path) {}
    fn report_skipped(&self, _path: &Path) {}
    fn prompt_upgrade(
        &self,
        results: Vec<crate::core::UpdateResult>,
    ) -> Result<Vec<crate::core::UpdateResult>, crate::error::PinnerError> {
        Ok(results)
    }
}
