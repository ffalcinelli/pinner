use crate::error::PinnerError;
use crate::pipeline::init::{init_project, init_project_with_selection};
use crate::pipeline::Pipeline;
use colored::Colorize;
use std::path::PathBuf;

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
                        eprintln!(
                            "Warning: Could not verify OCI provenance for {}@{} due to error: {}",
                            image_name, sha_str, e
                        );
                        continue;
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

            let mut is_compromised = false;
            let mut reasons = Vec::new();

            if let Ok(Some(body)) = self.resolver.check_vulnerabilities(&sha_str).await {
                if let Ok(osv_resp) = serde_json::from_str::<OsvResponse>(&body) {
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

            let toml_str = config.to_formatted_string()?;
            std::fs::write(".pinner.toml", toml_str)?;
            println!("\n{} Updated .pinner.toml", "✔".green().bold());
        }

        if !vulnerable_deps.is_empty() {
            println!("\n{}", "⚠ Note: Vulnerable dependencies with standard CVEs were detected. Review these carefully before manually vetting them.".yellow());
        }

        Ok(())
    }
}
