use crate::error::PinnerError;
use crate::patcher::Patcher;
use crate::resolver::Resolver;
use crate::scanner::Scanner;
use colored::Colorize;
use std::path::PathBuf;

pub mod init;
pub mod pr;
pub mod sbom;
pub mod scan;

/// The central orchestration point for the Pinner pipeline.
///
/// It connects the three phases of execution:
/// 1. **Scanner**: Find dependencies in the file system.
/// 2. **Resolver**: Fetch immutable hashes from remote APIs.
/// 3. **Patcher**: Apply changes to files on disk.
pub struct Pipeline {
    scanner: Scanner,
    resolver: Resolver,
    patcher: Patcher,
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

    /// Returns a reference to the pipeline's scanner.
    pub fn scanner(&self) -> &Scanner {
        &self.scanner
    }

    /// Returns a reference to the pipeline's resolver.
    pub fn resolver(&self) -> &Resolver {
        &self.resolver
    }

    /// Returns a reference to the pipeline's patcher.
    pub fn patcher(&self) -> &Patcher {
        &self.patcher
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
        check_osv: bool,
        strict: bool,
    ) -> Result<crate::core::VerificationResult, PinnerError> {
        let (tasks, _) = self.scanner.collect_tasks(paths).await?;
        let mut unpinned = Vec::new();
        let mut compromised = Vec::new();
        let mut non_vetted = Vec::new();

        if !self.patcher.formatter.quiet
            && self.patcher.formatter.format == crate::cli::OutputFormat::Text
        {
            eprintln!("{}", "Verifying workflow dependencies...".bold());
        }

        let mut junit_cases = Vec::new();

        for task in tasks {
            let is_pinned = if let Some(tag) = &task.current_tag {
                (tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit()))
                    || (tag.starts_with("sha256:") && tag.len() == 71)
                    || (task.key == "orbs" && !tag.is_empty())
            } else {
                false
            };

            let action_name = task.action.to_string();
            let file_path = task.path.display().to_string();

            if !is_pinned {
                unpinned.push(crate::core::UnpinnedDependency {
                    path: task.path.clone(),
                    action: task.action.clone(),
                    tag: task.current_tag.clone(),
                    line: task.line,
                    column: task.column,
                });

                if !self.patcher.formatter.quiet {
                    if self.patcher.formatter.format == crate::cli::OutputFormat::Text {
                        let display_tag = task.current_tag.as_deref().unwrap_or("latest");
                        eprintln!(
                            "  {} {}@{} in {}:{}:{} [✗ unpinned]",
                            "✗".red().bold(),
                            task.action.to_string().yellow(),
                            display_tag.yellow(),
                            task.path.display().to_string().cyan(),
                            task.line.to_string().magenta(),
                            task.column.to_string().magenta(),
                        );
                    } else if self.patcher.formatter.format == crate::cli::OutputFormat::Github {
                        let display_tag = task.current_tag.as_deref().unwrap_or("latest");
                        println!(
                            "::error file={},line={},col={}::Dependency {} is not pinned to a SHA (found tag: {})",
                            file_path, task.line, task.column, action_name, display_tag
                        );
                    }
                }

                if self.patcher.formatter.format == crate::cli::OutputFormat::Junit {
                    let display_tag = task.current_tag.as_deref().unwrap_or("latest");
                    junit_cases.push(format!(
                        "    <testcase name=\"{}\" classname=\"{}\" time=\"0.0\">\n      <failure message=\"Dependency is not pinned\">Dependency {} is not pinned to a SHA (found tag: {}) in {}:{}:{}</failure>\n    </testcase>",
                        action_name, file_path, action_name, display_tag, file_path, task.line, task.column
                    ));
                }
            } else {
                let tag = task.current_tag.as_deref().unwrap_or("");
                let mut status = self
                    .patcher
                    .formatter
                    .check_hash_security(&task.action.to_string(), tag);

                if status == crate::patcher::formatter::HashSecurityStatus::NotChecked && check_osv
                {
                    let action_str = task.action.to_string();
                    let is_git_sha = tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit());

                    if !is_git_sha {
                        let image_name =
                            action_str.strip_prefix("docker://").unwrap_or(&action_str);
                        match self
                            .resolver
                            .registry
                            .verify_provenance(image_name, tag)
                            .await
                        {
                            Ok(true) => {}
                            Ok(false) => {
                                status = crate::patcher::formatter::HashSecurityStatus::Compromised;
                            }
                            Err(e) => {
                                if !self.patcher.formatter.quiet {
                                    eprintln!(
                                        "Warning: Could not verify OCI provenance for {}@{} due to error: {}",
                                        image_name, tag, e
                                    );
                                }
                            }
                        }
                    } else if let Ok(Some(body)) = self.resolver.check_vulnerabilities(tag).await {
                        #[derive(serde::Deserialize)]
                        struct OsvResponse {
                            vulns: Option<Vec<serde_json::Value>>,
                        }

                        if let Ok(osv_resp) = serde_json::from_str::<OsvResponse>(&body) {
                            if let Some(vulns) = osv_resp.vulns {
                                if !vulns.is_empty() {
                                    status =
                                        crate::patcher::formatter::HashSecurityStatus::Compromised;
                                }
                            }
                        }
                    }
                }

                match status {
                    crate::patcher::formatter::HashSecurityStatus::Compromised => {
                        compromised.push(crate::core::CompromisedDependency {
                            path: task.path.clone(),
                            action: task.action.clone(),
                            hash: tag.to_string(),
                            line: task.line,
                            column: task.column,
                        });

                        if !self.patcher.formatter.quiet {
                            if self.patcher.formatter.format == crate::cli::OutputFormat::Text {
                                eprintln!(
                                    "  {} {}@{} in {}:{}:{} [✗ compromised]",
                                    "✗".red().bold(),
                                    task.action.to_string().yellow(),
                                    tag.red(),
                                    task.path.display().to_string().cyan(),
                                    task.line.to_string().magenta(),
                                    task.column.to_string().magenta(),
                                );
                            } else if self.patcher.formatter.format
                                == crate::cli::OutputFormat::Github
                            {
                                println!(
                                    "::error file={},line={},col={}::Dependency {}@{} is COMPROMISED (Supply Chain Attack)!",
                                    file_path, task.line, task.column, action_name, tag
                                );
                            }
                        }

                        if self.patcher.formatter.format == crate::cli::OutputFormat::Junit {
                            junit_cases.push(format!(
                                "    <testcase name=\"{}\" classname=\"{}\" time=\"0.0\">\n      <failure message=\"Dependency is compromised\">Dependency {}@{} is COMPROMISED (Supply Chain Attack)! in {}:{}:{}</failure>\n    </testcase>",
                                action_name, file_path, action_name, tag, file_path, task.line, task.column
                            ));
                        }
                    }
                    crate::patcher::formatter::HashSecurityStatus::NotChecked => {
                        if strict {
                            non_vetted.push(crate::core::NonVettedDependency {
                                path: task.path.clone(),
                                action: task.action.clone(),
                                tag: task.current_tag.clone(),
                                line: task.line,
                                column: task.column,
                            });

                            if !self.patcher.formatter.quiet {
                                if self.patcher.formatter.format == crate::cli::OutputFormat::Text {
                                    eprintln!(
                                        "  {} {}@{} in {}:{}:{} [✗ not vetted]",
                                        "✗".red().bold(),
                                        task.action.to_string().yellow(),
                                        tag.yellow(),
                                        task.path.display().to_string().cyan(),
                                        task.line.to_string().magenta(),
                                        task.column.to_string().magenta(),
                                    );
                                } else if self.patcher.formatter.format
                                    == crate::cli::OutputFormat::Github
                                {
                                    println!(
                                        "::error file={},line={},col={}::Dependency {}@{} is pinned but not vetted (strict mode enabled)",
                                        file_path, task.line, task.column, action_name, tag
                                    );
                                }
                            }

                            if self.patcher.formatter.format == crate::cli::OutputFormat::Junit {
                                junit_cases.push(format!(
                                    "    <testcase name=\"{}\" classname=\"{}\" time=\"0.0\">\n      <failure message=\"Dependency is not vetted\">Dependency {}@{} is pinned but not vetted in {}:{}:{}</failure>\n    </testcase>",
                                    action_name, file_path, action_name, tag, file_path, task.line, task.column
                                ));
                            }
                        } else if self.patcher.formatter.format == crate::cli::OutputFormat::Junit {
                            junit_cases.push(format!(
                                "    <testcase name=\"{}\" classname=\"{}\" time=\"0.0\"/>",
                                action_name, file_path
                            ));
                        }
                    }
                    crate::patcher::formatter::HashSecurityStatus::Vetted => {
                        if self.patcher.formatter.format == crate::cli::OutputFormat::Junit {
                            junit_cases.push(format!(
                                "    <testcase name=\"{}\" classname=\"{}\" time=\"0.0\"/>",
                                action_name, file_path
                            ));
                        }
                    }
                }
            }
        }

        let is_success = unpinned.is_empty() && compromised.is_empty() && non_vetted.is_empty();

        if !self.patcher.formatter.quiet {
            if self.patcher.formatter.format == crate::cli::OutputFormat::Text {
                if is_success {
                    eprintln!(
                        "\n{} Verification successful! All dependencies are pinned and secure.",
                        "✔".green().bold()
                    );
                } else {
                    eprintln!(
                        "\n{} Verification failed! Some dependencies are not pinned, are compromised, or are not vetted.",
                        "✗".red().bold()
                    );
                }
            } else if self.patcher.formatter.format == crate::cli::OutputFormat::Junit {
                let mut xml = String::new();
                xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");

                let total_tests = junit_cases.len();
                let total_failures = unpinned.len() + compromised.len() + non_vetted.len();

                xml.push_str(&format!(
                    "<testsuites name=\"Pinner Verification\" tests=\"{}\" failures=\"{}\" errors=\"0\" time=\"0.0\">\n",
                    total_tests, total_failures
                ));
                xml.push_str(&format!(
                    "  <testsuite name=\"pinner.verify\" tests=\"{}\" failures=\"{}\" errors=\"0\" time=\"0.0\">\n",
                    total_tests, total_failures
                ));
                for case in junit_cases {
                    xml.push_str(&case);
                    xml.push('\n');
                }
                xml.push_str("  </testsuite>\n");
                xml.push_str("</testsuites>\n");

                print!("{}", xml);
            }
        }

        Ok(crate::core::VerificationResult {
            unpinned,
            compromised,
            non_vetted,
        })
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::UpgradeStrategy;
    use crate::patcher::Formatter;
    use crate::resolver::provider::MockRemoteProvider;
    use crate::resolver::registry::MockRegistryProvider;
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_pipeline_verify_github_format() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let osv_client = Arc::new(crate::resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });

        let patcher = Patcher::new(
            Formatter::new(
                crate::cli::OutputFormat::Github,
                false,
                vec![],
                vec![],
                true,
            ),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let res = pipeline
            .verify(std::slice::from_ref(&f), false, false)
            .await
            .unwrap();
        assert!(!res.is_success());
    }

    #[tokio::test]
    async fn test_pipeline_verify_junit_format() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let osv_client = Arc::new(crate::resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });

        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Junit, false, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let res = pipeline
            .verify(std::slice::from_ref(&f), false, false)
            .await
            .unwrap();
        assert!(!res.is_success());
    }
}
