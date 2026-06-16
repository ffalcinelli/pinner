use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::provider::{BaseHttpClient, RemoteProvider};
use async_trait::async_trait;
use serde::Deserialize;

pub struct ReqwestGitLabProvider {
    pub base: BaseHttpClient,
}

impl ReqwestGitLabProvider {
    pub fn new(base_url: String, token: Option<String>) -> Result<Self, PinnerError> {
        Ok(Self {
            base: BaseHttpClient::new(base_url, token, "Bearer", "GITLAB_TOKEN")?,
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
            Ok(DependencyRef::from(res.id))
        } else {
            Err(self.base.handle_error(resp, action))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_gitlab_get_commit_sha() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v4/projects/o%2Fr/repository/commits/v1")
            .with_status(200)
            .with_body(r#"{"id":"gitlabsha"}"#)
            .create_async()
            .await;

        let provider = ReqwestGitLabProvider::new(server.url(), None).unwrap();
        let sha = provider
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "include")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "gitlabsha");
    }

    #[tokio::test]
    async fn test_gitlab_get_default_branch() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/v4/projects/o%2Fr")
            .with_status(200)
            .with_body(r#"{"default_branch":"develop"}"#)
            .create_async()
            .await;

        let provider = ReqwestGitLabProvider::new(server.url(), None).unwrap();
        let branch = provider
            .get_default_branch(&DependencyName::from("o/r"), "")
            .await
            .unwrap();
        assert_eq!(branch.0, "develop");
    }
}
