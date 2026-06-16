use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::provider::{
    BaseHttpClient, RefResponse, ReleaseResponse, RemoteProvider, RepoResponse,
};
use async_trait::async_trait;
use moka::future::Cache;
use std::time::Duration;

/// Default implementation of [`RemoteProvider`] for GitHub using `reqwest`.
pub struct ReqwestGithubProvider {
    pub base: BaseHttpClient,
    pub sha_cache: Cache<(DependencyName, String), DependencyRef>,
    pub release_cache: Cache<DependencyName, String>,
    pub branch_cache: Cache<DependencyName, BranchName>,
}

impl ReqwestGithubProvider {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self, PinnerError> {
        Ok(Self {
            base: BaseHttpClient::new(base_url, token, "Bearer", "GITHUB_TOKEN")?,
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
        })
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
            Err(self.base.handle_error(resp, action))
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
            Err(self.base.handle_error(resp, action))
        }
    }

    async fn list_tags(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        #[derive(serde::Deserialize)]
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
            Err(self.base.handle_error(resp, action))
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
