use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::provider::{BaseHttpClient, RemoteProvider};
use async_trait::async_trait;
use moka::future::Cache;
use serde::Deserialize;
use std::time::Duration;

pub struct ReqwestForgejoProvider {
    pub base: BaseHttpClient,
    pub sha_cache: Cache<(DependencyName, String), DependencyRef>,
}

impl ReqwestForgejoProvider {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self, PinnerError> {
        Ok(Self {
            base: BaseHttpClient::new(base_url, token, "token", "FORGEJO_TOKEN")?,
            sha_cache: Cache::builder()
                .max_capacity(1000)
                .time_to_live(Duration::from_secs(3600))
                .build(),
        })
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
            Err(self.base.handle_error(resp, action))
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
