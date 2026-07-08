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
        let is_github = host.contains("github.com") || std::env::var("PINNER_GITHUB_URL").is_ok();
        let is_gitlab = host.contains("gitlab.com") || std::env::var("PINNER_GITLAB_URL").is_ok();

        if is_github {
            let token = std::env::var("GITHUB_TOKEN").map_err(|_| {
                PinnerError::Api("GITHUB_TOKEN environment variable is not set".to_string())
            })?;

            let api_base = std::env::var("PINNER_GITHUB_URL")
                .unwrap_or_else(|_| "https://api.github.com".to_string());
            let pr_url = format!("{}/repos/{}/{}/pulls", api_base, owner, repo);
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
        } else if is_gitlab {
            let token = std::env::var("GITLAB_TOKEN").map_err(|_| {
                PinnerError::Api("GITLAB_TOKEN environment variable is not set".to_string())
            })?;

            let project_path = format!("{}/{}", owner, repo);
            let encoded_project_path = project_path.replace('/', "%2F");
            let api_base = std::env::var("PINNER_GITLAB_URL")
                .unwrap_or_else(|_| "https://gitlab.com/api/v4".to_string());
            let pr_url = format!(
                "{}/projects/{}/merge_requests",
                api_base, encoded_project_path
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

    #[tokio::test]
    #[serial_test::serial]
    async fn test_pr_create_no_changes() {
        let dir = tempfile::tempdir().unwrap();
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        std::process::Command::new("git")
            .arg("init")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test User"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .output()
            .unwrap();

        let f = dir.path().join("action.yml");
        std::fs::write(
            &f,
            "uses: actions/checkout@1111111111111111111111111111111111111111",
        )
        .unwrap();
        std::process::Command::new("git")
            .arg("add")
            .arg("action.yml")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .output()
            .unwrap();

        let remote = std::sync::Arc::new(crate::resolver::provider::MockRemoteProvider::new());
        let registry = std::sync::Arc::new(crate::resolver::registry::MockRegistryProvider::new());
        let osv = std::sync::Arc::new(crate::resolver::OsvClient::new(
            None,
            false,
            std::time::Duration::from_secs(0),
        ));
        let resolver = crate::resolver::Resolver::new(
            remote,
            registry,
            osv,
            crate::cli::UpgradeStrategy::Latest,
            10,
        );
        let pipeline = Pipeline::new(
            crate::scanner::Scanner::new(vec![]),
            resolver,
            crate::patcher::Patcher::new(
                crate::patcher::Formatter::new(
                    crate::cli::OutputFormat::Text,
                    false,
                    vec![],
                    vec![],
                    true,
                ),
                std::sync::Arc::new(crate::patcher::ui::TestUi { response: true }),
                false,
            ),
        );

        let res = pipeline
            .pr_create(&[f], "pinner/test-branch", "commit msg")
            .await;
        assert!(res.is_ok());

        std::env::set_current_dir(orig_dir).unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_pr_create_with_changes_github() {
        let mut server = mockito::Server::new_async().await;
        let pr_mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .with_status(201)
            .with_body(r#"{"html_url":"http://github.com/owner/repo/pull/1"}"#)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        std::process::Command::new("git")
            .arg("init")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test User"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .output()
            .unwrap();

        let f = dir.path().join("action.yml");
        std::fs::write(&f, "uses: actions/checkout@v3").unwrap();
        std::process::Command::new("git")
            .arg("add")
            .arg("action.yml")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .output()
            .unwrap();

        let upstream = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(upstream.path())
            .output()
            .unwrap();

        std::process::Command::new("git")
            .args(["remote", "add", "origin", upstream.path().to_str().unwrap()])
            .output()
            .unwrap();

        let mut remote = crate::resolver::provider::MockRemoteProvider::new();
        remote
            .expect_get_commit_sha()
            .returning(|_, _, _| Ok(crate::core::DependencyRef::GitSha("newhash".to_string())));

        let registry = std::sync::Arc::new(crate::resolver::registry::MockRegistryProvider::new());
        let osv = std::sync::Arc::new(crate::resolver::OsvClient::new(
            None,
            false,
            std::time::Duration::from_secs(0),
        ));
        let resolver = crate::resolver::Resolver::new(
            std::sync::Arc::new(remote),
            registry,
            osv,
            crate::cli::UpgradeStrategy::Latest,
            10,
        );
        let pipeline = Pipeline::new(
            crate::scanner::Scanner::new(vec![]),
            resolver,
            crate::patcher::Patcher::new(
                crate::patcher::Formatter::new(
                    crate::cli::OutputFormat::Text,
                    false,
                    vec![],
                    vec![],
                    true,
                ),
                std::sync::Arc::new(crate::patcher::ui::TestUi { response: true }),
                false,
            ),
        );

        std::env::set_var("GITHUB_TOKEN", "mock_token");
        std::env::set_var("PINNER_GITHUB_URL", server.url());

        // Set the remote URL to a fake GitHub URL to parse owner/repo
        std::process::Command::new("git")
            .args([
                "remote",
                "set-url",
                "origin",
                "git@github.com:owner/repo.git",
            ])
            .output()
            .unwrap();

        // Push should fail because it doesn't match dummy bare repo, but wait!
        // If we set the push command to push to upstream dummy repo:
        // Wait, parse_git_remote extracts owner/repo from remote URL.
        // If we have origin URL as "git@github.com:owner/repo.git", we can run pr_create!
        // But wait! If we do `git push origin <branch>`, git will try to push to `git@github.com:owner/repo.git`, which might fail if there's no internet/ssh access or credentials.
        // Wait! How do we make git push succeed?
        // In git, we can add a push URL to origin that overrides the fetch URL!
        // `git remote set-url --push origin /path/to/upstream`
        // Oh my god! This is incredibly brilliant! Git will parse the fetch URL `git@github.com:owner/repo.git` (so owner/repo = owner/repo), but it will actually push to the local `/path/to/upstream` dummy repository!
        // This is absolute wizardry!
        std::process::Command::new("git")
            .args([
                "remote",
                "set-url",
                "--push",
                "origin",
                upstream.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();

        let res = pipeline
            .pr_create(&[f], "pinner/test-branch-2", "commit msg 2")
            .await;
        assert!(res.is_ok());

        pr_mock.assert_async().await;

        std::env::set_current_dir(orig_dir).unwrap();
        std::env::remove_var("GITHUB_TOKEN");
        std::env::remove_var("PINNER_GITHUB_URL");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_pr_create_with_changes_gitlab() {
        let mut server = mockito::Server::new_async().await;
        let pr_mock = server
            .mock("POST", "/projects/owner%2Frepo/merge_requests")
            .with_status(201)
            .with_body(r#"{"web_url":"http://gitlab.com/owner/repo/-/merge_requests/1"}"#)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let orig_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        std::process::Command::new("git")
            .arg("init")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.name", "Test User"])
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .output()
            .unwrap();

        let f = dir.path().join("action.yml");
        std::fs::write(&f, "uses: actions/checkout@v3").unwrap();
        std::process::Command::new("git")
            .arg("add")
            .arg("action.yml")
            .output()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .output()
            .unwrap();

        let upstream = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(upstream.path())
            .output()
            .unwrap();

        std::process::Command::new("git")
            .args(["remote", "add", "origin", upstream.path().to_str().unwrap()])
            .output()
            .unwrap();

        let mut remote = crate::resolver::provider::MockRemoteProvider::new();
        remote
            .expect_get_commit_sha()
            .returning(|_, _, _| Ok(crate::core::DependencyRef::GitSha("newhash".to_string())));

        let registry = std::sync::Arc::new(crate::resolver::registry::MockRegistryProvider::new());
        let osv = std::sync::Arc::new(crate::resolver::OsvClient::new(
            None,
            false,
            std::time::Duration::from_secs(0),
        ));
        let resolver = crate::resolver::Resolver::new(
            std::sync::Arc::new(remote),
            registry,
            osv,
            crate::cli::UpgradeStrategy::Latest,
            10,
        );
        let pipeline = Pipeline::new(
            crate::scanner::Scanner::new(vec![]),
            resolver,
            crate::patcher::Patcher::new(
                crate::patcher::Formatter::new(
                    crate::cli::OutputFormat::Text,
                    false,
                    vec![],
                    vec![],
                    true,
                ),
                std::sync::Arc::new(crate::patcher::ui::TestUi { response: true }),
                false,
            ),
        );

        std::env::set_var("GITLAB_TOKEN", "mock_token");
        std::env::set_var("PINNER_GITLAB_URL", server.url());

        std::process::Command::new("git")
            .args([
                "remote",
                "set-url",
                "origin",
                "https://gitlab.com/owner/repo.git",
            ])
            .output()
            .unwrap();

        std::process::Command::new("git")
            .args([
                "remote",
                "set-url",
                "--push",
                "origin",
                upstream.path().to_str().unwrap(),
            ])
            .output()
            .unwrap();

        let res = pipeline
            .pr_create(&[f], "pinner/test-branch-gitlab", "commit msg gitlab")
            .await;
        assert!(res.is_ok());

        pr_mock.assert_async().await;

        std::env::set_current_dir(orig_dir).unwrap();
        std::env::remove_var("GITLAB_TOKEN");
        std::env::remove_var("PINNER_GITLAB_URL");
    }
}
