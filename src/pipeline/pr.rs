//! The PR creation module handles local Git mutations and automated Pull/Merge Request
//! creation on remote code hosting platforms (e.g., GitHub and GitLab).
//!
//! It orchestrates:
//! 1. Running the pinning pipeline.
//! 2. Checking if any files are modified.
//! 3. Creating a new git branch and committing/pushing changes.
//! 4. Authenticating and submitting PRs/MRs to GitHub/GitLab REST APIs.

use crate::error::PinnerError;
use crate::pipeline::Pipeline;
use std::path::PathBuf;

/// Helper to execute a local Git command synchronously and capture stdout.
/// Returns a `PinnerError` if execution fails or if the command returns a non-zero exit code.
fn run_git(args: &[&str]) -> Result<String, PinnerError> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| PinnerError::Api(format!("Failed to execute git: {}", e)))?;
    if !output.status.success() {
        return Err(PinnerError::Api(format!(
            "git command failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// A drop guard that automatically restores the original git branch
/// when the guard goes out of scope (e.g. on early returns or failures).
struct BranchGuard {
    original_branch: String,
}

impl Drop for BranchGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new("git")
            .args(["checkout", &self.original_branch])
            .output();
    }
}

/// Parses git remote origin URL strings to extract host, owner, and repository name.
/// Supports both SSH (git@...) and HTTPS protocols.
fn parse_git_remote(url: &str) -> Option<(String, String, String)> {
    let url = url.trim();
    if url.starts_with("git@") {
        let part = url.strip_prefix("git@")?;
        let (host, path) = part.split_once(':')?;
        let path = path.strip_suffix(".git").unwrap_or(path);
        let last_slash = path.rfind('/')?;
        let owner = &path[..last_slash];
        let repo = &path[last_slash + 1..];
        Some((host.to_string(), owner.to_string(), repo.to_string()))
    } else if url.starts_with("https://") {
        let part = url.strip_prefix("https://")?;
        let (host, path) = part.split_once('/')?;
        let path = path.strip_suffix(".git").unwrap_or(path);
        let last_slash = path.rfind('/')?;
        let owner = &path[..last_slash];
        let repo = &path[last_slash + 1..];
        Some((host.to_string(), owner.to_string(), repo.to_string()))
    } else {
        None
    }
}

impl Pipeline {
    /// Automatically commits pinning changes, pushes to a new branch, and creates a Pull Request / Merge Request.
    pub async fn pr_create(
        &self,
        paths: &[PathBuf],
        branch: &str,
        message: &str,
    ) -> Result<(), PinnerError> {
        // 1. Store the original branch to switch back later
        let original_branch = run_git(&["branch", "--show-current"])?;
        let _guard = BranchGuard { original_branch };

        // 2. Perform pinning first
        self.pin(paths).await?;

        // 3. Check if there are modified files
        let status = run_git(&["status", "--porcelain"])?;
        let mut modified_files = Vec::new();
        for line in status.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                modified_files.push(parts[parts.len() - 1].to_string());
            }
        }

        if modified_files.is_empty() {
            println!("No changes detected. Pull Request is not needed.");
            return Ok(());
        }

        // 4. Get remote origin info
        let remote_url = run_git(&["remote", "get-url", "origin"])?;
        let (host, owner, repo) = parse_git_remote(&remote_url).ok_or_else(|| {
            PinnerError::Api(format!(
                "Failed to parse git remote origin URL: {}",
                remote_url
            ))
        })?;

        // 5. Detect base branch
        let target_branch = run_git(&["rev-parse", "--abbrev-ref", "origin/HEAD"])
            .ok()
            .and_then(|s| s.strip_prefix("origin/").map(|s| s.to_string()))
            .unwrap_or_else(|| "main".to_string());

        // 6. Checkout to the new branch
        run_git(&["checkout", "-B", branch])?;

        // 7. Stage and commit
        for file in &modified_files {
            run_git(&["add", file])?;
        }
        run_git(&["commit", "-m", message])?;

        // 8. Push branch
        run_git(&["push", "origin", branch, "--force"])?;

        // 9. API Request to open PR
        let client = reqwest::Client::new();
        if host.contains("github.com") {
            let token = std::env::var("GITHUB_TOKEN").map_err(|_| {
                PinnerError::Api("GITHUB_TOKEN environment variable is not set".to_string())
            })?;

            let pr_url = format!("https://api.github.com/repos/{}/{}/pulls", owner, repo);
            let payload = serde_json::json!({
                "title": message,
                "head": branch,
                "base": target_branch,
                "body": "This Pull Request was automatically created by Pinner to pin CI/CD dependencies to secure commit hashes."
            });

            let resp = client
                .post(&pr_url)
                .header("Authorization", format!("Bearer {}", token))
                .header("Accept", "application/vnd.github+json")
                .header("User-Agent", "pinner")
                .json(&payload)
                .send()
                .await
                .map_err(|e| {
                    PinnerError::Api(format!("Failed to send GitHub PR request: {}", e))
                })?;

            if !resp.status().is_success() {
                let err_body = resp.text().await.unwrap_or_default();
                return Err(PinnerError::Api(format!(
                    "GitHub PR creation failed: {}",
                    err_body
                )));
            }

            println!("GitHub Pull Request successfully created!");
        } else if host.contains("gitlab.com") {
            let token = std::env::var("GITLAB_TOKEN").map_err(|_| {
                PinnerError::Api("GITLAB_TOKEN environment variable is not set".to_string())
            })?;

            let project_path = format!("{}/{}", owner, repo);
            let encoded_project_path = project_path.replace('/', "%2F");
            let pr_url = format!(
                "https://gitlab.com/api/v4/projects/{}/merge_requests",
                encoded_project_path
            );
            let payload = serde_json::json!({
                "title": message,
                "source_branch": branch,
                "target_branch": target_branch,
                "description": "This Merge Request was automatically created by Pinner to pin CI/CD dependencies to secure commit hashes."
            });

            let resp = client
                .post(&pr_url)
                .header("PRIVATE-TOKEN", token)
                .header("User-Agent", "pinner")
                .json(&payload)
                .send()
                .await
                .map_err(|e| {
                    PinnerError::Api(format!("Failed to send GitLab MR request: {}", e))
                })?;

            if !resp.status().is_success() {
                let err_body = resp.text().await.unwrap_or_default();
                return Err(PinnerError::Api(format!(
                    "GitLab Merge Request creation failed: {}",
                    err_body
                )));
            }

            println!("GitLab Merge Request successfully created!");
        } else {
            return Err(PinnerError::Api(format!(
                "Auto-mitigation is not supported for remote host: {}",
                host
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_remote_ssh() {
        let url = "git@github.com:ffalcinelli/pinner.git";
        let parsed = parse_git_remote(url);
        assert_eq!(
            parsed,
            Some((
                "github.com".to_string(),
                "ffalcinelli".to_string(),
                "pinner".to_string()
            ))
        );
    }

    #[test]
    fn test_parse_git_remote_https() {
        let url = "https://gitlab.com/group/subgroup/project.git";
        let parsed = parse_git_remote(url);
        assert_eq!(
            parsed,
            Some((
                "gitlab.com".to_string(),
                "group/subgroup".to_string(),
                "project".to_string()
            ))
        );
    }
}
