use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::provider::{
    BaseHttpClient, RefResponse, ReleaseResponse, RemoteProvider, RepoResponse,
};
use async_trait::async_trait;

/// Default implementation of [`RemoteProvider`] for GitHub using `reqwest`.
pub struct ReqwestGithubProvider {
    pub base: BaseHttpClient,
}

impl ReqwestGithubProvider {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self, PinnerError> {
        Ok(Self {
            base: BaseHttpClient::new(base_url, token, "Bearer", "GITHUB_TOKEN")?,
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
        let repo = action.repository_path();
        let url = format!("{}/repos/{}/commits/{}", self.base.base_url, repo, tag);
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
            Ok(DependencyRef::from(res.sha))
        } else {
            Err(self.base.handle_error(resp, action))
        }
    }

    async fn get_latest_release(
        &self,
        action: &DependencyName,
        key: &str,
    ) -> Result<String, PinnerError> {
        let repo = action.repository_path();
        let url = format!("{}/repos/{}/releases/latest", self.base.base_url, repo);
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

        let repo = action.repository_path();
        let url = format!("{}/repos/{}/tags", self.base.base_url, repo);
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
        let repo = action.repository_path();
        let url = format!("{}/repos/{}", self.base.base_url, repo);
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
    async fn test_github_get_commit_sha() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(200)
            .with_body(r#"{"sha":"githubsha"}"#)
            .create_async()
            .await;

        let provider = ReqwestGithubProvider::new(server.url(), None).unwrap();
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "githubsha");
    }

    #[tokio::test]
    async fn test_github_get_commit_sha_with_subdir() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/snyk/actions/commits/v1")
            .with_status(200)
            .with_body(r#"{"sha":"snyksha"}"#)
            .create_async()
            .await;

        let provider = ReqwestGithubProvider::new(server.url(), None).unwrap();
        let sha = provider
            .get_commit_sha(&DependencyName::from("snyk/actions/setup"), "v1", "uses")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "snyksha");
    }

    #[tokio::test]
    async fn test_github_get_latest_release() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(200)
            .with_body(r#"{"tag_name":"v1.2.3"}"#)
            .create_async()
            .await;

        let provider = ReqwestGithubProvider::new(server.url(), None).unwrap();
        let tag = provider
            .get_latest_release(&DependencyName::from("o/r"), "uses")
            .await
            .unwrap();
        assert_eq!(tag, "v1.2.3");
    }
}
