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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_bitbucket_cloud_get_commit_sha_tag() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repositories/o/r/refs/tags/v1")
            .with_status(200)
            .with_body(r#"{"target":{"hash":"cloudsha"}}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, true).unwrap();
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "cloudsha");
    }

    #[tokio::test]
    async fn test_bitbucket_cloud_get_commit_sha_branch_fallback() {
        let mut server = mockito::Server::new_async().await;
        let _m1 = server
            .mock("GET", "/repositories/o/r/refs/tags/main")
            .with_status(404)
            .create_async()
            .await;
        let _m2 = server
            .mock("GET", "/repositories/o/r/refs/branches/main")
            .with_status(200)
            .with_body(r#"{"target":{"hash":"branchsha"}}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, true).unwrap();
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/r"), "main", "uses")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "branchsha");
    }

    #[tokio::test]
    async fn test_bitbucket_dc_get_commit_sha() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/rest/api/1.0/projects/PROJ/repos/repo/tags/v1")
            .with_status(200)
            .with_body(r#"{"latestCommit":"dcsha"}"#)
            .create_async()
            .await;

        // Force is_cloud to false by using a non-bitbucket.org URL
        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, false).unwrap();
        let sha = provider
            .get_commit_sha(&DependencyName::from("PROJ/repo"), "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "dcsha");
    }

    #[tokio::test]
    async fn test_bitbucket_cloud_get_default_branch() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repositories/o/r")
            .with_status(200)
            .with_body(r#"{"mainbranch":{"name":"develop"}}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, true).unwrap();
        let branch = provider
            .get_default_branch(&DependencyName::from("o/r"), "uses")
            .await
            .unwrap();
        assert_eq!(branch.0, "develop");
    }

    #[tokio::test]
    async fn test_bitbucket_dc_get_default_branch() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/rest/api/1.0/projects/P/repos/R")
            .with_status(200)
            .with_body(r#"{"defaultBranch":"master"}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, false).unwrap();
        let branch = provider
            .get_default_branch(&DependencyName::from("P/R"), "uses")
            .await
            .unwrap();
        assert_eq!(branch.0, "master");
    }

    #[tokio::test]
    async fn test_bitbucket_dc_get_commit_sha_branch_fallback() {
        let mut server = mockito::Server::new_async().await;
        // Mock tag lookup failure
        let _m1 = server
            .mock("GET", "/rest/api/1.0/projects/P/repos/R/tags/v1")
            .with_status(404)
            .create_async()
            .await;
        // Mock branch lookup success
        let _m2 = server
            .mock(
                "GET",
                "/rest/api/1.0/projects/P/repos/R/branches?filterText=v1",
            )
            .with_status(200)
            .with_body(r#"{"values":[{"displayId":"v1","latestCommit":"dcbranchsha"}]}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, false).unwrap();
        let sha = provider
            .get_commit_sha(&DependencyName::from("P/R"), "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "dcbranchsha");
    }

    #[tokio::test]
    async fn test_bitbucket_invalid_action_format() {
        let provider =
            ReqwestBitbucketProvider::with_type("http://localhost".to_string(), None, false)
                .unwrap();
        let res = provider
            .get_commit_sha(&DependencyName::from("invalid"), "v1", "uses")
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_bitbucket_cloud_list_tags() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repositories/o/r/refs/tags")
            .with_status(200)
            .with_body(r#"{"values":[{"name":"v1"},{"name":"v2"}]}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, true).unwrap();
        let tags = provider
            .list_tags(&DependencyName::from("o/r"), "uses")
            .await
            .unwrap();
        assert_eq!(tags, vec!["v1", "v2"]);
    }

    #[tokio::test]
    async fn test_bitbucket_dc_list_tags() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/rest/api/1.0/projects/P/repos/R/tags")
            .with_status(200)
            .with_body(r#"{"values":[{"display_id":"v1"},{"display_id":"v2"}]}"#)
            .create_async()
            .await;

        let provider = ReqwestBitbucketProvider::with_type(server.url(), None, false).unwrap();
        let tags = provider
            .list_tags(&DependencyName::from("P/R"), "uses")
            .await
            .unwrap();
        assert_eq!(tags, vec!["v1", "v2"]);
    }
}
