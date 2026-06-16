// Public modules
pub mod cli;
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

    pub async fn verify(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let (tasks, _) = self.scanner.collect_tasks(paths).await?;
        let mut unpinned = Vec::new();

        for task in tasks {
            let is_pinned = if let Some(tag) = &task.current_tag {
                (tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit()))
                    || (tag.starts_with("sha256:") && tag.len() == 71) // sha256: + 64 hex
            } else {
                false
            };

            if !is_pinned {
                unpinned.push((
                    task.path.clone(),
                    task.action.to_string(),
                    task.current_tag.clone(),
                ));
            }
        }

        if !unpinned.is_empty() {
            if !self.patcher.formatter.quiet {
                println!(
                    "{}",
                    "Verification failed! Unpinned actions found:".red().bold()
                );
                for (path, action, tag) in &unpinned {
                    let display_tag = tag.as_deref().unwrap_or("latest");
                    println!(
                        "  {}@{} in {}",
                        action.yellow(),
                        display_tag.yellow(),
                        path.display().to_string().cyan()
                    );
                }
            }
            return Err(PinnerError::VerificationFailed(
                "Some actions are not pinned to a SHA".into(),
            ));
        }

        if !self.patcher.formatter.quiet {
            println!("{}", "✔ All actions are correctly pinned!".green().bold());
        }
        Ok(())
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
    let patcher = Patcher::new(formatter, cli.yes, cli.dry_run);

    let pipeline = Pipeline::new(scanner, resolver, patcher);

    match cli.command {
        Commands::Pin => pipeline.pin(&paths).await,
        Commands::Upgrade => pipeline.upgrade(&paths).await,
        Commands::Verify => pipeline.verify(&paths).await,
        Commands::Set { action, hash } => pipeline.set(&paths, &action, &hash).await,
        Commands::InstallHook => install_git_hook(),
        Commands::GenerateCompletion { .. } => Ok(()),
    }
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
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true),
            true,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let res = pipeline.verify(&[f.clone()]).await;
        assert!(res.is_err()); // v3 is not pinned

        fs::write(
            &f,
            "uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        )
        .unwrap();
        let res = pipeline.verify(&[f]).await;
        assert!(res.is_ok());
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
        let mut patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true),
            true,
            false,
        );
        patcher.force_confirm = Some(true);
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        pipeline
            .set(&[f.clone()], "actions/checkout", "newhash")
            .await
            .unwrap();
        let content = fs::read_to_string(f).unwrap();
        assert!(content.contains("actions/checkout@newhash"));
    }
}
