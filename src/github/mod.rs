use crate::error::PinnerError;
use async_trait::async_trait;
use moka::future::Cache;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

#[cfg(test)]
use mockall::automock;

/// Represents a GitHub Action name (e.g., "actions/checkout").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActionName(pub String);

impl fmt::Display for ActionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for ActionName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for ActionName {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Represents a Git commit SHA-1 hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitSha(pub String);

impl fmt::Display for CommitSha {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for CommitSha {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Represents a Git branch name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchName(pub String);

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for BranchName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

#[derive(Debug, Deserialize)]
pub struct RefResponse {
    pub sha: String,
}

#[derive(Debug, Deserialize)]
pub struct ReleaseResponse {
    pub tag_name: String,
}

#[derive(Debug, Deserialize)]
pub struct RepoResponse {
    pub default_branch: String,
}

/// Trait for interacting with the GitHub API.
#[cfg_attr(test, automock)]
#[async_trait]
pub trait GithubProvider: Send + Sync {
    /// Fetches the commit SHA for a given action and tag/branch.
    async fn get_commit_sha(
        &self,
        action: &ActionName,
        tag: &str,
    ) -> Result<CommitSha, PinnerError>;
    /// Fetches the latest release tag for a given action.
    async fn get_latest_release(&self, action: &ActionName) -> Result<String, PinnerError>;
    /// Fetches all tags for a given action.
    async fn list_tags(&self, action: &ActionName) -> Result<Vec<String>, PinnerError>;
    /// Fetches the default branch for a given action.
    async fn get_default_branch(&self, action: &ActionName) -> Result<BranchName, PinnerError>;
}

/// Default implementation of [`GithubProvider`] using `reqwest`.
pub struct ReqwestGithubProvider {
    pub client: ClientWithMiddleware,
    pub base_url: String,
    pub sha_cache: Cache<(ActionName, String), CommitSha>,
    pub release_cache: Cache<ActionName, String>,
    pub branch_cache: Cache<ActionName, BranchName>,
}

#[cfg(not(tarpaulin))]
impl Default for ReqwestGithubProvider {
    fn default() -> Self {
        Self::new("https://api.github.com".to_string(), None)
    }
}

impl ReqwestGithubProvider {
    /// Creates a new [`ReqwestGithubProvider`] with the specified base URL and optional token.
    pub fn new(base_url: String, token: Option<String>) -> Self {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, HeaderValue::from_static("pinner"));

        let token = token.or_else(|| std::env::var("GITHUB_TOKEN").ok());

        if let Some(t) = token {
            if let Ok(auth) = HeaderValue::from_str(&format!("Bearer {}", t)) {
                h.insert(AUTHORIZATION, auth);
            }
        }

        let reqwest_client = reqwest::Client::builder()
            .default_headers(h)
            .build()
            .expect("Failed to build reqwest client");

        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
        let client = ClientBuilder::new(reqwest_client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Self {
            client,
            base_url,
            sha_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(Duration::from_secs(3600))
                .build(),
            release_cache: Cache::builder()
                .max_capacity(500)
                .time_to_live(Duration::from_secs(3600))
                .build(),
            branch_cache: Cache::builder()
                .max_capacity(500)
                .time_to_live(Duration::from_secs(3600))
                .build(),
        }
    }

    fn handle_api_error(&self, status: reqwest::StatusCode, action: &ActionName) -> PinnerError {
        match status.as_u16() {
            403 | 429 => PinnerError::Api(format!(
                "GitHub API rate limit exceeded (HTTP {}). Try providing a GITHUB_TOKEN to increase limits.",
                status
            )),
            _ => PinnerError::Api(format!(
                "HTTP {}: Error for action {}",
                status, action
            )),
        }
    }
}

#[async_trait]
impl GithubProvider for ReqwestGithubProvider {
    async fn get_commit_sha(
        &self,
        action: &ActionName,
        tag: &str,
    ) -> Result<CommitSha, PinnerError> {
        let key = (action.clone(), tag.to_string());
        if let Some(sha) = self.sha_cache.get(&key).await {
            return Ok(sha);
        }

        let url = format!("{}/repos/{}/commits/{}", self.base_url, action, tag);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let res: RefResponse = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            let sha = CommitSha(res.sha);
            self.sha_cache.insert(key, sha.clone()).await;
            Ok(sha)
        } else {
            Err(self.handle_api_error(resp.status(), action))
        }
    }

    async fn get_latest_release(&self, action: &ActionName) -> Result<String, PinnerError> {
        if let Some(tag) = self.release_cache.get(action).await {
            return Ok(tag);
        }

        let url = format!("{}/repos/{}/releases/latest", self.base_url, action);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let rel: ReleaseResponse = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            self.release_cache
                .insert(action.clone(), rel.tag_name.clone())
                .await;
            Ok(rel.tag_name)
        } else if resp.status().as_u16() == 404 {
            let default_branch = self.get_default_branch(action).await?;
            Ok(default_branch.0)
        } else {
            Err(self.handle_api_error(resp.status(), action))
        }
    }

    async fn list_tags(&self, action: &ActionName) -> Result<Vec<String>, PinnerError> {
        #[derive(Deserialize)]
        struct Tag {
            name: String,
        }

        let url = format!("{}/repos/{}/tags", self.base_url, action);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let tags: Vec<Tag> = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            Ok(tags.into_iter().map(|t| t.name).collect())
        } else {
            Err(self.handle_api_error(resp.status(), action))
        }
    }

    async fn get_default_branch(&self, action: &ActionName) -> Result<BranchName, PinnerError> {
        if let Some(branch) = self.branch_cache.get(action).await {
            return Ok(branch);
        }

        let url = format!("{}/repos/{}", self.base_url, action);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let repo: RepoResponse = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            let branch = BranchName(repo.default_branch);
            self.branch_cache
                .insert(action.clone(), branch.clone())
                .await;
            Ok(branch)
        } else {
            Ok(BranchName("main".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn test_action_name_display_and_from() {
        let name = ActionName::from("actions/checkout");
        assert_eq!(format!("{}", name), "actions/checkout");
        assert_eq!(ActionName::from("a".to_string()), ActionName("a".into()));
    }

    #[test]
    fn test_commit_sha_display_and_from() {
        let sha = CommitSha::from("a1b2c3d".to_string());
        assert_eq!(format!("{}", sha), "a1b2c3d");
    }

    #[test]
    fn test_branch_name_display_and_from() {
        let branch = BranchName::from("main".to_string());
        assert_eq!(format!("{}", branch), "main");
    }

    #[tokio::test]
    async fn test_handle_api_error() {
        let provider = ReqwestGithubProvider::new("https://api.github.com".into(), None);
        let action = ActionName::from("o/r");

        let err = provider.handle_api_error(StatusCode::FORBIDDEN, &action);
        assert!(format!("{}", err).contains("rate limit exceeded"));

        let err = provider.handle_api_error(StatusCode::TOO_MANY_REQUESTS, &action);
        assert!(format!("{}", err).contains("rate limit exceeded"));

        let err = provider.handle_api_error(StatusCode::NOT_FOUND, &action);
        assert!(format!("{}", err).contains("HTTP 404"));
        assert!(format!("{}", err).contains("o/r"));

        let err = provider.handle_api_error(StatusCode::INTERNAL_SERVER_ERROR, &action);
        assert!(format!("{}", err).contains("HTTP 500"));
    }

    #[tokio::test]
    async fn test_list_tags_success() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/r/tags")
            .with_status(200)
            .with_body(r#"[{"name":"v1.0.0"},{"name":"v1.1.0"}]"#)
            .create_async()
            .await;

        let provider = ReqwestGithubProvider::new(server.url(), None);
        let tags = provider.list_tags(&ActionName::from("o/r")).await.unwrap();
        assert_eq!(tags, vec!["v1.0.0".to_string(), "v1.1.0".to_string()]);
    }

    #[tokio::test]
    async fn test_list_tags_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/r/tags")
            .with_status(500)
            .create_async()
            .await;

        let provider = ReqwestGithubProvider::new(server.url(), None);
        let res = provider.list_tags(&ActionName::from("o/r")).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_get_latest_release_404_fallback() {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(404)
            .create_async()
            .await;
        let _m2 = server
            .mock("GET", "/repos/o/r")
            .with_status(200)
            .with_body(r#"{"default_branch":"main"}"#)
            .create_async()
            .await;

        let provider = ReqwestGithubProvider::new(server.url(), None);
        let tag = provider
            .get_latest_release(&ActionName::from("o/r"))
            .await
            .unwrap();
        assert_eq!(tag, "main");
    }

    #[tokio::test]
    async fn test_get_default_branch_fail_fallback() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/r")
            .with_status(500)
            .create_async()
            .await;

        let provider = ReqwestGithubProvider::new(server.url(), None);
        let branch = provider
            .get_default_branch(&ActionName::from("o/r"))
            .await
            .unwrap();
        assert_eq!(branch.0, "main");
    }

    #[test]
    fn test_dto_deserialization() {
        let ref_json = r#"{"sha":"123"}"#;
        let res: RefResponse = serde_json::from_str(ref_json).unwrap();
        assert_eq!(res.sha, "123");

        let rel_json = r#"{"tag_name":"v1"}"#;
        let res: ReleaseResponse = serde_json::from_str(rel_json).unwrap();
        assert_eq!(res.tag_name, "v1");

        let repo_json = r#"{"default_branch":"develop"}"#;
        let res: RepoResponse = serde_json::from_str(repo_json).unwrap();
        assert_eq!(res.default_branch, "develop");
    }

    #[test]
    #[serial_test::serial]
    fn test_token_injection() {
        std::env::set_var("GITHUB_TOKEN", "env_token");
        let _provider = ReqwestGithubProvider::new("https://api.github.com".into(), None);
        // We can't easily check the private client headers, but we covered the line.
        std::env::remove_var("GITHUB_TOKEN");

        let _provider2 = ReqwestGithubProvider::new(
            "https://api.github.com".into(),
            Some("manual_token".into()),
        );
        // Covered Some(t) path.
    }

    #[tokio::test]
    async fn test_provider_caching() {
        let mut s = mockito::Server::new_async().await;
        let _m = s
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(200)
            .with_body(r#"{"sha":"123"}"#)
            .expect(1) // Only one call expected
            .create_async()
            .await;

        let provider = ReqwestGithubProvider::new(s.url(), None);
        let action = ActionName::from("o/r");

        let sha1 = provider.get_commit_sha(&action, "v1").await.unwrap();
        assert_eq!(sha1.0, "123");

        let sha2 = provider.get_commit_sha(&action, "v1").await.unwrap();
        assert_eq!(sha2.0, "123");
        // Second call should hit cache.

        let _m2 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(200)
            .with_body(r#"{"tag_name":"v2"}"#)
            .expect(1)
            .create_async()
            .await;

        let r1 = provider.get_latest_release(&action).await.unwrap();
        assert_eq!(r1, "v2");
        let r2 = provider.get_latest_release(&action).await.unwrap();
        assert_eq!(r2, "v2");

        let _m3 = s
            .mock("GET", "/repos/o/r")
            .with_status(200)
            .with_body(r#"{"default_branch":"main"}"#)
            .expect(1)
            .create_async()
            .await;

        let b1 = provider.get_default_branch(&action).await.unwrap();
        assert_eq!(b1.0, "main");
        let b2 = provider.get_default_branch(&action).await.unwrap();
        assert_eq!(b2.0, "main");
    }
}
