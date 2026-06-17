use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::provider::{BaseHttpClient, RemoteProvider};
use async_trait::async_trait;
use serde::Deserialize;

pub struct ReqwestForgejoProvider {
    pub base: BaseHttpClient,
}

impl ReqwestForgejoProvider {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self, PinnerError> {
        Ok(Self {
            base: BaseHttpClient::new(base_url, token, "token", "FORGEJO_TOKEN")?,
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
            Ok(DependencyRef::from(res.sha))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_forgejo_get_commit_sha() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/repos/o/r/commits/v1")
            .with_status(200)
            .with_body(r#"{"sha":"forgejosha"}"#)
            .create_async()
            .await;

        let provider = ReqwestForgejoProvider::new(server.url(), None).unwrap();
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "forgejosha");
    }

    #[tokio::test]
    async fn test_forgejo_get_latest_release() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/repos/o/r/releases")
            .with_status(200)
            .with_body(r#"[{"tag_name":"v2.0.0"}]"#)
            .create_async()
            .await;

        let provider = ReqwestForgejoProvider::new(server.url(), None).unwrap();
        let tag = provider
            .get_latest_release(&DependencyName::from("o/r"), "uses")
            .await
            .unwrap();
        assert_eq!(tag, "v2.0.0");
    }

    #[tokio::test]
    async fn test_forgejo_list_tags() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/repos/o/r/tags")
            .with_status(200)
            .with_body(r#"[{"name":"v1"},{"name":"v2"}]"#)
            .create_async()
            .await;

        let provider = ReqwestForgejoProvider::new(server.url(), None).unwrap();
        let tags = provider
            .list_tags(&DependencyName::from("o/r"), "uses")
            .await
            .unwrap();
        assert_eq!(tags, vec!["v1", "v2"]);
    }

    #[tokio::test]
    async fn test_forgejo_get_default_branch() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v1/repos/o/r")
            .with_status(200)
            .with_body(r#"{"default_branch":"develop"}"#)
            .create_async()
            .await;

        let provider = ReqwestForgejoProvider::new(server.url(), None).unwrap();
        let branch = provider
            .get_default_branch(&DependencyName::from("o/r"), "uses")
            .await
            .unwrap();
        assert_eq!(branch.0, "develop");
    }
}
