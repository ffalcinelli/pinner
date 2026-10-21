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
        .map_err(|e| PinnerError::Git(format!("Failed to execute git: {}", e)))?;
    if !output.status.success() {
        return Err(PinnerError::Git(format!(
            "git {} failed: {} {}",
            args.first().unwrap_or(&""),
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
            .args(["switch", "--", &self.original_branch])
            .output();
    }
}

/// The hosting APIs and tokens `pr-create` uses to open the Pull/Merge Request.
#[derive(Debug, Clone, Default)]
pub struct PrRemote {
    /// Base URL of the GitHub API (e.g. `https://api.github.com`).
    pub github_url: String,
    /// Base URL of the GitLab instance (e.g. `https://gitlab.com`); `/api/v4` is appended.
    pub gitlab_url: String,
    /// GitHub token, from `--github-token`, `GITHUB_TOKEN` or `.pinner.toml`.
    pub github_token: Option<String>,
    /// GitLab token, from `--gitlab-token`, `GITLAB_TOKEN` or `.pinner.toml`.
    pub gitlab_token: Option<String>,
}

/// Returns the token, treating a blank one (e.g. an unset CI secret) as missing.
fn require_token(token: &Option<String>, source: &str) -> Result<String, PinnerError> {
    token
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
        .ok_or_else(|| PinnerError::Authentication(format!("{} is not set", source)))
}

/// Returns true if `remote_host` is served by the API at `api_url`: the same host
/// (GitHub Enterprise, self-hosted GitLab) or its `api.` subdomain (`api.github.com`).
fn api_serves_host(api_url: &str, remote_host: &str) -> bool {
    reqwest::Url::parse(api_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
        .is_some_and(|api_host| {
            let remote_host = remote_host.to_lowercase();
            api_host == remote_host || api_host == format!("api.{}", remote_host)
        })
}

/// Removes `user:password@` credentials from a remote URL so it can be shown safely.
fn redact_remote_url(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority_end = rest.find('/').unwrap_or(rest.len());
            match rest[..authority_end].rfind('@') {
                Some(at) => format!("{}://{}", scheme, &rest[at + 1..]),
                None => url.to_string(),
            }
        }
        None => url.to_string(),
    }
}

/// Parses git remote origin URL strings to extract host, owner, and repository name.
/// Supports both SSH (git@...) and HTTPS protocols. Credentials in HTTPS URLs are dropped.
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
        let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
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
    ///
    /// Tracked files must be unmodified beforehand, so the commit contains only what
    /// pinner changed; untracked files are never committed.
    pub async fn pr_create(
        &self,
        paths: &[PathBuf],
        branch: &str,
        message: &str,
        remote: &PrRemote,
    ) -> Result<(), PinnerError> {
        // 1. Refuse to run over local changes, which would end up in the commit.
        let dirty = run_git(&["status", "--porcelain", "--untracked-files=no"])?;
        if !dirty.is_empty() {
            return Err(PinnerError::Git(format!(
                "pr-create needs a clean working tree, but tracked files have changes. Commit or stash them first:\n{}",
                dirty
            )));
        }

        // 2. Store the original branch to switch back later
        let original_branch = run_git(&["branch", "--show-current"])?;
        let _guard = BranchGuard { original_branch };

        // 3. Perform pinning first
        self.pin(paths).await?;

        // 4. The tree was clean, so every modified tracked file was changed by pinner.
        if run_git(&["diff", "--name-only"])?.is_empty() {
            println!("No changes detected. Pull Request is not needed.");
            return Ok(());
        }

        // 5. Get remote origin info
        let remote_url = run_git(&["remote", "get-url", "origin"])?;
        let (host, owner, repo) = parse_git_remote(&remote_url).ok_or_else(|| {
            PinnerError::Git(format!(
                "Failed to parse git remote origin URL: {}",
                redact_remote_url(&remote_url)
            ))
        })?;

        // 6. Detect base branch
        let target_branch = run_git(&["rev-parse", "--abbrev-ref", "origin/HEAD"])
            .ok()
            .and_then(|s| s.strip_prefix("origin/").map(|s| s.to_string()))
            .unwrap_or_else(|| "main".to_string());

        // 7. Checkout to the new branch
        run_git(&["switch", "-C", branch])?;

        // 8. Stage pinner's changes (tracked files only) and commit
        run_git(&["add", "--update"])?;
        run_git(&["commit", "-m", message])?;

        // 9. Push branch, without overwriting commits pinner has not seen
        run_git(&["push", "origin", "--force-with-lease", "--", branch])?;

        // 10. API Request to open PR
        let client = reqwest::Client::new();
        let is_github = host.contains("github.com") || api_serves_host(&remote.github_url, &host);
        let is_gitlab = host.contains("gitlab.com") || api_serves_host(&remote.gitlab_url, &host);

        if is_github {
            let token = require_token(
                &remote.github_token,
                "A GitHub token (--github-token, GITHUB_TOKEN or github_token in .pinner.toml)",
            )?;

            let pr_url = format!(
                "{}/repos/{}/{}/pulls",
                remote.github_url.trim_end_matches('/'),
                owner,
                repo
            );
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
            let token = require_token(
                &remote.gitlab_token,
                "A GitLab token (--gitlab-token, GITLAB_TOKEN or gitlab_token in .pinner.toml)",
            )?;

            let project_path = format!("{}/{}", owner, repo);
            let encoded_project_path = project_path.replace('/', "%2F");
            let pr_url = format!(
                "{}/api/v4/projects/{}/merge_requests",
                remote.gitlab_url.trim_end_matches('/'),
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
            return Err(PinnerError::Unsupported(format!(
                "Opening a Pull/Merge Request is not supported for remote host {}; pass --github-url or --gitlab-url if it is a GitHub Enterprise or self-hosted GitLab instance",
                host
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github_remote(url: &str) -> PrRemote {
        PrRemote {
            github_url: url.to_string(),
            github_token: Some("mock_token".to_string()),
            ..Default::default()
        }
    }

    fn git(args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Creates a repository in the current directory with `action.yml` committed.
    fn init_repo(content: &str) {
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test User"]);
        git(&["config", "user.email", "test@example.com"]);
        std::fs::write("action.yml", content).unwrap();
        git(&["add", "action.yml"]);
        git(&["commit", "-m", "initial commit"]);
    }

    fn test_pipeline(remote: crate::resolver::provider::MockRemoteProvider) -> Pipeline {
        let resolver = crate::resolver::Resolver::new(
            std::sync::Arc::new(remote),
            std::sync::Arc::new(crate::resolver::registry::MockRegistryProvider::new()),
            std::sync::Arc::new(crate::resolver::OsvClient::new(
                None,
                false,
                std::time::Duration::from_secs(0),
            )),
            crate::cli::UpgradeStrategy::Latest,
            10,
        );
        Pipeline::new(
            crate::scanner::Scanner::new(vec![]),
            resolver,
            crate::patcher::Patcher::new(
                crate::patcher::Formatter::new(
                    crate::cli::OutputFormat::Text,
                    true,
                    vec![],
                    vec![],
                    true,
                ),
                std::sync::Arc::new(crate::patcher::ui::TestUi { response: true }),
                false,
            ),
        )
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_pr_create_refuses_dirty_tree() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::TestCwdGuard::new(dir.path());
        init_repo("uses: actions/checkout@v3\n");
        std::fs::write("action.yml", "uses: actions/checkout@v3\n# local edit\n").unwrap();

        // The resolver must never be reached: the mock has no expectations.
        let pipeline = test_pipeline(crate::resolver::provider::MockRemoteProvider::new());
        let res = pipeline
            .pr_create(
                &[PathBuf::from("action.yml")],
                "pinner/dirty",
                "msg",
                &github_remote("http://127.0.0.1:1"),
            )
            .await;

        let err = res.unwrap_err();
        assert!(matches!(err, PinnerError::Git(_)));
        assert!(err.to_string().contains("clean working tree"));
        assert_eq!(git(&["branch", "--show-current"]).trim(), "main");
        assert_eq!(
            std::fs::read_to_string("action.yml").unwrap(),
            "uses: actions/checkout@v3\n# local edit\n"
        );
        assert!(git(&["branch", "--list", "pinner/dirty"]).is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_pr_create_commits_only_pinned_files() {
        let mut server = mockito::Server::new_async().await;
        let pr_mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .with_status(201)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::TestCwdGuard::new(dir.path());
        init_repo("uses: actions/checkout@v3\n");
        std::fs::write(".env", "SECRET=1\n").unwrap();

        let upstream = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(upstream.path())
            .output()
            .unwrap();
        git(&["remote", "add", "origin", "git@github.com:owner/repo.git"]);
        git(&[
            "remote",
            "set-url",
            "--push",
            "origin",
            upstream.path().to_str().unwrap(),
        ]);

        let mut remote = crate::resolver::provider::MockRemoteProvider::new();
        remote.expect_get_commit_sha().returning(|_, _, _| {
            Ok(crate::core::DependencyRef::GitSha(
                "1111111111111111111111111111111111111111".to_string(),
            ))
        });
        let res = test_pipeline(remote)
            .pr_create(
                &[PathBuf::from("action.yml")],
                "pinner/only-pinned",
                "msg",
                &github_remote(&server.url()),
            )
            .await;
        assert!(res.is_ok(), "pr_create failed with error: {:?}", res);
        pr_mock.assert_async().await;

        let committed = git(&["show", "--name-only", "--format=", "pinner/only-pinned"]);
        assert_eq!(committed.trim(), "action.yml");
        // The untracked file is left alone.
        assert!(std::path::Path::new(".env").exists());
    }

    #[test]
    fn test_parse_git_remote_strips_credentials() {
        let parsed = parse_git_remote("https://user:s3cret@github.com/owner/repo.git");
        assert_eq!(
            parsed,
            Some((
                "github.com".to_string(),
                "owner".to_string(),
                "repo".to_string()
            ))
        );
    }

    #[test]
    fn test_redact_remote_url() {
        assert_eq!(
            redact_remote_url("https://user:s3cret@host.example/a/b"),
            "https://host.example/a/b"
        );
        assert_eq!(
            redact_remote_url("https://host.example/a@b"),
            "https://host.example/a@b"
        );
        assert_eq!(redact_remote_url("git@host:a/b"), "git@host:a/b");
    }

    #[test]
    fn test_api_serves_host() {
        assert!(api_serves_host("https://api.github.com", "github.com"));
        assert!(api_serves_host("https://ghe.corp/api/v3", "ghe.corp"));
        assert!(api_serves_host("https://gitlab.corp", "GitLab.corp"));
        assert!(!api_serves_host("https://gitlab.com", "ghe.corp"));
        assert!(!api_serves_host("not a url", "ghe.corp"));
    }

    #[test]
    fn test_require_token() {
        assert_eq!(
            require_token(&Some(" tok ".into()), "X").unwrap(),
            "tok".to_string()
        );
        assert!(matches!(
            require_token(&Some("  ".into()), "X"),
            Err(PinnerError::Authentication(_))
        ));
        assert!(require_token(&None, "X").is_err());
    }

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
        let mut server = mockito::Server::new_async().await;
        let _pr_mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .with_status(201)
            .with_body(r#"{"html_url":"http://github.com/owner/repo/pull/1"}"#)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::TestCwdGuard::new(dir.path());

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

        let upstream = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init", "--bare"])
            .current_dir(upstream.path())
            .output()
            .unwrap();

        std::process::Command::new("git")
            .args(["remote", "add", "origin", "git@github.com:owner/repo.git"])
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
            .pr_create(
                &[f],
                "pinner/test-branch",
                "commit msg",
                &github_remote(&server.url()),
            )
            .await;
        assert!(res.is_ok(), "pr_create failed with error: {:?}", res);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_pr_create_with_changes_github() {
        let mut server = mockito::Server::new_async().await;
        let pr_mock = server
            .mock("POST", "/repos/owner/repo/pulls")
            .match_header("authorization", "Bearer mock_token")
            .with_status(201)
            .with_body(r#"{"html_url":"http://github.com/owner/repo/pull/1"}"#)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::TestCwdGuard::new(dir.path());

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
            .pr_create(
                &[f],
                "pinner/test-branch-2",
                "commit msg 2",
                &github_remote(&server.url()),
            )
            .await;
        assert!(res.is_ok(), "pr_create failed with error: {:?}", res);

        pr_mock.assert_async().await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_pr_create_with_changes_gitlab() {
        let mut server = mockito::Server::new_async().await;
        let pr_mock = server
            .mock("POST", "/api/v4/projects/owner%2Frepo/merge_requests")
            .match_header("PRIVATE-TOKEN", "mock_token")
            .with_status(201)
            .with_body(r#"{"web_url":"http://gitlab.com/owner/repo/-/merge_requests/1"}"#)
            .create_async()
            .await;

        let dir = tempfile::tempdir().unwrap();
        let _guard = crate::TestCwdGuard::new(dir.path());

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

        let remote = PrRemote {
            gitlab_url: server.url(),
            gitlab_token: Some("mock_token".to_string()),
            ..Default::default()
        };
        let res = pipeline
            .pr_create(
                &[f],
                "pinner/test-branch-gitlab",
                "commit msg gitlab",
                &remote,
            )
            .await;
        assert!(res.is_ok(), "pr_create failed with error: {:?}", res);

        pr_mock.assert_async().await;
    }
}
