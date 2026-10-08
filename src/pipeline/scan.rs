use crate::core::{is_git_sha, is_hash_ref, is_oci_digest, DependencyRef, UpdateResult};
use crate::error::PinnerError;
use crate::pipeline::init::{init_project, init_project_with_selection};
use crate::pipeline::Pipeline;
use colored::Colorize;
use futures::stream::{self, StreamExt};
use std::path::PathBuf;

/// How a scanned dependency reference was classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanVerdict {
    /// No advisories, or a signed image.
    Clean,
    /// OSV reports ordinary vulnerabilities.
    Vulnerable,
    /// OSV reports a malicious/compromised release.
    Compromised,
    /// Image without a cosign signature. Reported, but never written to any list.
    Unsigned,
}

/// A scanned dependency reference (the one in use, or its upgrade candidate).
struct ScanEntry {
    action: String,
    sha: String,
    /// Upgrade candidate shown in the report (`"None"` when there is none).
    candidate: String,
    /// Human-readable version, when known.
    tag: Option<String>,
    /// OSV advisories as `(id, summary)`.
    advisories: Vec<(String, String)>,
}

/// Words in an OSV advisory that indicate a supply-chain compromise rather than a
/// regular vulnerability.
const COMPROMISE_KEYWORDS: [&str; 6] = [
    "malicious",
    "compromised",
    "backdoor",
    "malware",
    "hijacked",
    "exfiltrat",
];

impl Pipeline {
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
            if let Some(tag) = task.current_tag.clone() {
                if is_git_sha(&tag) || is_oci_digest(&tag) {
                    results.push(UpdateResult {
                        action: task.action.clone(),
                        path: task.path.clone(),
                        old_tag: Some(tag.clone()),
                        new_sha: DependencyRef::from(tag),
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

        let concurrency = self.resolver.concurrency.max(1);

        // Pass 1: resolve upgrade candidates, so both the current reference and the
        // candidate get scanned.
        let with_candidates: Vec<_> = stream::iter(results)
            .map(|res| async move {
                let candidate = self
                    .resolver
                    .get_upgrade_candidate(&res.task)
                    .await
                    .ok()
                    .flatten();
                (res, candidate)
            })
            .buffered(concurrency)
            .collect()
            .await;

        let mut scan_targets = Vec::new();
        for (res, upgrade_cand) in with_candidates {
            let upgrade_cand_str = match &upgrade_cand {
                Some((r, Some(t))) => format!("{} # {}", r, t),
                Some((r, None)) => r.to_string(),
                None => "None".to_string(),
            };

            if let Some((cand_ref, cand_tag)) = upgrade_cand {
                let cand_sha = cand_ref.to_string();
                if cand_sha != res.new_sha.to_string() {
                    // An upgrade candidate has no candidate of its own.
                    scan_targets.push((
                        res.action.to_string(),
                        cand_sha,
                        cand_tag,
                        "None".to_string(),
                    ));
                }
            }

            scan_targets.push((
                res.action.to_string(),
                res.new_sha.to_string(),
                res.new_tag,
                upgrade_cand_str,
            ));
        }

        // De-duplicate scan targets by (action, sha) to avoid redundant requests
        let mut seen = std::collections::HashSet::new();
        scan_targets.retain(|(action, sha, _, _)| seen.insert((action.clone(), sha.clone())));

        // Pass 2: classify every target.
        let outcomes: Vec<_> = stream::iter(scan_targets)
            .map(|(action, sha, tag, candidate)| self.scan_target(action, sha, tag, candidate))
            .buffer_unordered(concurrency)
            .collect()
            .await;

        let mut clean_deps = Vec::new();
        let mut vulnerable_deps = Vec::new();
        let mut compromised_deps = Vec::new();
        let mut unsigned_deps = Vec::new();
        for outcome in outcomes {
            match outcome {
                Ok((ScanVerdict::Clean, entry)) => clean_deps.push(entry),
                Ok((ScanVerdict::Vulnerable, entry)) => vulnerable_deps.push(entry),
                Ok((ScanVerdict::Compromised, entry)) => compromised_deps.push(entry),
                Ok((ScanVerdict::Unsigned, entry)) => unsigned_deps.push(entry),
                Err(warning) => eprintln!("{} {}", "warning:".yellow().bold(), warning),
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
            for e in &compromised_deps {
                println!(
                    "  {}@{} is COMPROMISED! (Upgrade candidate: {})",
                    e.action.yellow(),
                    e.sha.cyan(),
                    e.candidate.magenta()
                );
                for (id, summary) in &e.advisories {
                    println!("    - {}: {}", id.red(), summary);
                }
            }
        }

        if !vulnerable_deps.is_empty() {
            println!(
                "\n{}",
                "⚠ Vulnerable Dependencies (Standard CVEs):".yellow().bold()
            );
            for e in &vulnerable_deps {
                println!(
                    "  {}@{} has known vulnerabilities: (Upgrade candidate: {})",
                    e.action.yellow(),
                    e.sha.cyan(),
                    e.candidate.magenta()
                );
                for (id, summary) in &e.advisories {
                    println!("    - {}: {}", id.magenta(), summary);
                }
            }
        }

        if !unsigned_deps.is_empty() {
            println!(
                "\n{}",
                "⚠ Unsigned Images (no cosign signature, provenance not verifiable):"
                    .yellow()
                    .bold()
            );
            for e in &unsigned_deps {
                println!(
                    "  {}@{} (Upgrade candidate: {})",
                    e.action.yellow(),
                    e.sha.cyan(),
                    e.candidate.magenta()
                );
            }
        }

        if !clean_deps.is_empty() {
            println!("\n{}", "✔ Clean Dependencies:".green().bold());
            for e in &clean_deps {
                println!(
                    "  {}@{} (Upgrade candidate: {})",
                    e.action.yellow(),
                    e.sha.cyan(),
                    e.candidate.magenta()
                );
            }
        }

        // References already listed in the merged (local + global) configuration.
        let combined_vetted = &self.patcher.formatter.vetted;
        let combined_compromised = &self.patcher.formatter.compromised;

        let mut clean_to_vet = Vec::new();
        if !clean_deps.is_empty() {
            // Filter out dependencies that are already in combined_vetted
            let new_clean_deps: Vec<_> = clean_deps
                .into_iter()
                .filter(|d| {
                    let full_ref = format!("{}@{}", d.action, d.sha);
                    !combined_vetted
                        .iter()
                        .any(|e| *e == full_ref || *e == d.sha)
                })
                .collect();

            if !new_clean_deps.is_empty() {
                if yes {
                    clean_to_vet = new_clean_deps
                        .into_iter()
                        .map(|d| (d.action, d.sha, d.tag))
                        .collect();
                } else {
                    let items: Vec<String> = new_clean_deps
                        .iter()
                        .map(|d| format!("{}@{}", d.action, d.sha))
                        .collect();
                    let chosen = dialoguer::MultiSelect::new()
                        .with_prompt(
                            "Select clean dependencies to add to the vetted whitelist in .pinner.toml",
                        )
                        .items(&items)
                        .defaults(&vec![true; items.len()])
                        .interact()
                        .unwrap_or_default();

                    clean_to_vet = new_clean_deps
                        .into_iter()
                        .enumerate()
                        .filter(|(idx, _)| chosen.contains(idx))
                        .map(|(_, d)| (d.action, d.sha, d.tag))
                        .collect();
                }
            }
        }

        let mut compromised_to_blacklist = Vec::new();
        if !compromised_deps.is_empty() {
            // Filter out dependencies that are already in combined_compromised
            let new_compromised_deps: Vec<_> = compromised_deps
                .into_iter()
                .filter(|d| {
                    let full_ref = format!("{}@{}", d.action, d.sha);
                    !combined_compromised
                        .iter()
                        .any(|e| *e == full_ref || *e == d.sha)
                })
                .collect();

            if !new_compromised_deps.is_empty() {
                if yes {
                    compromised_to_blacklist = new_compromised_deps
                        .into_iter()
                        .map(|d| (d.action, d.sha, d.tag))
                        .collect();
                } else {
                    let items: Vec<String> = new_compromised_deps
                        .iter()
                        .map(|d| format!("{}@{}", d.action, d.sha))
                        .collect();
                    let chosen = dialoguer::MultiSelect::new()
                        .with_prompt("Select compromised dependencies to add to the compromised blacklist in .pinner.toml")
                        .items(&items)
                        .defaults(&vec![true; items.len()])
                        .interact()
                        .unwrap_or_default();

                    compromised_to_blacklist = new_compromised_deps
                        .into_iter()
                        .enumerate()
                        .filter(|(idx, _)| chosen.contains(idx))
                        .map(|(_, d)| (d.action, d.sha, d.tag))
                        .collect();
                }
            }
        }

        if !clean_to_vet.is_empty() || !compromised_to_blacklist.is_empty() {
            let mut config = if std::path::Path::new(".pinner.toml").exists() {
                let content = std::fs::read_to_string(".pinner.toml")?;
                toml::from_str::<crate::config::Config>(&content).map_err(|e| {
                    crate::error::PinnerError::Config(format!(
                        "Failed to parse .pinner.toml: {}",
                        e
                    ))
                })?
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

            let toml_str = config.to_formatted_string()?;

            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(".pinner.toml")?;
            use std::io::Write;
            file.write_all(toml_str.as_bytes())?;

            println!("\n{} Updated .pinner.toml", "✔".green().bold());
        }

        if !vulnerable_deps.is_empty() {
            println!("\n{}", "⚠ Note: Vulnerable dependencies with standard CVEs were detected. Review these carefully before manually vetting them.".yellow());
        }

        Ok(())
    }

    /// Classifies one reference: commits are looked up in OSV, images are checked for
    /// a cosign signature. Lookup failures are returned as warning messages so the
    /// reference is neither vetted nor blacklisted on incomplete information.
    async fn scan_target(
        &self,
        action: String,
        sha: String,
        tag: Option<String>,
        candidate: String,
    ) -> Result<(ScanVerdict, ScanEntry), String> {
        let mut entry = ScanEntry {
            tag: tag.filter(|t| !is_hash_ref(t)),
            action,
            sha,
            candidate,
            advisories: Vec::new(),
        };

        if !is_git_sha(&entry.sha) {
            let image = entry
                .action
                .strip_prefix("docker://")
                .unwrap_or(&entry.action);
            return match self
                .resolver
                .registry
                .verify_provenance(image, &entry.sha)
                .await
            {
                Ok(true) => Ok((ScanVerdict::Clean, entry)),
                Ok(false) => Ok((ScanVerdict::Unsigned, entry)),
                Err(e) => Err(format!(
                    "Could not verify OCI provenance for {}@{} due to error: {}",
                    image, entry.sha, e
                )),
            };
        }

        #[derive(serde::Deserialize)]
        struct OsvResponse {
            vulns: Option<Vec<OsvVulnerability>>,
        }

        #[derive(serde::Deserialize)]
        struct OsvVulnerability {
            id: String,
            summary: Option<String>,
            details: Option<String>,
        }

        let body = match self.resolver.check_vulnerabilities(&entry.sha).await {
            Ok(Some(body)) => body,
            Ok(None) => return Ok((ScanVerdict::Clean, entry)),
            Err(e) => {
                return Err(format!(
                    "Could not query OSV for {}@{}: {}",
                    entry.action, entry.sha, e
                ))
            }
        };
        let vulns = serde_json::from_str::<OsvResponse>(&body)
            .map_err(|e| {
                format!(
                    "Invalid OSV response for {}@{}: {}",
                    entry.action, entry.sha, e
                )
            })?
            .vulns
            .unwrap_or_default();

        let mut verdict = ScanVerdict::Clean;
        for vuln in vulns {
            let summary = vuln.summary.unwrap_or_default();
            let text = format!("{} {}", summary, vuln.details.unwrap_or_default()).to_lowercase();
            if COMPROMISE_KEYWORDS.iter().any(|k| text.contains(k)) {
                verdict = ScanVerdict::Compromised;
            } else if verdict == ScanVerdict::Clean {
                verdict = ScanVerdict::Vulnerable;
            }
            entry.advisories.push((vuln.id, summary));
        }
        Ok((verdict, entry))
    }
}

#[cfg(test)]
mod tests {
    // use super::*;
    use crate::cli::UpgradeStrategy;
    use crate::patcher::{Formatter, Patcher};
    use crate::pipeline::Pipeline;
    use crate::resolver::provider::MockRemoteProvider;
    use crate::resolver::registry::MockRegistryProvider;
    use crate::resolver::{OsvClient, Resolver};
    use crate::scanner::Scanner;
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_pipeline_scan_no_deps() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "").unwrap();

        let scanner = Scanner::new(vec![]);
        let osv_client = Arc::new(OsvClient::new(None, false, Duration::from_secs(0)));
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });

        // Use dry_run=true as per memory instructions
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, false, vec![], vec![], true),
            ui,
            true,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let res = pipeline.scan(std::slice::from_ref(&f), true).await;

        assert!(res.is_ok());
    }
}
