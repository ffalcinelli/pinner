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
}
