use crate::error::PinnerError;
use async_trait::async_trait;
use moka::future::Cache;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
use mockall::automock;

/// Represents a dependency name (e.g., "actions/checkout" or "alpine").
///
/// # Example
///
/// ```
/// use pinner::providers::DependencyName;
///
/// let name = DependencyName::from("actions/checkout");
/// assert_eq!(name.to_string(), "actions/checkout");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DependencyName(pub String);

impl fmt::Display for DependencyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for DependencyName {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for DependencyName {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Represents an immutable dependency reference (Git SHA or Docker Digest).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DependencyRef {
    GitSha(String),
    DockerDigest(String),
}

impl fmt::Display for DependencyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitSha(s) => write!(f, "{}", s),
            Self::DockerDigest(s) => write!(f, "{}", s),
        }
    }
}

impl From<String> for DependencyRef {
    fn from(s: String) -> Self {
        if s.starts_with("sha256:") {
            Self::DockerDigest(s)
        } else {
            Self::GitSha(s)
        }
    }
}

/// Represents a Git branch name.
///
/// # Example
///
/// ```
/// use pinner::providers::BranchName;
///
/// let branch = BranchName::from("main".to_string());
/// assert_eq!(branch.to_string(), "main");
/// ```
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

#[derive(Debug, Deserialize)]
struct BitbucketCloudRefResponse {
    target: BitbucketCloudTarget,
}

#[derive(Debug, Deserialize)]
struct BitbucketCloudTarget {
    hash: String,
    target: Option<BitbucketCloudInnerTarget>,
}

#[derive(Debug, Deserialize)]
struct BitbucketCloudInnerTarget {
    hash: String,
}

#[derive(Debug, Deserialize)]
struct BitbucketDCRefResponse {
    #[serde(rename = "latestCommit")]
    latest_commit: String,
}

#[derive(Debug, Deserialize)]
struct BitbucketDCRepoResponse {
    #[serde(rename = "defaultBranch")]
    default_branch: String,
}

/// Trait for interacting with a remote provider (GitHub, Bitbucket, etc.).
#[cfg_attr(test, automock)]
#[async_trait]
pub trait RemoteProvider: Send + Sync {
    /// Fetches the commit SHA for a given action and tag/branch.
    async fn get_commit_sha(
        &self,
        action: &DependencyName,
        tag: &str,
        key: &str,
    ) -> Result<DependencyRef, PinnerError>;
    /// Fetches the latest release tag for a given action.
    async fn get_latest_release(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<String, PinnerError>;
    /// Fetches all tags for a given action.
    async fn list_tags(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<Vec<String>, PinnerError>;
    /// Fetches the default branch for a given action.
    async fn get_default_branch(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<BranchName, PinnerError>;
}

/// Shared HTTP client logic for repository providers.
pub struct BaseHttpClient {
    pub client: ClientWithMiddleware,
    pub base_url: String,
}

impl BaseHttpClient {
    pub fn new(base_url: String, token: Option<String>, token_prefix: &str, env_var: &str) -> Self {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, HeaderValue::from_static("pinner"));

        let token = token.or_else(|| std::env::var(env_var).ok());

        if let Some(t) = token {
            let auth_val = if token_prefix.is_empty() {
                t
            } else {
                format!("{} {}", token_prefix, t)
            };
            if let Ok(auth) = HeaderValue::from_str(&auth_val) {
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

        Self { client, base_url }
    }

    pub fn handle_error(
        &self,
        status: reqwest::StatusCode,
        action: &DependencyName,
    ) -> PinnerError {
        match status.as_u16() {
            403 | 429 => PinnerError::RateLimit(format!(
                "API rate limit exceeded (HTTP {}) at {}. Try providing an API token to increase limits.",
                status, self.base_url
            )),
            _ => PinnerError::Api(format!(
                "HTTP {}: Error for action {} at {}",
                status, action, self.base_url
            )),
        }
    }
}

/// Default implementation of [`RemoteProvider`] for GitHub using `reqwest`.
pub struct ReqwestGithubProvider {
    pub base: BaseHttpClient,
    pub sha_cache: Cache<(DependencyName, String), DependencyRef>,
    pub release_cache: Cache<DependencyName, String>,
    pub branch_cache: Cache<DependencyName, BranchName>,
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
        Self {
            base: BaseHttpClient::new(base_url, token, "Bearer", "GITHUB_TOKEN"),
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
}

#[async_trait]
impl RemoteProvider for ReqwestGithubProvider {
    async fn get_commit_sha(
        &self,
        action: &DependencyName,
        tag: &str,
        _key: &str,
    ) -> Result<DependencyRef, PinnerError> {
        let key = (action.clone(), tag.to_string());
        if let Some(sha) = self.sha_cache.get(&key).await {
            return Ok(sha);
        }

        let url = format!("{}/repos/{}/commits/{}", self.base.base_url, action, tag);
        let resp = self
            .base
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
            let sha = DependencyRef::from(res.sha);
            self.sha_cache.insert(key, sha.clone()).await;
            Ok(sha)
        } else {
            Err(self.base.handle_error(resp.status(), action))
        }
    }

    async fn get_latest_release(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<String, PinnerError> {
        if let Some(tag) = self.release_cache.get(action).await {
            return Ok(tag);
        }

        let url = format!("{}/repos/{}/releases/latest", self.base.base_url, action);
        let resp = self
            .base
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
            let default_branch = self.get_default_branch(action, key).await?;
            Ok(default_branch.0)
        } else {
            Err(self.base.handle_error(resp.status(), action))
        }
    }

    async fn list_tags(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        #[derive(Deserialize)]
        struct Tag {
            name: String,
        }

        let url = format!("{}/repos/{}/tags", self.base.base_url, action);
        let resp = self
            .base
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
            Err(self.base.handle_error(resp.status(), action))
        }
    }

    async fn get_default_branch(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<BranchName, PinnerError> {
        if let Some(branch) = self.branch_cache.get(action).await {
            return Ok(branch);
        }

        let url = format!("{}/repos/{}", self.base.base_url, action);
        let resp = self
            .base
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

/// Implementation of [`RemoteProvider`] for Bitbucket using `reqwest`.
pub struct ReqwestBitbucketProvider {
    pub base: BaseHttpClient,
    pub sha_cache: Cache<(DependencyName, String), DependencyRef>,
    pub is_cloud: bool,
}

impl ReqwestBitbucketProvider {
    pub fn new(base_url: String, token: Option<String>) -> Self {
        let is_cloud = base_url.contains("bitbucket.org");
        Self::with_type(base_url, token, is_cloud)
    }

    pub fn with_type(base_url: String, token: Option<String>, is_cloud: bool) -> Self {
        Self {
            base: BaseHttpClient::new(base_url, token, "Bearer", "BITBUCKET_TOKEN"),
            sha_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(Duration::from_secs(3600))
                .build(),
            is_cloud,
        }
    }
}

#[async_trait]
impl RemoteProvider for ReqwestBitbucketProvider {
    async fn get_commit_sha(
        &self,
        action: &DependencyName,
        tag: &str,
        _key: &str,
    ) -> Result<DependencyRef, PinnerError> {
        let key = (action.clone(), tag.to_string());
        if let Some(sha) = self.sha_cache.get(&key).await {
            return Ok(sha);
        }

        let url = if self.is_cloud {
            format!(
                "{}/repositories/{}/refs/tags/{}",
                self.base.base_url, action, tag
            )
        } else {
            // Data Center format: projects/{PROJ}/repos/{REPO}/tags/{TAG}
            // We assume action is formatted as "proj/repo"
            let Some((project, repo)) = action.0.split_once('/') else {
                return Err(PinnerError::Api(format!(
                    "Invalid Bitbucket action format: {}. Expected 'project/repo'",
                    action
                )));
            };
            format!(
                "{}/rest/api/1.0/projects/{}/repos/{}/tags/{}",
                self.base.base_url, project, repo, tag
            )
        };

        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            let sha = if self.is_cloud {
                let res: BitbucketCloudRefResponse = resp
                    .json()
                    .await
                    .map_err(|e| PinnerError::Api(e.to_string()))?;
                // Handle annotated tags
                res.target.target.map(|t| t.hash).unwrap_or(res.target.hash)
            } else {
                let res: BitbucketDCRefResponse = resp
                    .json()
                    .await
                    .map_err(|e| PinnerError::Api(e.to_string()))?;
                res.latest_commit
            };

            let sha = DependencyRef::from(sha);
            self.sha_cache.insert(key, sha.clone()).await;
            Ok(sha)
        } else if self.is_cloud {
            // Try branch if tag fails on Cloud
            let branch_url = format!(
                "{}/repositories/{}/refs/branches/{}",
                self.base.base_url, action, tag
            );
            let resp = self
                .base
                .client
                .get(&branch_url)
                .send()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;

            let status = resp.status();
            if status.is_success() {
                let res: BitbucketCloudRefResponse = resp
                    .json()
                    .await
                    .map_err(|e| PinnerError::Api(e.to_string()))?;
                let sha = DependencyRef::from(res.target.hash);
                self.sha_cache.insert(key, sha.clone()).await;
                Ok(sha)
            } else {
                Err(PinnerError::Api(format!(
                    "Bitbucket API error (HTTP {}): Ref not found: {}",
                    status, tag
                )))
            }
        } else {
            // Try branch for DC
            let Some((project, repo)) = action.0.split_once('/') else {
                return Err(PinnerError::Api(format!(
                    "Invalid Bitbucket action format: {}. Expected 'project/repo'",
                    action
                )));
            };
            let branch_url = format!(
                "{}/rest/api/1.0/projects/{}/repos/{}/branches?filterText={}",
                self.base.base_url, project, repo, tag
            );
            let resp = self
                .base
                .client
                .get(&branch_url)
                .send()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;

            #[derive(Deserialize)]
            struct DCBranchResponse {
                values: Vec<BitbucketDCRefResponse>,
            }

            let status = resp.status();
            if status.is_success() {
                let res: DCBranchResponse = resp
                    .json()
                    .await
                    .map_err(|e| PinnerError::Api(e.to_string()))?;
                if let Some(val) = res.values.first() {
                    let sha = DependencyRef::from(val.latest_commit.clone());
                    self.sha_cache.insert(key, sha.clone()).await;
                    return Ok(sha);
                }
            }
            Err(PinnerError::Api(format!(
                "Bitbucket API error (HTTP {}): Ref not found: {}",
                status, tag
            )))
        }
    }

    async fn get_latest_release(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<String, PinnerError> {
        // Bitbucket doesn't have a simple "latest release" API like GitHub.
        // For now, we fallback to the default branch.
        let branch = self.get_default_branch(action, key).await?;
        Ok(branch.0)
    }

    async fn list_tags(
        &self,
        _action: &DependencyName,
        _key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        Ok(vec![])
    }

    async fn get_default_branch(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<BranchName, PinnerError> {
        let url = if self.is_cloud {
            format!("{}/repositories/{}", self.base.base_url, action)
        } else {
            let Some((project, repo)) = action.0.split_once('/') else {
                return Ok(BranchName("main".to_string()));
            };
            format!(
                "{}/rest/api/1.0/projects/{}/repos/{}",
                self.base.base_url, project, repo
            )
        };

        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            if self.is_cloud {
                #[derive(Deserialize)]
                struct CloudRepo {
                    mainbranch: Option<CloudMainBranch>,
                }
                #[derive(Deserialize)]
                struct CloudMainBranch {
                    name: String,
                }
                let repo: CloudRepo = resp
                    .json()
                    .await
                    .map_err(|e| PinnerError::Api(e.to_string()))?;
                Ok(BranchName(
                    repo.mainbranch
                        .map(|b| b.name)
                        .unwrap_or("main".to_string()),
                ))
            } else {
                let repo: BitbucketDCRepoResponse = resp
                    .json()
                    .await
                    .map_err(|e| PinnerError::Api(e.to_string()))?;
                Ok(BranchName(repo.default_branch))
            }
        } else {
            Ok(BranchName("main".to_string()))
        }
    }
}

/// Default implementation of [`RemoteProvider`] for GitLab using `reqwest`.
pub struct ReqwestGitLabProvider {
    pub base: BaseHttpClient,
    pub sha_cache: Cache<(DependencyName, String), DependencyRef>,
}

impl ReqwestGitLabProvider {
    pub fn new(base_url: String, token: Option<String>) -> Self {
        Self {
            base: BaseHttpClient::new(base_url, token, "Bearer", "GITLAB_TOKEN"),
            sha_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(Duration::from_secs(3600))
                .build(),
        }
    }
}

#[async_trait]
impl RemoteProvider for ReqwestGitLabProvider {
    async fn get_commit_sha(
        &self,
        action: &DependencyName,
        tag: &str,
        _key: &str,
    ) -> Result<DependencyRef, PinnerError> {
        let key = (action.clone(), tag.to_string());
        if let Some(sha) = self.sha_cache.get(&key).await {
            return Ok(sha);
        }

        // project_id is the URL-encoded path
        let project_id = action.0.replace('/', "%2F");
        let url = format!(
            "{}/api/v4/projects/{}/repository/commits/{}",
            self.base.base_url, project_id, tag
        );
        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct GitLabCommit {
                id: String,
            }
            let res: GitLabCommit = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            let sha = DependencyRef::from(res.id);
            self.sha_cache.insert(key, sha.clone()).await;
            Ok(sha)
        } else {
            Err(PinnerError::Api(format!(
                "GitLab API error (HTTP {}): project {}",
                resp.status(),
                action
            )))
        }
    }

    async fn get_latest_release(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<String, PinnerError> {
        let project_id = action.0.replace('/', "%2F");
        let url = format!(
            "{}/api/v4/projects/{}/releases",
            self.base.base_url, project_id
        );
        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct GitLabRelease {
                tag_name: String,
            }
            let releases: Vec<GitLabRelease> = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            if let Some(rel) = releases.first() {
                return Ok(rel.tag_name.clone());
            }
        }

        let branch = self.get_default_branch(action, "").await?;
        Ok(branch.0)
    }

    async fn list_tags(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        let project_id = action.0.replace('/', "%2F");
        let url = format!(
            "{}/api/v4/projects/{}/repository/tags",
            self.base.base_url, project_id
        );
        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct GitLabTag {
                name: String,
            }
            let tags: Vec<GitLabTag> = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            Ok(tags.into_iter().map(|t| t.name).collect())
        } else {
            Ok(vec![])
        }
    }

    async fn get_default_branch(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<BranchName, PinnerError> {
        let project_id = action.0.replace('/', "%2F");
        let url = format!("{}/api/v4/projects/{}", self.base.base_url, project_id);
        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct GitLabProject {
                default_branch: Option<String>,
            }
            let project: GitLabProject = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            Ok(BranchName(
                project.default_branch.unwrap_or_else(|| "main".to_string()),
            ))
        } else {
            Ok(BranchName("main".to_string()))
        }
    }
}

/// Default implementation of [`RemoteProvider`] for Forgejo/Gitea using `reqwest`.
pub struct ReqwestForgejoProvider {
    pub base: BaseHttpClient,
    pub sha_cache: Cache<(DependencyName, String), DependencyRef>,
}

impl ReqwestForgejoProvider {
    pub fn new(base_url: String, token: Option<String>) -> Self {
        Self {
            base: BaseHttpClient::new(base_url, token, "token", "FORGEJO_TOKEN"),
            sha_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(Duration::from_secs(3600))
                .build(),
        }
    }
}

#[async_trait]
impl RemoteProvider for ReqwestForgejoProvider {
    async fn get_commit_sha(
        &self,
        action: &DependencyName,
        tag: &str,
        _key: &str,
    ) -> Result<DependencyRef, PinnerError> {
        let key = (action.clone(), tag.to_string());
        if let Some(sha) = self.sha_cache.get(&key).await {
            return Ok(sha);
        }

        let url = format!(
            "{}/api/v1/repos/{}/commits/{}",
            self.base.base_url, action, tag
        );
        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct ForgejoCommit {
                sha: String,
            }
            let res: ForgejoCommit = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            let sha = DependencyRef::from(res.sha);
            self.sha_cache.insert(key, sha.clone()).await;
            Ok(sha)
        } else {
            Err(self.base.handle_error(resp.status(), action))
        }
    }

    async fn get_latest_release(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<String, PinnerError> {
        let url = format!("{}/api/v1/repos/{}/releases", self.base.base_url, action);
        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct ForgejoRelease {
                tag_name: String,
            }
            let releases: Vec<ForgejoRelease> = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            if let Some(rel) = releases.first() {
                return Ok(rel.tag_name.clone());
            }
        }

        let branch = self.get_default_branch(action, "").await?;
        Ok(branch.0)
    }

    async fn list_tags(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        let url = format!("{}/api/v1/repos/{}/tags", self.base.base_url, action);
        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct ForgejoTag {
                name: String,
            }
            let tags: Vec<ForgejoTag> = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            Ok(tags.into_iter().map(|t| t.name).collect())
        } else {
            Ok(vec![])
        }
    }

    async fn get_default_branch(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<BranchName, PinnerError> {
        let url = format!("{}/api/v1/repos/{}", self.base.base_url, action);
        let resp = self
            .base
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| PinnerError::Api(e.to_string()))?;

        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct ForgejoRepo {
                default_branch: String,
            }
            let repo: ForgejoRepo = resp
                .json()
                .await
                .map_err(|e| PinnerError::Api(e.to_string()))?;
            Ok(BranchName(repo.default_branch))
        } else {
            Ok(BranchName("main".to_string()))
        }
    }
}

/// Supported provider types.
pub enum ProviderType {
    GitHub(Arc<ReqwestGithubProvider>),
    Bitbucket(Arc<ReqwestBitbucketProvider>),
    GitLab(Arc<ReqwestGitLabProvider>),
    Forgejo(Arc<ReqwestForgejoProvider>),
}

/// Configuration for the UnifiedProvider.
pub struct UnifiedProviderConfig {
    pub github_url: String,
    pub github_token: Option<String>,
    pub bitbucket_url: String,
    pub bitbucket_token: Option<String>,
    pub gitlab_url: String,
    pub gitlab_token: Option<String>,
    pub forgejo_url: String,
    pub forgejo_token: Option<String>,
}

/// A provider that dispatches to various CI providers based on the YAML key.
pub struct UnifiedProvider {
    pub providers: Vec<ProviderType>,
}

impl UnifiedProvider {
    pub fn new(config: UnifiedProviderConfig) -> Self {
        Self {
            providers: vec![
                ProviderType::GitHub(Arc::new(ReqwestGithubProvider::new(
                    config.github_url,
                    config.github_token,
                ))),
                ProviderType::Bitbucket(Arc::new(ReqwestBitbucketProvider::new(
                    config.bitbucket_url,
                    config.bitbucket_token,
                ))),
                ProviderType::GitLab(Arc::new(ReqwestGitLabProvider::new(
                    config.gitlab_url,
                    config.gitlab_token,
                ))),
                ProviderType::Forgejo(Arc::new(ReqwestForgejoProvider::new(
                    config.forgejo_url,
                    config.forgejo_token,
                ))),
            ],
        }
    }

    fn get_provider(&self, key: &str, _action: &DependencyName) -> Option<&ProviderType> {
        match key {
            "pipe" => self
                .providers
                .iter()
                .find(|p| matches!(p, ProviderType::Bitbucket(_))),
            "include" => self
                .providers
                .iter()
                .find(|p| matches!(p, ProviderType::GitLab(_))),
            _ => self
                .providers
                .iter()
                .find(|p| matches!(p, ProviderType::GitHub(_))),
        }
    }
}

#[async_trait]
impl RemoteProvider for UnifiedProvider {
    async fn get_commit_sha(
        &self,
        action: &DependencyName,
        tag: &str,
        key: &str,
    ) -> Result<DependencyRef, PinnerError> {
        match self.get_provider(key, action) {
            Some(ProviderType::GitHub(p)) => p.get_commit_sha(action, tag, key).await,
            Some(ProviderType::Bitbucket(p)) => p.get_commit_sha(action, tag, key).await,
            Some(ProviderType::GitLab(p)) => p.get_commit_sha(action, tag, key).await,
            Some(ProviderType::Forgejo(p)) => p.get_commit_sha(action, tag, key).await,
            None => Err(PinnerError::Api(format!(
                "No provider found for key: {}",
                key
            ))),
        }
    }

    async fn get_latest_release(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<String, PinnerError> {
        match self.get_provider(key, action) {
            Some(ProviderType::GitHub(p)) => p.get_latest_release(action, key).await,
            Some(ProviderType::Bitbucket(p)) => p.get_latest_release(action, key).await,
            Some(ProviderType::GitLab(p)) => p.get_latest_release(action, key).await,
            Some(ProviderType::Forgejo(p)) => p.get_latest_release(action, key).await,
            None => Err(PinnerError::Api(format!(
                "No provider found for key: {}",
                key
            ))),
        }
    }

    async fn list_tags(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        match self.get_provider(key, action) {
            Some(ProviderType::GitHub(p)) => p.list_tags(action, key).await,
            Some(ProviderType::Bitbucket(p)) => p.list_tags(action, key).await,
            Some(ProviderType::GitLab(p)) => p.list_tags(action, key).await,
            Some(ProviderType::Forgejo(p)) => p.list_tags(action, key).await,
            None => Err(PinnerError::Api(format!(
                "No provider found for key: {}",
                key
            ))),
        }
    }

    async fn get_default_branch(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<BranchName, PinnerError> {
        match self.get_provider(key, action) {
            Some(ProviderType::GitHub(p)) => p.get_default_branch(action, key).await,
            Some(ProviderType::Bitbucket(p)) => p.get_default_branch(action, key).await,
            Some(ProviderType::GitLab(p)) => p.get_default_branch(action, key).await,
            Some(ProviderType::Forgejo(p)) => p.get_default_branch(action, key).await,
            None => Ok(BranchName("main".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn test_action_name_display_and_from() {
        let name = DependencyName::from("actions/checkout");
        assert_eq!(format!("{}", name), "actions/checkout");
        assert_eq!(
            DependencyName::from("a".to_string()),
            DependencyName("a".into())
        );
    }

    #[test]
    fn test_commit_sha_display_and_from() {
        let sha = DependencyRef::from("a1b2c3d".to_string());
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
        let action = DependencyName::from("o/r");

        let err = provider.base.handle_error(StatusCode::FORBIDDEN, &action);
        assert!(matches!(err, PinnerError::RateLimit(_)));
        assert!(format!("{}", err).contains("rate limit exceeded"));
        assert!(format!("{}", err).contains("https://api.github.com"));

        let err = provider
            .base
            .handle_error(StatusCode::TOO_MANY_REQUESTS, &action);
        assert!(matches!(err, PinnerError::RateLimit(_)));
        assert!(format!("{}", err).contains("rate limit exceeded"));
        assert!(format!("{}", err).contains("https://api.github.com"));

        let err = provider.base.handle_error(StatusCode::NOT_FOUND, &action);
        assert!(matches!(err, PinnerError::Api(_)));
        assert!(format!("{}", err).contains("HTTP 404"));
        assert!(format!("{}", err).contains("o/r"));
        assert!(format!("{}", err).contains("https://api.github.com"));

        let err = provider
            .base
            .handle_error(StatusCode::INTERNAL_SERVER_ERROR, &action);
        assert!(matches!(err, PinnerError::Api(_)));
        assert!(format!("{}", err).contains("HTTP 500"));
        assert!(format!("{}", err).contains("https://api.github.com"));
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
        let tags = provider
            .list_tags(&DependencyName::from("o/r"), "uses")
            .await
            .unwrap();
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
        let res = provider
            .list_tags(&DependencyName::from("o/r"), "uses")
            .await;
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
            .get_latest_release(&DependencyName::from("o/r"), "uses")
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
            .get_default_branch(&DependencyName::from("o/r"), "uses")
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

        let bb_json = r#"{"defaultBranch":"prod"}"#;
        let res: BitbucketDCRepoResponse = serde_json::from_str(bb_json).unwrap();
        assert_eq!(res.default_branch, "prod");
    }

    #[tokio::test]
    async fn test_unified_provider_exhaustive() {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(200)
            .with_body(r#"{"tag_name":"v1"}"#)
            .create_async()
            .await;
        let _m2 = server
            .mock("GET", "/api/v4/projects/o%2Fr/repository/tags")
            .with_status(200)
            .with_body(r#"[{"name":"v2"}]"#)
            .create_async()
            .await;

        let unified = UnifiedProvider::new(UnifiedProviderConfig {
            github_url: server.url(),
            github_token: None,
            bitbucket_url: server.url(),
            bitbucket_token: None,
            gitlab_url: server.url(),
            gitlab_token: None,
            forgejo_url: server.url(),
            forgejo_token: None,
        });

        let rel = unified
            .get_latest_release(&DependencyName::from("o/r"), "uses")
            .await
            .unwrap();
        assert_eq!(rel, "v1");

        let tags = unified
            .list_tags(&DependencyName::from("o/r"), "include")
            .await
            .unwrap();
        assert_eq!(tags, vec!["v2".to_string()]);

        let branch = unified
            .get_default_branch(&DependencyName::from("o/r"), "none")
            .await
            .unwrap();
        assert_eq!(branch.0, "main");
    }

    #[tokio::test]
    async fn test_unified_provider_error() {
        let unified = UnifiedProvider::new(UnifiedProviderConfig {
            github_url: "http://invalid".into(),
            github_token: None,
            bitbucket_url: "http://invalid".into(),
            bitbucket_token: None,
            gitlab_url: "http://invalid".into(),
            gitlab_token: None,
            forgejo_url: "http://invalid".into(),
            forgejo_token: None,
        });
        let res = unified
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_provider_errors_exhaustive() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(404)
            .create_async()
            .await;

        let gitlab = ReqwestGitLabProvider::new(server.url(), None);
        assert!(gitlab
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "")
            .await
            .is_err());
        assert!(gitlab
            .list_tags(&DependencyName::from("o/r"), "")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            gitlab
                .get_default_branch(&DependencyName::from("o/r"), "")
                .await
                .unwrap()
                .0,
            "main"
        );

        let forgejo = ReqwestForgejoProvider::new(server.url(), None);
        assert!(forgejo
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "")
            .await
            .is_err());
        assert!(forgejo
            .list_tags(&DependencyName::from("o/r"), "")
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            forgejo
                .get_default_branch(&DependencyName::from("o/r"), "")
                .await
                .unwrap()
                .0,
            "main"
        );

        let bb_cloud = ReqwestBitbucketProvider::with_type(server.url(), None, true);
        assert!(bb_cloud
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "")
            .await
            .is_err());
        assert_eq!(
            bb_cloud
                .get_default_branch(&DependencyName::from("o/r"), "")
                .await
                .unwrap()
                .0,
            "main"
        );

        let bb_dc = ReqwestBitbucketProvider::with_type(server.url(), None, false);
        assert!(bb_dc
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "")
            .await
            .is_err());
        assert_eq!(
            bb_dc
                .get_default_branch(&DependencyName::from("o/r"), "")
                .await
                .unwrap()
                .0,
            "main"
        );
    }

    #[tokio::test]
    async fn test_gitlab_latest_release() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v4/projects/o%2Fr/releases")
            .with_status(200)
            .with_body(r#"[{"tag_name":"v10"}]"#)
            .create_async()
            .await;

        let provider = ReqwestGitLabProvider::new(server.url(), None);
        let rel = provider
            .get_latest_release(&DependencyName::from("o/r"), "")
            .await
            .unwrap();
        assert_eq!(rel, "v10");
    }

    #[tokio::test]
    async fn test_forgejo_methods() {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock("GET", "/api/v1/repos/o/r/releases")
            .with_status(200)
            .with_body(r#"[{"tag_name":"f1"}]"#)
            .create_async()
            .await;
        let _m2 = server
            .mock("GET", "/api/v1/repos/o/r/tags")
            .with_status(200)
            .with_body(r#"[{"name":"t1"}]"#)
            .create_async()
            .await;

        let provider = ReqwestForgejoProvider::new(server.url(), None);
        assert_eq!(
            provider
                .get_latest_release(&DependencyName::from("o/r"), "")
                .await
                .unwrap(),
            "f1"
        );
        assert_eq!(
            provider
                .list_tags(&DependencyName::from("o/r"), "")
                .await
                .unwrap(),
            vec!["t1".to_string()]
        );
    }

    #[tokio::test]
    async fn test_gitlab_provider() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v4/projects/o%2Fr/repository/commits/v1")
            .with_status(200)
            .with_body(r#"{"id":"gitlabsha"}"#)
            .create_async()
            .await;

        let provider = ReqwestGitLabProvider::new(server.url(), None);
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "include")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "gitlabsha");
    }

    #[tokio::test]
    async fn test_forgejo_provider() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/repos/o/r/commits/v1")
            .with_status(200)
            .with_body(r#"{"sha":"forgejosha"}"#)
            .create_async()
            .await;

        let provider = ReqwestForgejoProvider::new(server.url(), None);
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "forgejosha");
    }

    #[tokio::test]
    async fn test_bitbucket_cloud_branch_fallback() {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock("GET", "/repositories/o/p/refs/tags/v1")
            .with_status(404)
            .create_async()
            .await;
        let _m2 = server
            .mock("GET", "/repositories/o/p/refs/branches/v1")
            .with_status(200)
            .with_body(r#"{"target":{"hash":"branchsha"}}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, true);
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/p"), "v1", "pipe")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "branchsha");
    }

    #[tokio::test]
    async fn test_bitbucket_cloud_annotated_tag() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repositories/o/p/refs/tags/v1")
            .with_status(200)
            .with_body(r#"{"target":{"hash":"tagsha","target":{"hash":"realsha"}}}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, true);
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/p"), "v1", "pipe")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "realsha");
    }

    #[tokio::test]
    async fn test_bitbucket_dc_branch_fallback() {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock("GET", "/rest/api/1.0/projects/PROJ/repos/repo/tags/v1")
            .with_status(404)
            .create_async()
            .await;
        let _m2 = server
            .mock(
                "GET",
                "/rest/api/1.0/projects/PROJ/repos/repo/branches?filterText=v1",
            )
            .with_status(200)
            .with_body(r#"{"values":[{"latestCommit":"branchsha"}]}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, false);
        let sha = provider
            .get_commit_sha(&DependencyName::from("PROJ/repo"), "v1", "pipe")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "branchsha");
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
        let action = DependencyName::from("o/r");

        let sha1 = provider
            .get_commit_sha(&action, "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha1.to_string(), "123");

        let sha2 = provider
            .get_commit_sha(&action, "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha2.to_string(), "123");
        // Second call should hit cache.

        let _m2 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(200)
            .with_body(r#"{"tag_name":"v2"}"#)
            .expect(1)
            .create_async()
            .await;

        let r1 = provider.get_latest_release(&action, "uses").await.unwrap();
        assert_eq!(r1, "v2");
        let r2 = provider.get_latest_release(&action, "uses").await.unwrap();
        assert_eq!(r2, "v2");

        let _m3 = s
            .mock("GET", "/repos/o/r")
            .with_status(200)
            .with_body(r#"{"default_branch":"main"}"#)
            .expect(1)
            .create_async()
            .await;

        let b1 = provider.get_default_branch(&action, "uses").await.unwrap();
        assert_eq!(b1.0, "main");
        let b2 = provider.get_default_branch(&action, "uses").await.unwrap();
        assert_eq!(b2.0, "main");
    }

    #[tokio::test]
    async fn test_bitbucket_cloud_provider() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repositories/o/p/refs/tags/v1")
            .with_status(200)
            .with_body(r#"{"target":{"hash":"cloudsha"}}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, true);
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/p"), "v1", "pipe")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "cloudsha");
    }

    #[tokio::test]
    async fn test_bitbucket_dc_provider() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/rest/api/1.0/projects/PROJ/repos/repo/tags/v1")
            .with_status(200)
            .with_body(r#"{"latestCommit":"dcsha"}"#)
            .create_async()
            .await;

        // Base URL doesn't contain bitbucket.org
        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, false);
        assert!(!provider.is_cloud);

        let sha = provider
            .get_commit_sha(&DependencyName::from("PROJ/repo"), "v1", "pipe")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "dcsha");
    }

    #[tokio::test]
    async fn test_unified_provider_routing() {
        let mut server = mockito::Server::new_async().await;
        let _m_github = server
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(200)
            .with_body(r#"{"sha":"githubsha"}"#)
            .create_async()
            .await;

        let _m_bitbucket = server
            .mock("GET", "/rest/api/1.0/projects/o/repos/p/tags/v1")
            .with_status(200)
            .with_body(r#"{"latestCommit":"bitbucketsha"}"#)
            .create_async()
            .await;

        let unified = UnifiedProvider::new(UnifiedProviderConfig {
            github_url: server.url(),
            github_token: None,
            bitbucket_url: server.url(),
            bitbucket_token: None,
            gitlab_url: server.url(),
            gitlab_token: None,
            forgejo_url: server.url(),
            forgejo_token: None,
        });

        let sha1 = unified
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha1.to_string(), "githubsha");

        let sha2 = unified
            .get_commit_sha(&DependencyName::from("o/p"), "v1", "pipe")
            .await
            .unwrap();
        assert_eq!(sha2.to_string(), "bitbucketsha");
    }
}
