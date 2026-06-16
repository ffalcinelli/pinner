// Public modules
pub mod cli;
pub mod config;
pub mod core;
pub mod error;
pub mod patcher;
pub mod resolver;
pub mod scanner;

pub use cli::{Cli, Commands};
pub use error::PinnerError;
pub use patcher::{Formatter, Patcher};
pub use resolver::{RegistryProvider, RemoteProvider, Resolver};
pub use scanner::Scanner;

use colored::Colorize;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Pipeline {
    pub scanner: Scanner,
    pub resolver: Resolver,
    pub patcher: Patcher,
}

impl Pipeline {
    pub fn new(scanner: Scanner, resolver: Resolver, patcher: Patcher) -> Self {
        Self {
            scanner,
            resolver,
            patcher,
        }
    }

    pub async fn pin(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let (tasks, file_contents) = self.scanner.collect_tasks(paths).await?;
        let results = self.resolver.resolve_tasks(tasks, true).await?;
        self.patcher.apply_changes(results, file_contents).await
    }

    pub async fn upgrade(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let (tasks, file_contents) = self.scanner.collect_tasks(paths).await?;
        let results = self.resolver.resolve_tasks(tasks, false).await?;
        self.patcher.apply_changes(results, file_contents).await
    }

    pub async fn verify(
        &self,
        paths: &[PathBuf],
    ) -> Result<crate::core::VerificationResult, PinnerError> {
        let (tasks, _) = self.scanner.collect_tasks(paths).await?;
        let mut unpinned = Vec::new();

        for task in tasks {
            let is_pinned = if let Some(tag) = &task.current_tag {
                (tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit()))
                    || (tag.starts_with("sha256:") && tag.len() == 71) // sha256: + 64 hex
                    || (task.key == "orbs" && !tag.is_empty())
            } else {
                false
            };

            if !is_pinned {
                unpinned.push(crate::core::UnpinnedDependency {
                    path: task.path.clone(),
                    action: task.action.clone(),
                    tag: task.current_tag.clone(),
                    line: task.line,
                    column: task.column,
                });
            }
        }

        let result = crate::core::VerificationResult { unpinned };

        if !result.is_success() {
            if !self.patcher.formatter.quiet
                && self.patcher.formatter.format == crate::cli::OutputFormat::Text
            {
                eprintln!(
                    "{}",
                    "Verification failed! Unpinned actions found:".red().bold()
                );
                for dep in &result.unpinned {
                    let display_tag = dep.tag.as_deref().unwrap_or("latest");
                    eprintln!(
                        "  {}@{} in {}:{}:{}",
                        dep.action.to_string().yellow(),
                        display_tag.yellow(),
                        dep.path.display().to_string().cyan(),
                        dep.line.to_string().magenta(),
                        dep.column.to_string().magenta(),
                    );
                }
            }
        } else if !self.patcher.formatter.quiet
            && self.patcher.formatter.format == crate::cli::OutputFormat::Text
        {
            eprintln!("{}", "✔ All actions are correctly pinned!".green().bold());
        }

        Ok(result)
    }

    pub async fn set(
        &self,
        paths: &[PathBuf],
        action: &str,
        hash: &str,
    ) -> Result<(), PinnerError> {
        let (tasks, file_contents) = self.scanner.collect_tasks(paths).await?;
        let mut results = Vec::new();

        for task in tasks {
            if task.action.0 == action {
                results.push(crate::core::UpdateResult {
                    action: task.action.clone(),
                    path: task.path.clone(),
                    old_tag: task.current_tag.clone(),
                    task: task.clone(),
                    new_sha: crate::core::DependencyRef::from(hash.to_string()),
                    new_tag: None,
                });
            }
        }

        self.patcher.apply_changes(results, file_contents).await
    }
}

pub async fn run<G: RemoteProvider + 'static, R: RegistryProvider + 'static>(
    cli: Cli,
    remote: G,
    registry: R,
    paths: Vec<PathBuf>,
) -> Result<(), PinnerError> {
    let scanner = Scanner::new(cli.ignore.clone());
    let formatter = Formatter::new(cli.output_format(), cli.quiet);
    let resolver = Resolver::new(
        Arc::new(remote),
        Arc::new(registry),
        cli.upgrade_strategy.clone(),
        cli.concurrency.unwrap_or(10),
    );
    let ui = Arc::new(crate::patcher::ui::ConsoleUi::new(cli.yes));
    let patcher = Patcher::new(formatter, ui, cli.dry_run);

    let pipeline = Pipeline::new(scanner, resolver, patcher);

    match cli.command {
        Commands::Pin => pipeline.pin(&paths).await?,
        Commands::Upgrade => pipeline.upgrade(&paths).await?,
        Commands::Verify => {
            let result = pipeline.verify(&paths).await?;
            if cli.output_format() == crate::cli::OutputFormat::Json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result)
                        .map_err(|e| PinnerError::Api(e.to_string()))?
                );
            }
            if !result.is_success() {
                return Err(PinnerError::VerificationFailed(
                    "Some actions are not pinned to a SHA".into(),
                ));
            }
        }
        Commands::Set { action, hash } => pipeline.set(&paths, &action, &hash).await?,
        Commands::InstallHook => install_git_hook()?,
        Commands::GenerateCompletion { .. } => {}
    }

    Ok(())
}

pub fn install_git_hook() -> Result<(), PinnerError> {
    let git_dir = PathBuf::from(".git");
    if !git_dir.exists() {
        return Err(PinnerError::Config(
            "Not a git repository (no .git directory found)".into(),
        ));
    }

    let hooks_dir = git_dir.join("hooks");
    if !hooks_dir.exists() {
        fs::create_dir_all(&hooks_dir)?;
    }

    let hook_path = hooks_dir.join("pre-commit");

    let hook_content = r#"#!/bin/sh
# Pinner pre-commit hook: Verify that all actions are pinned to a SHA.
pinner verify --quiet
"#;

    fs::write(&hook_path, hook_content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms)?;
    }

    println!(
        "{} Git pre-commit hook installed successfully at {}",
        "✔".green().bold(),
        hook_path.display().to_string().cyan()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::UpgradeStrategy;
    use crate::resolver::provider::MockRemoteProvider;
    use crate::resolver::registry::MockRegistryProvider;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_pipeline_verify() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let res = pipeline.verify(std::slice::from_ref(&f)).await.unwrap();
        assert!(!res.is_success()); // v3 is not pinned

        fs::write(
            &f,
            "uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        )
        .unwrap();
        let res = pipeline.verify(std::slice::from_ref(&f)).await.unwrap();
        assert!(res.is_success());
    }

    #[tokio::test]
    async fn test_pipeline_set() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        pipeline
            .set(std::slice::from_ref(&f), "actions/checkout", "newhash")
            .await
            .unwrap();
        let content = fs::read_to_string(f).unwrap();
        assert!(content.contains("actions/checkout@newhash"));
    }
}
