use crate::cli::OutputFormat;
use crate::error::PinnerError;
use crate::patcher::formatter::HashSecurityStatus;
use crate::patcher::report::{self, VerifyFinding, VerifyStatus};
use crate::patcher::Patcher;
use crate::resolver::{OsvVerdict, Resolver};
use crate::scanner::Scanner;
use colored::Colorize;
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
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
    ///
    /// Each dependency is classified (see [`VerifyStatus`]); with `check_osv`, pinned
    /// commits are looked up in OSV and pinned images are checked for a cosign
    /// signature. The findings are then rendered in the configured output format.
    pub async fn verify(
        &self,
        paths: &[PathBuf],
        check_osv: bool,
        strict: bool,
    ) -> Result<crate::core::VerificationResult, PinnerError> {
        let (tasks, _) = self.scanner.collect_tasks(paths).await?;
        let formatter = &self.patcher.formatter;

        if !formatter.quiet && formatter.format == OutputFormat::Text {
            eprintln!("{}", "Verifying workflow dependencies...".bold());
        }

        let mut findings: Vec<VerifyFinding> = tasks
            .into_iter()
            .map(|task| {
                let status = match task.current_tag.as_deref() {
                    Some(tag) if crate::core::is_immutable_ref(tag, &task.key) => {
                        match formatter.check_hash_security(&task.action.to_string(), tag) {
                            HashSecurityStatus::Vetted => VerifyStatus::Vetted,
                            HashSecurityStatus::Compromised => VerifyStatus::Compromised,
                            HashSecurityStatus::NotChecked => VerifyStatus::Pinned,
                        }
                    }
                    _ => VerifyStatus::Unpinned,
                };
                VerifyFinding {
                    task,
                    status,
                    advisories: Vec::new(),
                }
            })
            .collect();

        if check_osv {
            self.apply_remote_checks(&mut findings).await;
        }

        if strict {
            for f in findings
                .iter_mut()
                .filter(|f| f.status == VerifyStatus::Pinned)
            {
                f.status = VerifyStatus::NotVetted;
            }
        }

        if !formatter.quiet {
            match formatter.format {
                OutputFormat::Text => eprint!("{}", report::render_text(&findings, strict)),
                OutputFormat::Github => print!("{}", report::render_github(&findings, strict)),
                OutputFormat::Markdown => {
                    print!("{}", report::render_markdown(&findings, strict))
                }
                OutputFormat::Junit => print!("{}", report::render_junit(&findings, strict)),
                OutputFormat::Json => {}
            }
        }

        Ok(build_verification_result(findings, strict))
    }

    /// Queries OSV for pinned commits and checks pinned images for a cosign signature,
    /// downgrading findings accordingly. Identical references are checked once and up
    /// to `resolver.concurrency` checks run at a time. Lookup failures are reported as
    /// warnings and leave the finding unchanged.
    async fn apply_remote_checks(&self, findings: &mut [VerifyFinding]) {
        let mut targets: Vec<(String, String)> = findings
            .iter()
            .filter(|f| f.status == VerifyStatus::Pinned && f.task.key != "orbs")
            .map(|f| (f.task.action.to_string(), f.reference().to_string()))
            .collect();
        targets.sort();
        targets.dedup();

        type Outcome = Option<(VerifyStatus, Vec<String>)>;
        let outcomes: HashMap<(String, String), Outcome> = stream::iter(targets)
            .map(|(action, reference)| async move {
                let outcome = self.remote_check(&action, &reference).await;
                ((action, reference), outcome)
            })
            .buffer_unordered(self.resolver.concurrency.max(1))
            .collect()
            .await;

        for f in findings.iter_mut() {
            let key = (f.task.action.to_string(), f.reference().to_string());
            if let Some(Some((status, advisories))) = outcomes.get(&key) {
                f.status = *status;
                f.advisories = advisories.clone();
            }
        }
    }

    /// Returns the downgraded status (and OSV advisory IDs) for a pinned reference, or
    /// `None` when the check passes or cannot be completed.
    async fn remote_check(
        &self,
        action: &str,
        reference: &str,
    ) -> Option<(VerifyStatus, Vec<String>)> {
        let quiet = self.patcher.formatter.quiet;

        if !crate::core::is_git_sha(reference) {
            let image = action.strip_prefix("docker://").unwrap_or(action);
            return match self
                .resolver
                .registry
                .verify_provenance(image, reference)
                .await
            {
                Ok(true) => None,
                Ok(false) => Some((VerifyStatus::Unsigned, Vec::new())),
                Err(e) => {
                    if !quiet {
                        eprintln!(
                            "{} Could not verify OCI provenance for {}@{} due to error: {}",
                            "warning:".yellow().bold(),
                            image,
                            reference,
                            e
                        );
                    }
                    None
                }
            };
        }

        match self.resolver.assess_commit(reference).await {
            Ok(assessment) => {
                let status = match assessment.verdict {
                    OsvVerdict::Clean => return None,
                    OsvVerdict::Vulnerable => VerifyStatus::Vulnerable,
                    OsvVerdict::Compromised => VerifyStatus::Compromised,
                };
                Some((status, assessment.ids()))
            }
            Err(e) => {
                if !quiet {
                    eprintln!(
                        "{} Could not query OSV for {}@{}: {}",
                        "warning:".yellow().bold(),
                        action,
                        reference,
                        e
                    );
                }
                None
            }
        }
    }

    /// Forcibly sets a specific action to a provided hash across all files, optionally overriding the tag comment.
    pub async fn set(
        &self,
        paths: &[PathBuf],
        action: &str,
        hash: &str,
        tag_override: Option<&str>,
    ) -> Result<(), PinnerError> {
        let (tasks, file_contents) = self.scanner.collect_tasks(paths).await?;
        let mut results = Vec::new();

        for task in tasks {
            if task.action.0 == action {
                let new_tag = tag_override
                    .map(String::from)
                    .or_else(|| task.logical_tag());
                results.push(crate::core::UpdateResult {
                    action: task.action.clone(),
                    path: task.path.clone(),
                    old_tag: task.current_tag.clone(),
                    task: task.clone(),
                    new_sha: crate::core::DependencyRef::from(hash.to_string()),
                    new_tag,
                });
            }
        }

        self.patcher.apply_changes(results, file_contents).await
    }
}

/// Collects the findings into the serializable verification result.
fn build_verification_result(
    findings: Vec<VerifyFinding>,
    strict: bool,
) -> crate::core::VerificationResult {
    use crate::core::{
        CompromisedDependency, NonVettedDependency, UnpinnedDependency, UnsignedDependency,
        VulnerableDependency,
    };

    let mut result = crate::core::VerificationResult {
        strict,
        ..Default::default()
    };
    for f in findings {
        let VerifyFinding {
            task,
            status,
            advisories,
        } = f;
        match status {
            VerifyStatus::Unpinned => result.unpinned.push(UnpinnedDependency {
                path: task.path,
                action: task.action,
                tag: task.current_tag,
                line: task.line,
                column: task.column,
            }),
            VerifyStatus::Compromised => result.compromised.push(CompromisedDependency {
                path: task.path,
                action: task.action,
                hash: task.current_tag.unwrap_or_default(),
                advisories,
                line: task.line,
                column: task.column,
            }),
            VerifyStatus::Vulnerable => result.vulnerable.push(VulnerableDependency {
                path: task.path,
                action: task.action,
                hash: task.current_tag.unwrap_or_default(),
                advisories,
                line: task.line,
                column: task.column,
            }),
            VerifyStatus::Unsigned => result.unsigned.push(UnsignedDependency {
                path: task.path,
                action: task.action,
                digest: task.current_tag.unwrap_or_default(),
                line: task.line,
                column: task.column,
            }),
            VerifyStatus::NotVetted => result.non_vetted.push(NonVettedDependency {
                path: task.path,
                action: task.action,
                tag: task.current_tag,
                line: task.line,
                column: task.column,
            }),
            VerifyStatus::Pinned | VerifyStatus::Vetted => {}
        }
    }
    result
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
    async fn test_pipeline_verify_markdown_format() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3\nuses: actions/setup-node@1111111111111111111111111111111111111111").unwrap();

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
                crate::cli::OutputFormat::Markdown,
                false,
                vec!["actions/setup-node@1111111111111111111111111111111111111111".to_string()],
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

    #[tokio::test]
    async fn test_pipeline_verify_all_formats_and_statuses() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3\nuses: actions/setup-node@1111111111111111111111111111111111111111\nuses: docker://node:18").unwrap();

        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });

        // Test with OutputFormat::Github
        {
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
            let patcher_github = Patcher::new(
                Formatter::new(
                    crate::cli::OutputFormat::Github,
                    false,
                    vec![],
                    vec![],
                    true,
                ),
                ui.clone(),
                false,
            );
            let pipeline_github = Pipeline::new(scanner, resolver, patcher_github);
            let res_github = pipeline_github
                .verify(std::slice::from_ref(&f), false, true)
                .await
                .unwrap();
            assert!(!res_github.is_success());
        }

        // Test with OutputFormat::Junit
        {
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
            let patcher_junit = Patcher::new(
                Formatter::new(crate::cli::OutputFormat::Junit, false, vec![], vec![], true),
                ui,
                false,
            );
            let pipeline_junit = Pipeline::new(scanner, resolver, patcher_junit);
            let res_junit = pipeline_junit
                .verify(std::slice::from_ref(&f), false, true)
                .await
                .unwrap();
            assert!(!res_junit.is_success());
        }
    }

    struct MockPromptUi {
        called: std::sync::Arc<std::sync::Mutex<bool>>,
    }

    impl crate::patcher::ui::UserInterface for MockPromptUi {
        fn confirm_patch(&self, _path: &std::path::Path) -> bool {
            true
        }
        fn report_success(&self, _path: &std::path::Path) {}
        fn report_skipped(&self, _path: &std::path::Path) {}
        fn prompt_upgrade(
            &self,
            results: Vec<crate::core::UpdateResult>,
        ) -> Result<Vec<crate::core::UpdateResult>, crate::error::PinnerError> {
            *self.called.lock().unwrap() = true;
            Ok(results)
        }
    }

    #[tokio::test]
    async fn test_pipeline_upgrade_non_interactive() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_latest_release()
            .returning(|_, _| Ok("v4".to_string()));
        remote
            .expect_get_commit_sha()
            .returning(|_, tag, _| Ok(crate::core::DependencyRef::GitSha(format!("{}sha", tag))));

        let osv_client = Arc::new(crate::resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );

        let called = std::sync::Arc::new(std::sync::Mutex::new(false));
        let ui = Arc::new(MockPromptUi {
            called: called.clone(),
        });

        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, false, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        pipeline
            .upgrade(std::slice::from_ref(&f), false)
            .await
            .unwrap();

        assert!(
            !*called.lock().unwrap(),
            "prompt_upgrade should not be called in non-interactive mode"
        );
        let content = fs::read_to_string(&f).unwrap();
        assert!(
            content.contains("v4sha"),
            "File should be patched with the new sha"
        );
    }

    #[tokio::test]
    async fn test_pipeline_upgrade_interactive() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_latest_release()
            .returning(|_, _| Ok("v4".to_string()));
        remote
            .expect_get_commit_sha()
            .returning(|_, tag, _| Ok(crate::core::DependencyRef::GitSha(format!("{}sha", tag))));

        let osv_client = Arc::new(crate::resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );

        let called = std::sync::Arc::new(std::sync::Mutex::new(false));
        let ui = Arc::new(MockPromptUi {
            called: called.clone(),
        });

        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, false, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        pipeline
            .upgrade(std::slice::from_ref(&f), true)
            .await
            .unwrap();

        assert!(
            *called.lock().unwrap(),
            "prompt_upgrade should be called in interactive mode"
        );
        let content = fs::read_to_string(&f).unwrap();
        assert!(
            content.contains("v4sha"),
            "File should be patched with the new sha"
        );
    }

    #[tokio::test]
    async fn test_pipeline_pin() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_commit_sha()
            .returning(|_, tag, _| Ok(crate::core::DependencyRef::GitSha(format!("{}sha", tag))));

        let osv_client = Arc::new(crate::resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );

        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });

        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, false, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        pipeline.pin(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(
            content.contains("v3sha"),
            "File should be pinned with the new sha"
        );
    }

    fn verify_pipeline(
        registry: MockRegistryProvider,
        check_format: crate::cli::OutputFormat,
    ) -> Pipeline {
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(registry),
            Arc::new(crate::resolver::OsvClient::new(
                None,
                false,
                Duration::from_secs(0),
            )),
            UpgradeStrategy::Latest,
            4,
        );
        let patcher = Patcher::new(
            Formatter::new(check_format, true, vec![], vec![], true),
            Arc::new(crate::patcher::ui::TestUi { response: true }),
            false,
        );
        Pipeline::new(Scanner::new(vec![]), resolver, patcher)
    }

    #[tokio::test]
    async fn test_pipeline_verify_unsigned_image_warns_unless_strict() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        let digest = format!("sha256:{}", "1".repeat(64));
        fs::write(
            &f,
            format!(
                "jobs:\n  a:\n    container: alpine@{d}\n    services:\n      db:\n        image: alpine@{d}\n",
                d = digest
            ),
        )
        .unwrap();

        for strict in [false, true] {
            let mut registry = MockRegistryProvider::new();
            // Identical references are checked only once.
            registry
                .expect_verify_provenance()
                .times(1)
                .returning(|_, _| Ok(false));
            let pipeline = verify_pipeline(registry, crate::cli::OutputFormat::Text);

            let res = pipeline
                .verify(std::slice::from_ref(&f), true, strict)
                .await
                .unwrap();
            assert_eq!(res.unsigned.len(), 2);
            assert!(res.compromised.is_empty());
            assert!(res.non_vetted.is_empty());
            assert_eq!(res.is_success(), !strict);
        }
    }

    #[tokio::test]
    async fn test_pipeline_verify_provenance_error_keeps_finding() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, format!("image: alpine@sha256:{}", "1".repeat(64))).unwrap();

        let mut registry = MockRegistryProvider::new();
        registry
            .expect_verify_provenance()
            .returning(|_, _| Err(PinnerError::Api("boom".into())));
        let pipeline = verify_pipeline(registry, crate::cli::OutputFormat::Text);

        let res = pipeline
            .verify(std::slice::from_ref(&f), true, false)
            .await
            .unwrap();
        assert!(res.is_success());
        assert!(res.unsigned.is_empty());
    }
}
