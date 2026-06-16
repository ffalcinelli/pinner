use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::provider::{BaseHttpClient, RemoteProvider};
use async_trait::async_trait;
use serde::Deserialize;

pub struct ReqwestBitbucketProvider {
    pub base: BaseHttpClient,
    pub is_cloud: bool,
}

impl ReqwestBitbucketProvider {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self, PinnerError> {
        let is_cloud = base_url.contains("bitbucket.org");
        Self::with_type(base_url, token, is_cloud)
    }

    pub fn with_type(
        base_url: String,
        token: Option<String>,
        is_cloud: bool,
    ) -> Result<Self, PinnerError> {
        Ok(Self {
            base: BaseHttpClient::new(base_url, token, "Bearer", "BITBUCKET_TOKEN")?,
            is_cloud,
        })
    }
}

#[derive(Deserialize)]
struct BitbucketCloudRefResponse {
    target: BitbucketCloudTarget,
}

#[derive(Deserialize)]
struct BitbucketCloudTarget {
    hash: String,
    target: Option<BitbucketCloudInnerTarget>,
}

#[derive(Deserialize)]
struct BitbucketCloudInnerTarget {
    hash: String,
}

#[derive(Deserialize)]
struct BitbucketDCRefResponse {
    #[serde(rename = "latestCommit")]
    latest_commit: String,
}

#[derive(Deserialize)]
struct BitbucketDCRepoResponse {
    #[serde(rename = "defaultBranch")]
    default_branch: String,
}

#[async_trait]
impl RemoteProvider for ReqwestBitbucketProvider {
    async fn get_commit_sha(
        &self,
        action: &DependencyName,
        tag: &str,
        _key: &str,
    ) -> Result<DependencyRef, PinnerError> {
        let url = if self.is_cloud {
            format!(
                "{}/repositories/{}/refs/tags/{}",
                self.base.base_url, action, tag
            )
        } else {
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
                res.target.target.map(|t| t.hash).unwrap_or(res.target.hash)
            } else {
                let res: BitbucketDCRefResponse = resp
                    .json()
                    .await
                    .map_err(|e| PinnerError::Api(e.to_string()))?;
                res.latest_commit
            };

            Ok(DependencyRef::from(sha))
        } else if self.is_cloud {
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
                Ok(DependencyRef::from(res.target.hash))
            } else {
                Err(PinnerError::Api(format!(
                    "Bitbucket API error (HTTP {}): Ref not found: {}",
                    status, tag
                )))
            }
        } else {
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
                    return Ok(DependencyRef::from(val.latest_commit.clone()));
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
        let branch = self.get_default_branch(action, key).await?;
        Ok(branch.0)
    }

    async fn list_tags(
        &self,
        action: &DependencyName,
        _key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        let url = if self.is_cloud {
            format!("{}/repositories/{}/refs/tags", self.base.base_url, action)
        } else {
            let Some((project, repo)) = action.0.split_once('/') else {
                return Ok(vec![]);
            };
            format!(
                "{}/rest/api/1.0/projects/{}/repos/{}/tags",
                self.base.base_url, project, repo
            )
        };

        let resp = self.base.client.get(&url).send().await?;

        if !resp.status().is_success() {
            return Ok(vec![]);
        }

        #[derive(Deserialize)]
        struct BitbucketCloudTagsResponse {
            values: Vec<BitbucketCloudTag>,
        }
        #[derive(Deserialize)]
        struct BitbucketCloudTag {
            name: String,
        }

        #[derive(Deserialize)]
        struct BitbucketDCTagsResponse {
            values: Vec<BitbucketDCTag>,
        }
        #[derive(Deserialize)]
        struct BitbucketDCTag {
            display_id: String,
        }

        if self.is_cloud {
            let res: BitbucketCloudTagsResponse = resp.json().await?;
            Ok(res.values.into_iter().map(|t| t.name).collect())
        } else {
            let res: BitbucketDCTagsResponse = resp.json().await?;
            Ok(res.values.into_iter().map(|t| t.display_id).collect())
        }
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
