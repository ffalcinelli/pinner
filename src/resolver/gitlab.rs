use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::provider::{BaseHttpClient, RemoteProvider};
use async_trait::async_trait;
use moka::future::Cache;
use serde::Deserialize;
use std::time::Duration;

pub struct ReqwestGitLabProvider {
    pub base: BaseHttpClient,
    pub sha_cache: Cache<(DependencyName, String), DependencyRef>,
}

impl ReqwestGitLabProvider {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self, PinnerError> {
        Ok(Self {
            base: BaseHttpClient::new(base_url, token, "Bearer", "GITLAB_TOKEN")?,
            sha_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(Duration::from_secs(3600))
                .build(),
        })
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
