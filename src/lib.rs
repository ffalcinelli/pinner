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

        let client = reqwest::Client::new();

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

                if !self.patcher.formatter.quiet
                    && self.patcher.formatter.format == crate::cli::OutputFormat::Text
                {
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
                            _ => {
                                status = crate::patcher::formatter::HashSecurityStatus::Compromised;
                            }
                        }
                    } else {
                        #[derive(serde::Serialize)]
                        struct OsvQuery {
                            commit: String,
                        }

                        #[derive(serde::Deserialize)]
                        struct OsvResponse {
                            vulns: Option<Vec<serde_json::Value>>,
                        }

                        let base_url = std::env::var("PINNER_OSV_URL")
                            .unwrap_or_else(|_| "https://api.osv.dev/v1/query".to_string());

                        let response = client
                            .post(&base_url)
                            .json(&OsvQuery {
                                commit: tag.to_string(),
                            })
                            .send()
                            .await;

                        if let Ok(resp) = response {
                            if resp.status().is_success() {
                                if let Ok(osv_resp) = resp.json::<OsvResponse>().await {
                                    if let Some(vulns) = osv_resp.vulns {
                                        if !vulns.is_empty() {
                                            status = crate::patcher::formatter::HashSecurityStatus::Compromised;
                                        }
                                    }
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

                        if !self.patcher.formatter.quiet
                            && self.patcher.formatter.format == crate::cli::OutputFormat::Text
                        {
                            eprintln!(
                                "  {} {}@{} in {}:{}:{} [✗ compromised]",
                                "✗".red().bold(),
                                task.action.to_string().yellow(),
                                tag.red(),
                                task.path.display().to_string().cyan(),
                                task.line.to_string().magenta(),
                                task.column.to_string().magenta(),
                            );
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

                            if !self.patcher.formatter.quiet
                                && self.patcher.formatter.format == crate::cli::OutputFormat::Text
                            {
                                eprintln!(
                                    "  {} {}@{} in {}:{}:{} [✗ not vetted]",
                                    "✗".red().bold(),
                                    task.action.to_string().yellow(),
                                    tag.yellow(),
                                    task.path.display().to_string().cyan(),
                                    task.line.to_string().magenta(),
                                    task.column.to_string().magenta(),
                                );
                            }
                        } else {
                            if !self.patcher.formatter.quiet
                                && self.patcher.formatter.format == crate::cli::OutputFormat::Text
                            {
                                let status_str = crate::patcher::formatter::format_security_status(
                                    status,
                                    self.patcher.formatter.show_security_feedback,
                                );
                                eprintln!(
                                    "  {} {}@{} in {}:{}:{}{}",
                                    "✔".green().bold(),
                                    task.action.to_string().yellow(),
                                    tag.green(),
                                    task.path.display().to_string().cyan(),
                                    task.line.to_string().magenta(),
                                    task.column.to_string().magenta(),
                                    status_str,
                                );
                            }
                        }
                    }
                    crate::patcher::formatter::HashSecurityStatus::Vetted => {
                        if !self.patcher.formatter.quiet
                            && self.patcher.formatter.format == crate::cli::OutputFormat::Text
                        {
                            let status_str = crate::patcher::formatter::format_security_status(
                                status,
                                self.patcher.formatter.show_security_feedback,
                            );
                            eprintln!(
                                "  {} {}@{} in {}:{}:{}{}",
                                "✔".green().bold(),
                                task.action.to_string().yellow(),
                                tag.green(),
                                task.path.display().to_string().cyan(),
                                task.line.to_string().magenta(),
                                task.column.to_string().magenta(),
                                status_str,
                            );
                        }
                    }
                }
            }
        }

        let result = crate::core::VerificationResult {
            unpinned,
            compromised,
            non_vetted,
        };

        if !result.is_success() {
            if !self.patcher.formatter.quiet
                && self.patcher.formatter.format == crate::cli::OutputFormat::Text
            {
                eprintln!(
                    "\n{}",
                    "Verification failed! Unpinned, compromised, or non-vetted actions found:"
                        .red()
                        .bold()
                );
                for dep in &result.unpinned {
                    let display_tag = dep.tag.as_deref().unwrap_or("latest");
                    eprintln!(
                        "  {}@{} in {}:{}:{} (unpinned)",
                        dep.action.to_string().yellow(),
                        display_tag.yellow(),
                        dep.path.display().to_string().cyan(),
                        dep.line.to_string().magenta(),
                        dep.column.to_string().magenta(),
                    );
                }
                for dep in &result.compromised {
                    eprintln!(
                        "  {}@{} in {}:{}:{} (compromised)",
                        dep.action.to_string().yellow(),
                        dep.hash.red(),
                        dep.path.display().to_string().cyan(),
                        dep.line.to_string().magenta(),
                        dep.column.to_string().magenta(),
                    );
                }
                for dep in &result.non_vetted {
                    let display_tag = dep.tag.as_deref().unwrap_or("unknown");
                    eprintln!(
                        "  {}@{} in {}:{}:{} (not vetted)",
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
            eprintln!(
                "\n{}",
                "✔ All actions are correctly pinned and secure!"
                    .green()
                    .bold()
            );
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

    /// Scans workflows and queries OSV to identify compromised dependencies.
    pub async fn scan(&self, paths: &[PathBuf], yes: bool) -> Result<(), PinnerError> {
        if !std::path::Path::new(".pinner.toml").exists() {
            println!(
                "{} No .pinner.toml configuration found. Initializing project configuration...",
                "ℹ".blue().bold()
            );
            if yes {
                init_project_with_selection(1)?;
            } else {
                init_project()?;
            }
        }

        let (tasks, _) = self.scanner.collect_tasks(paths).await?;

        let mut results = Vec::new();
        let mut unpinned_tasks = Vec::new();

        for task in tasks {
            if let Some(ref tag) = task.current_tag {
                let is_sha = tag.len() == 40 && tag.chars().all(|c| c.is_ascii_hexdigit());
                if is_sha {
                    results.push(crate::core::UpdateResult {
                        action: task.action.clone(),
                        path: task.path.clone(),
                        old_tag: Some(tag.clone()),
                        new_sha: crate::core::DependencyRef::GitSha(tag.clone()),
                        new_tag: task.logical_tag(),
                        task,
                    });
                    continue;
                }
            }
            unpinned_tasks.push(task);
        }

        if !unpinned_tasks.is_empty() {
            let resolved = self.resolver.resolve_tasks(unpinned_tasks, true).await?;
            results.extend(resolved);
        }

        if results.is_empty() {
            println!("{}", "✔ No dependencies found to scan.".green().bold());
            return Ok(());
        }

        println!("{}", "Scanning dependencies with OSV database...".cyan());

        let client = reqwest::Client::new();

        let mut clean_deps = Vec::new();
        let mut vulnerable_deps = Vec::new();
        let mut compromised_deps = Vec::new();

        // Pass 1: Resolve the upgrade candidates and collect all targets to scan (both current and upgrade candidate)
        let mut scan_targets = Vec::new();
        for res in results {
            let upgrade_cand = self
                .resolver
                .get_upgrade_candidate(&res.task)
                .await
                .ok()
                .flatten();
            let upgrade_cand_str = match &upgrade_cand {
                Some((r, Some(t))) => format!("{} # {}", r, t),
                Some((r, None)) => r.to_string(),
                None => "None".to_string(),
            };

            // We push the current dependency
            scan_targets.push((
                res.action.clone(),
                res.new_sha.to_string(),
                res.new_tag.clone(),
                upgrade_cand_str.clone(),
                res.task.clone(),
            ));

            // If there's an upgrade candidate and it's different from the current SHA, we push it to scan too!
            if let Some((ref cand_ref, ref cand_tag)) = upgrade_cand {
                let cand_sha = cand_ref.to_string();
                if cand_sha != res.new_sha.to_string() {
                    let mut cand_task = res.task.clone();
                    cand_task.current_tag = cand_tag.clone();
                    scan_targets.push((
                        res.action.clone(),
                        cand_sha,
                        cand_tag.clone(),
                        "None".to_string(), // Upgrade candidate doesn't have its own upgrade candidate
                        cand_task,
                    ));
                }
            }
        }

        // De-duplicate scan targets by (action, sha) to avoid redundant requests
        let mut unique_targets = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for target in scan_targets {
            let key = (target.0.to_string(), target.1.clone());
            if seen.insert(key) {
                unique_targets.push(target);
            }
        }

        for (action, sha_str, new_tag, upgrade_cand_str, _task) in unique_targets {
            let action_str = action.to_string();

            // Extract tag version (if not a commit SHA)
            let is_sha = |s: &str| s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit());
            let tag_version = if let Some(ref t) = new_tag {
                if is_sha(t) {
                    None
                } else {
                    Some(t.clone())
                }
            } else {
                None
            };

            // Only query Git SHAs in OSV
            let is_git_sha = sha_str.len() == 40 && sha_str.chars().all(|c| c.is_ascii_hexdigit());
            if !is_git_sha {
                // Check provenance for OCI container images or other non-git registries
                let mut reasons = Vec::new();
                let mut is_compromised = false;

                let image_name = action_str.strip_prefix("docker://").unwrap_or(&action_str);

                match self
                    .resolver
                    .registry
                    .verify_provenance(image_name, &sha_str)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => {
                        is_compromised = true;
                        reasons.push((
                            "PROVENANCE_FAIL".to_string(),
                            "Provenance signature verification failed".to_string(),
                            true,
                        ));
                    }
                    Err(e) => {
                        is_compromised = true;
                        reasons.push((
                            "PROVENANCE_ERR".to_string(),
                            format!("Provenance verification error: {}", e),
                            true,
                        ));
                    }
                }

                if reasons.is_empty() {
                    clean_deps.push((action_str, sha_str, upgrade_cand_str, tag_version));
                } else if is_compromised {
                    compromised_deps.push((
                        action_str,
                        sha_str,
                        reasons,
                        upgrade_cand_str,
                        tag_version,
                    ));
                } else {
                    vulnerable_deps.push((
                        action_str,
                        sha_str,
                        reasons,
                        upgrade_cand_str,
                        tag_version,
                    ));
                }
                continue;
            }

            #[derive(serde::Serialize)]
            struct OsvQuery {
                commit: String,
            }

            #[derive(serde::Deserialize)]
            struct OsvResponse {
                vulns: Option<Vec<OsvVulnerability>>,
            }

            #[derive(serde::Deserialize, Clone)]
            struct OsvVulnerability {
                id: String,
                summary: Option<String>,
                details: Option<String>,
            }

            let base_url = std::env::var("PINNER_OSV_URL")
                .unwrap_or_else(|_| "https://api.osv.dev/v1/query".to_string());

            let response = client
                .post(&base_url)
                .json(&OsvQuery {
                    commit: sha_str.clone(),
                })
                .send()
                .await;

            let mut is_compromised = false;
            let mut reasons = Vec::new();

            if let Ok(resp) = response {
                if resp.status().is_success() {
                    if let Ok(osv_resp) = resp.json::<OsvResponse>().await {
                        if let Some(vulns) = osv_resp.vulns {
                            for vuln in vulns {
                                let id = vuln.id;
                                let summary = vuln.summary.clone().unwrap_or_default();
                                let details = vuln.details.clone().unwrap_or_default();

                                let text = format!("{} {}", summary, details).to_lowercase();
                                let is_comp = text.contains("malicious")
                                    || text.contains("compromised")
                                    || text.contains("backdoor")
                                    || text.contains("malware")
                                    || text.contains("hijacked")
                                    || text.contains("exfiltrat");

                                if is_comp {
                                    is_compromised = true;
                                }
                                reasons.push((id, summary, is_comp));
                            }
                        }
                    }
                }
            }

            if reasons.is_empty() {
                clean_deps.push((action_str, sha_str, upgrade_cand_str, tag_version));
            } else if is_compromised {
                compromised_deps.push((
                    action_str,
                    sha_str,
                    reasons,
                    upgrade_cand_str,
                    tag_version,
                ));
            } else {
                vulnerable_deps.push((action_str, sha_str, reasons, upgrade_cand_str, tag_version));
            }
        }

        println!("\n{}", "=== Pinner Security Scan Report ===".bold().cyan());

        if !compromised_deps.is_empty() {
            println!(
                "\n{}",
                "✗ Compromised Dependencies (Supply Chain Attacks):"
                    .red()
                    .bold()
            );
            for (action, sha, vulns, candidate, _) in &compromised_deps {
                println!(
                    "  {}@{} is COMPROMISED! (Upgrade candidate: {})",
                    action.yellow(),
                    sha.cyan(),
                    candidate.magenta()
                );
                for (id, summary, _) in vulns {
                    println!("    - {}: {}", id.red(), summary);
                }
            }
        }

        if !vulnerable_deps.is_empty() {
            println!(
                "\n{}",
                "⚠ Vulnerable Dependencies (Standard CVEs):".yellow().bold()
            );
            for (action, sha, vulns, candidate, _) in &vulnerable_deps {
                println!(
                    "  {}@{} has known vulnerabilities: (Upgrade candidate: {})",
                    action.yellow(),
                    sha.cyan(),
                    candidate.magenta()
                );
                for (id, summary, _) in vulns {
                    println!("    - {}: {}", id.magenta(), summary);
                }
            }
        }

        if !clean_deps.is_empty() {
            println!("\n{}", "✔ Clean Dependencies:".green().bold());
            for (action, sha, candidate, _) in &clean_deps {
                println!(
                    "  {}@{} (Upgrade candidate: {})",
                    action.yellow(),
                    sha.cyan(),
                    candidate.magenta()
                );
            }
        }

        let local_config = if std::path::Path::new(".pinner.toml").exists() {
            let content = std::fs::read_to_string(".pinner.toml")?;
            toml::from_str::<crate::config::Config>(&content).unwrap_or_default()
        } else {
            crate::config::Config::default()
        };
        let global_config = crate::config::Config::load_global();

        let mut combined_vetted = local_config.vetted.clone().unwrap_or_default();
        if let Some(gv) = global_config.vetted {
            for item in gv {
                if !combined_vetted
                    .iter()
                    .any(|e| e.reference == item.reference)
                {
                    combined_vetted.push(item);
                }
            }
        }
        let mut combined_compromised = local_config.compromised.clone().unwrap_or_default();
        if let Some(gc) = global_config.compromised {
            for item in gc {
                if !combined_compromised
                    .iter()
                    .any(|e| e.reference == item.reference)
                {
                    combined_compromised.push(item);
                }
            }
        }

        let mut clean_to_vet = Vec::new();
        if !clean_deps.is_empty() {
            // Filter out dependencies that are already in combined_vetted
            let new_clean_deps: Vec<_> = clean_deps
                .into_iter()
                .filter(|(action, sha, _, _)| {
                    let full_ref = format!("{}@{}", action, sha);
                    !combined_vetted
                        .iter()
                        .any(|e| e.reference == full_ref || e.reference == *sha)
                })
                .collect();

            if !new_clean_deps.is_empty() {
                if yes {
                    clean_to_vet = new_clean_deps
                        .iter()
                        .map(|(action, sha, _, tag)| (action.clone(), sha.clone(), tag.clone()))
                        .collect();
                } else {
                    let items: Vec<String> = new_clean_deps
                        .iter()
                        .map(|(action, sha, _, _)| format!("{}@{}", action, sha))
                        .collect();
                    let chosen = dialoguer::MultiSelect::new()
                        .with_prompt(
                            "Select clean dependencies to add to the vetted whitelist in .pinner.toml",
                        )
                        .items(&items)
                        .defaults(&vec![true; items.len()])
                        .interact()
                        .unwrap_or_default();
                    for idx in chosen {
                        let dep = &new_clean_deps[idx];
                        clean_to_vet.push((dep.0.clone(), dep.1.clone(), dep.3.clone()));
                    }
                }
            }
        }

        let mut compromised_to_blacklist = Vec::new();
        if !compromised_deps.is_empty() {
            // Filter out dependencies that are already in combined_compromised
            let new_compromised_deps: Vec<_> = compromised_deps
                .into_iter()
                .filter(|(action, sha, _, _, _)| {
                    let full_ref = format!("{}@{}", action, sha);
                    !combined_compromised
                        .iter()
                        .any(|e| e.reference == full_ref || e.reference == *sha)
                })
                .collect();

            if !new_compromised_deps.is_empty() {
                if yes {
                    compromised_to_blacklist = new_compromised_deps
                        .iter()
                        .map(|(action, sha, _, _, tag)| (action.clone(), sha.clone(), tag.clone()))
                        .collect();
                } else {
                    let items: Vec<String> = new_compromised_deps
                        .iter()
                        .map(|(action, sha, _, _, _)| format!("{}@{}", action, sha))
                        .collect();
                    let chosen = dialoguer::MultiSelect::new()
                        .with_prompt("Select compromised dependencies to add to the compromised blacklist in .pinner.toml")
                        .items(&items)
                        .defaults(&vec![true; items.len()])
                        .interact()
                        .unwrap_or_default();
                    for idx in chosen {
                        let dep = &new_compromised_deps[idx];
                        compromised_to_blacklist.push((
                            dep.0.clone(),
                            dep.1.clone(),
                            dep.4.clone(),
                        ));
                    }
                }
            }
        }

        if !clean_to_vet.is_empty() || !compromised_to_blacklist.is_empty() {
            let mut config = if std::path::Path::new(".pinner.toml").exists() {
                let content = std::fs::read_to_string(".pinner.toml")?;
                toml::from_str::<crate::config::Config>(&content).unwrap_or_default()
            } else {
                crate::config::Config::default()
            };

            let mut vetted_list = config.vetted.unwrap_or_default();
            let mut compromised_list = config.compromised.unwrap_or_default();

            let now_ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

            for (action, sha, tag) in clean_to_vet {
                let full_ref = format!("{}@{}", action, sha);
                if !vetted_list
                    .iter()
                    .any(|e| e.reference == full_ref || e.reference == sha)
                {
                    vetted_list.push(crate::config::SecurityEntry {
                        reference: full_ref,
                        tag,
                        timestamp: Some(now_ts.clone()),
                    });
                }
            }

            for (action, sha, tag) in compromised_to_blacklist {
                let full_ref = format!("{}@{}", action, sha);
                if !compromised_list
                    .iter()
                    .any(|e| e.reference == full_ref || e.reference == sha)
                {
                    compromised_list.push(crate::config::SecurityEntry {
                        reference: full_ref,
                        tag,
                        timestamp: Some(now_ts.clone()),
                    });
                }
            }

            config.vetted = Some(vetted_list);
            config.compromised = Some(compromised_list);

            let toml_str =
                toml::to_string_pretty(&config).map_err(|e| PinnerError::Config(e.to_string()))?;
            std::fs::write(".pinner.toml", toml_str)?;
            println!("\n{} Updated .pinner.toml", "✔".green().bold());
        }

        if !vulnerable_deps.is_empty() {
            println!("\n{}", "⚠ Note: Vulnerable dependencies with standard CVEs were detected. Review these carefully before manually vetting them.".yellow());
        }

        Ok(())
    }
}

pub async fn run<G: RemoteProvider + 'static, R: RegistryProvider + 'static>(
    cli: Cli,
    remote: G,
    registry: R,
    paths: Vec<PathBuf>,
) -> Result<(), PinnerError> {
    let config = crate::config::Config::load();
    let scanner = Scanner::new(cli.ignore.clone());
    let local_vetted: Vec<String> = config
        .vetted
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.reference)
        .collect();
    let local_compromised: Vec<String> = config
        .compromised
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.reference)
        .collect();

    let global_config = crate::config::Config::load_global();
    let global_vetted: Vec<String> = global_config
        .vetted
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.reference)
        .collect();
    let global_compromised: Vec<String> = global_config
        .compromised
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.reference)
        .collect();

    let mut vetted = local_vetted;
    for item in global_vetted {
        if !vetted.contains(&item) && !local_compromised.contains(&item) {
            vetted.push(item);
        }
    }

    let mut compromised = local_compromised;
    for item in global_compromised {
        if !compromised.contains(&item) && !vetted.contains(&item) {
            compromised.push(item);
        }
    }

    let formatter = Formatter::new(
        cli.output_format(),
        cli.quiet,
        vetted,
        compromised,
        !config.no_security_feedback.unwrap_or(false),
    );
    let disk_cache = if cli.no_cache {
        None
    } else {
        dirs::cache_dir().map(|mut p| {
            p.push("pinner");
            p
        })
    };

    let resolver = Resolver::new(
        Arc::new(CachedProvider::new(remote, disk_cache, cli.offline)),
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
            let result = pipeline.verify(&paths, cli.check_osv, cli.strict).await?;
            if cli.output_format() == crate::cli::OutputFormat::Json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result)
                        .map_err(|e| PinnerError::Api(e.to_string()))?
                );
            }
            if !result.is_success() {
                return Err(PinnerError::VerificationFailed(
                    "Some actions are not pinned to a SHA, are compromised, or are not vetted"
                        .into(),
                ));
            }
        }
        Commands::Set { action, hash } => pipeline.set(&paths, &action, &hash).await?,
        Commands::InstallHook => install_git_hook()?,
        Commands::Init => init_project()?,
        Commands::ExportSbom => pipeline.export_sbom(&paths).await?,
        Commands::Scan => pipeline.scan(&paths, cli.yes).await?,
        Commands::GenerateCompletion { .. } => {}
    }

    Ok(())
}

/// Initializes a new `.pinner.toml` configuration file with sensible defaults, using the specified selection for vetted Actions.
pub fn init_project_with_selection(selection: usize) -> Result<(), PinnerError> {
    init_project_internal(Some(selection))
}

/// Initializes a new `.pinner.toml` configuration file with sensible defaults.
pub fn init_project() -> Result<(), PinnerError> {
    init_project_internal(None)
}

fn init_project_internal(selection_opt: Option<usize>) -> Result<(), PinnerError> {
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
    config_lines.push("".to_string());

    let config_path = std::path::PathBuf::from(".pinner.toml");
    if config_path.exists() {
        println!(
            "{} .pinner.toml already exists, skipping creation.",
            "ℹ".blue().bold()
        );
    } else {
        let selection = match selection_opt {
            Some(s) => s,
            None => {
                let options = vec![
                    "None (start empty)",
                    "Default/GitHub (pre-populate with popular GitHub Actions)",
                ];
                dialoguer::Select::new()
                    .with_prompt("Select a service to populate the vetted whitelist")
                    .items(&options)
                    .default(1)
                    .interact()
                    .unwrap_or(0)
            }
        };

        let mut vetted_lines = Vec::new();
        if selection == 1 {
            vetted_lines.push("vetted = [".to_string());
            vetted_lines.push(
                "    \"actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332\", # v4.1.7"
                    .to_string(),
            );
            vetted_lines.push(
                "    \"actions/setup-node@601291da96165b6a1d4b1fb337131252d6e2735d\", # v4.0.3"
                    .to_string(),
            );
            vetted_lines.push(
                "    \"actions/setup-python@82c7e60c44059a00283f090ceb68f6854d17dcef\", # v5.1.0"
                    .to_string(),
            );
            vetted_lines.push(
                "    \"actions/setup-go@cd9a547d6d5b9454b6754024774b752817bf0a26\", # v5.0.2"
                    .to_string(),
            );
            vetted_lines.push(
                "    \"actions/cache@0c45773b623bea8c8e75f6c82b208c3cf94ea4f9\", # v4.0.2"
                    .to_string(),
            );
            vetted_lines.push("    \"actions/upload-artifact@65462800fd760344b1a7b4382951275a0abb4808\", # v4.3.3".to_string());
            vetted_lines.push("    \"actions/download-artifact@65a9edc5881444af0b9093a5e628f2fe47ea3d2e\"  # v4.1.7".to_string());
            vetted_lines.push("]".to_string());
        } else {
            vetted_lines.push("vetted = [".to_string());
            vetted_lines.push("    # \"actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332\", # Example vetted action".to_string());
            vetted_lines.push("]".to_string());
        }

        config_lines
            .push("# Vetted (trusted) dependency hashes or references (Whitelist)".to_string());
        config_lines.extend(vetted_lines);
        config_lines.push("".to_string());
        config_lines.push("# Compromised dependency hashes or references (Blacklist)".to_string());
        config_lines.push("compromised = [".to_string());
        config_lines.push("    # \"actions/checkout@badhash1234567890badhash1234567890bad\", # Example compromised action".to_string());
        config_lines.push("]".to_string());
        config_lines.push("".to_string());
        config_lines.push("# Disable visual security feedback".to_string());
        config_lines.push("no_security_feedback = false".to_string());

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
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let res = pipeline
            .verify(std::slice::from_ref(&f), false, false)
            .await
            .unwrap();
        assert!(!res.is_success()); // v3 is not pinned

        fs::write(
            &f,
            "uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        )
        .unwrap();
        let res = pipeline
            .verify(std::slice::from_ref(&f), false, false)
            .await
            .unwrap();
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
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
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

    #[tokio::test]
    async fn test_pipeline_scan() {
        let mut osv_server = mockito::Server::new_async().await;
        std::env::set_var("PINNER_OSV_URL", osv_server.url());

        // Mock clean commit
        let _m1 = osv_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"1111111111111111111111111111111111111111"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"vulns":[]}"#)
            .create_async()
            .await;

        // Mock compromised commit (supply chain attack)
        let _m2 = osv_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"2222222222222222222222222222222222222222"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"vulns":[{"id":"GHSA-1","summary":"Malicious package backdoored"}]}"#)
            .create_async()
            .await;

        // Mock standard vulnerable commit
        let _m3 = osv_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"3333333333333333333333333333333333333333"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"vulns":[{"id":"GHSA-2","summary":"Standard DoS vulnerability"}]}"#)
            .create_async()
            .await;

        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "jobs:\n  test:\n    steps:\n      - uses: clean@1111111111111111111111111111111111111111\n      - uses: comp@2222222222222222222222222222222222222222\n      - uses: vuln@3333333333333333333333333333333333333333").unwrap();

        let scanner = Scanner::new(vec![]);

        let mut remote = MockRemoteProvider::new();
        remote.expect_get_latest_release().returning(|action, _| {
            if action.0 == "clean" {
                Ok("v1.2.3".to_string())
            } else if action.0 == "comp" {
                Ok("v2.0.0".to_string())
            } else {
                Ok("v3.0.0".to_string())
            }
        });

        remote.expect_get_commit_sha().returning(|action, _tag, _| {
            if action.0 == "clean" {
                Ok(crate::core::DependencyRef::GitSha(
                    "9999999999999999999999999999999999999999".to_string(),
                ))
            } else if action.0 == "comp" {
                Ok(crate::core::DependencyRef::GitSha(
                    "8888888888888888888888888888888888888888".to_string(),
                ))
            } else {
                Ok(crate::core::DependencyRef::GitSha(
                    "7777777777777777777777777777777777777777".to_string(),
                ))
            }
        });

        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        // Run scan with yes=true
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        pipeline.scan(std::slice::from_ref(&f), true).await.unwrap();

        // Check that .pinner.toml was updated
        let toml_content = fs::read_to_string(".pinner.toml").unwrap();
        // Assert structured format is serialized properly
        assert!(toml_content.contains("ref = \"clean@1111111111111111111111111111111111111111\""));
        assert!(toml_content.contains("ref = \"comp@2222222222222222222222222222222222222222\""));
        assert!(toml_content.contains("timestamp ="));
        assert!(!toml_content.contains("vuln@3333333333333333333333333333333333333333")); // standard vulnerable NOT auto-added

        std::env::set_current_dir(original_dir).unwrap();
        std::env::remove_var("PINNER_OSV_URL");
    }

    #[tokio::test]
    async fn test_local_override_precedence() {
        let local_vetted = vec!["actions/checkout@v3".to_string()];
        let local_compromised = vec![];
        let global_vetted = vec![];
        let global_compromised = vec!["actions/checkout@v3".to_string()];

        let mut vetted = local_vetted;
        for item in global_vetted {
            if !vetted.contains(&item) && !local_compromised.contains(&item) {
                vetted.push(item);
            }
        }
        let mut compromised = local_compromised;
        for item in global_compromised {
            if !compromised.contains(&item) && !vetted.contains(&item) {
                compromised.push(item);
            }
        }

        let formatter = Formatter::new(
            crate::cli::OutputFormat::Text,
            true,
            vetted,
            compromised,
            true,
        );

        let status = formatter.check_hash_security("actions/checkout", "v3");
        assert_eq!(
            status,
            crate::patcher::formatter::HashSecurityStatus::Vetted
        );
    }

    #[tokio::test]
    async fn test_pipeline_getters() {
        let scanner = Scanner::new(vec![]);
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let _ = pipeline.scanner();
        let _ = pipeline.resolver();
        let _ = pipeline.patcher();
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

        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        pipeline
            .upgrade(std::slice::from_ref(&f), true)
            .await
            .unwrap();
    }
}
