//! Pinner is a high-performance utility for securing CI/CD workflows by pinning
//! mutable tags (like `@v1`) to immutable commit SHAs.
//!
//! It supports GitHub Actions, GitLab CI/CD, Bitbucket Pipelines, Forgejo,
//! and OCI container registries. It uses a precise AST-based parser to
//! ensure that YAML formatting and comments are preserved.

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
pub use resolver::{CachedProvider, RegistryProvider, RemoteProvider, Resolver};
pub use scanner::Scanner;

use colored::Colorize;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// The central orchestration point for the Pinner pipeline.
///
/// It connects the three phases of execution:
/// 1. **Scanner**: Find dependencies in the file system.
/// 2. **Resolver**: Fetch immutable hashes from remote APIs.
/// 3. **Patcher**: Apply changes to files on disk.
pub struct Pipeline {
    pub scanner: Scanner,
    pub resolver: Resolver,
    pub patcher: Patcher,
}

impl Pipeline {
    /// Creates a new `Pipeline`.
    pub fn new(scanner: Scanner, resolver: Resolver, patcher: Patcher) -> Self {
        Self {
            scanner,
            resolver,
            patcher,
        }
    }

    /// Automatically pins all symbolic action tags and image tags to hashes.
    pub async fn pin(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let (tasks, file_contents) = self.scanner.collect_tasks(paths).await?;
        let results = self.resolver.resolve_tasks(tasks, true).await?;
        self.patcher.apply_changes(results, file_contents).await
    }

    /// Upgrades dependencies to newer versions based on the configured strategy.
    pub async fn upgrade(&self, paths: &[PathBuf], interactive: bool) -> Result<(), PinnerError> {
        let (tasks, file_contents) = self.scanner.collect_tasks(paths).await?;
        let mut results = self.resolver.resolve_tasks(tasks, false).await?;

        if interactive {
            results = self.patcher.ui.prompt_upgrade(results)?;
        }

        self.patcher.apply_changes(results, file_contents).await
    }

    /// Verifies that all dependencies in the provided paths are pinned to an immutable hash.
    pub async fn verify(
        &self,
        paths: &[PathBuf],
    ) -> Result<crate::core::VerificationResult, PinnerError> {
        let (tasks, _) = self.scanner.collect_tasks(paths).await?;
        let mut unpinned = Vec::new();

        for task in tasks {
            let is_pinned = if let Some(tag) = &task.current_tag {
                (tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit()))
                    || (tag.starts_with("sha256:") && tag.len() == 71)
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

    /// Forcibly sets a specific action to a provided hash across all files.
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

    /// Exports an SBOM for all dependencies in the provided paths.
    pub async fn export_sbom(&self, paths: &[PathBuf]) -> Result<(), PinnerError> {
        let (tasks, _) = self.scanner.collect_tasks(paths).await?;

        #[derive(serde::Serialize)]
        struct Sbom {
            #[serde(rename = "bomFormat")]
            bom_format: String,
            #[serde(rename = "specVersion")]
            spec_version: String,
            components: Vec<Component>,
        }

        #[derive(serde::Serialize)]
        struct Component {
            name: String,
            version: String,
            #[serde(rename = "type")]
            component_type: String,
            purl: String,
        }

        let mut components = Vec::new();
        for task in tasks {
            let name = task.action.to_string();
            let version = task
                .current_tag
                .clone()
                .unwrap_or_else(|| "latest".to_string());
            let (component_type, purl) = if name.contains('/') && !name.contains('.') {
                (
                    "library",
                    format!("pkg:github/{}@{}", name, version.replace('@', "")),
                )
            } else {
                ("container", format!("pkg:oci/{}@{}", name, version))
            };

            components.push(Component {
                name,
                version,
                component_type: component_type.to_string(),
                purl,
            });
        }

        let sbom = Sbom {
            bom_format: "CycloneDX".to_string(),
            spec_version: "1.5".to_string(),
            components,
        };

        println!(
            "{}",
            serde_json::to_string_pretty(&sbom).map_err(|e| PinnerError::Config(e.to_string()))?
        );
        Ok(())
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
    let disk_cache = if cli.no_cache {
        None
    } else {
        dirs::cache_dir().map(|mut p| {
            p.push("pinner");
            p
        })
    };

    let resolver = Resolver::new(
        Arc::new(CachedProvider::new(remote, disk_cache)),
        Arc::new(registry),
        cli.upgrade_strategy.clone(),
        cli.concurrency.unwrap_or(10),
    );
    let ui = Arc::new(crate::patcher::ui::ConsoleUi::new(cli.yes));
    let patcher = Patcher::new(formatter, ui, cli.dry_run);

    let pipeline = Pipeline::new(scanner, resolver, patcher);

    match cli.command {
        Commands::Pin => pipeline.pin(&paths).await?,
        Commands::Upgrade { interactive } => pipeline.upgrade(&paths, interactive).await?,
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
        Commands::Init => init_project()?,
        Commands::ExportSbom => pipeline.export_sbom(&paths).await?,
        Commands::GenerateCompletion { .. } => {}
    }

    Ok(())
}

/// Initializes a new `.pinner.toml` configuration file with sensible defaults.
pub fn init_project() -> Result<(), PinnerError> {
    let mut config_lines = Vec::new();
    config_lines.push("# Pinner configuration file".to_string());
    config_lines
        .push("# For full documentation, see: https://github.com/ffalcinelli/pinner".to_string());
    config_lines.push("".to_string());

    let mut detected = Vec::new();
    if std::path::Path::new(".github/workflows").exists() {
        detected.push("GitHub Actions");
    }
    if std::path::Path::new(".gitlab-ci.yml").exists() {
        detected.push("GitLab CI");
    }
    if std::path::Path::new("bitbucket-pipelines.yml").exists()
        || std::path::Path::new("bitbucket-pipelines.yaml").exists()
    {
        detected.push("Bitbucket Pipelines");
    }
    if std::path::Path::new(".forgejo/workflows").exists() {
        detected.push("Forgejo/Gitea");
    }
    if std::path::Path::new(".circleci/config.yml").exists() {
        detected.push("CircleCI");
    }

    if !detected.is_empty() {
        println!(
            "{} Detected CI systems: {}",
            "✔".green().bold(),
            detected.join(", ").cyan()
        );
    } else {
        println!(
            "{} No CI systems detected, using defaults.",
            "⚠".yellow().bold()
        );
    }

    config_lines.push("# Automatically confirm all replacements".to_string());
    config_lines.push("yes = false".to_string());
    config_lines.push("".to_string());
    config_lines.push("# Upgrade strategy: latest, major, minor, commit".to_string());
    config_lines.push("upgrade_strategy = \"latest\"".to_string());
    config_lines.push("".to_string());
    config_lines.push("# Actions or images to ignore".to_string());
    config_lines.push("ignore = []".to_string());
    config_lines.push("".to_string());
    config_lines.push("# Number of concurrent API requests".to_string());
    config_lines.push("concurrency = 10".to_string());

    let config_path = std::path::PathBuf::from(".pinner.toml");
    if config_path.exists() {
        println!(
            "{} .pinner.toml already exists, skipping creation.",
            "ℹ".blue().bold()
        );
    } else {
        fs::write(&config_path, config_lines.join("\n"))?;
        println!("{} Created .pinner.toml", "✔".green().bold());
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
