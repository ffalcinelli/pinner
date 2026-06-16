use crate::core::{BranchName, DependencyName, DependencyRef};
use crate::error::PinnerError;
use crate::resolver::github::ReqwestGithubProvider;
use crate::resolver::provider::RemoteProvider;
use async_trait::async_trait;

/// Specialized provider for Azure Marketplace tasks, mapping them to the
/// `microsoft/azure-pipelines-tasks` GitHub repository.
pub struct ReqwestAzureProvider {
    github: ReqwestGithubProvider,
    target_repo: DependencyName,
}

impl ReqwestAzureProvider {
    pub fn new(github: ReqwestGithubProvider) -> Self {
        Self {
            github,
            target_repo: DependencyName::from("microsoft/azure-pipelines-tasks"),
        }
    }
}

#[async_trait]
impl RemoteProvider for ReqwestAzureProvider {
    async fn get_commit_sha(
        &self,
        _action: &DependencyName,
        _tag: &str,
        key: &str,
    ) -> Result<DependencyRef, PinnerError> {
        // For Azure tasks, we map to the latest release of the monorepo
        let latest_tag = self.get_latest_release(_action, key).await?;
        self.github
            .get_commit_sha(&self.target_repo, &latest_tag, key)
            .await
    }

    async fn get_latest_release(
        &self,
        _action: &DependencyName,
        key: &str,
    ) -> Result<String, PinnerError> {
        self.github.get_latest_release(&self.target_repo, key).await
    }

    async fn list_tags(
        &self,
        _action: &DependencyName,
        key: &str,
    ) -> Result<Vec<String>, PinnerError> {
        self.github.list_tags(&self.target_repo, key).await
    }

    async fn get_default_branch(
        &self,
        _action: &DependencyName,
        key: &str,
    ) -> Result<BranchName, PinnerError> {
        self.github.get_default_branch(&self.target_repo, key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_azure_provider_proxying() {
        let mut server = mockito::Server::new_async().await;
        // Mock latest release lookup for the monorepo
        let _m_rel = server
            .mock(
                "GET",
                "/repos/microsoft/azure-pipelines-tasks/releases/latest",
            )
            .with_status(200)
            .with_body(r#"{"tag_name":"v3.238.1"}"#)
            .create_async()
            .await;

        // Mock SHA lookup for that release
        let _m_sha = server
            .mock(
                "GET",
                "/repos/microsoft/azure-pipelines-tasks/commits/v3.238.1",
            )
            .with_status(200)
            .with_body(r#"{"sha":"azuresha"}"#)
            .create_async()
            .await;

        let github = ReqwestGithubProvider::new(server.url(), None).unwrap();
        let azure = ReqwestAzureProvider::new(github);

        let sha = azure
            .get_commit_sha(&DependencyName::from("NodeTool"), "0", "task")
            .await
            .unwrap();
        assert_eq!(sha.to_string(), "azuresha");
    }
}
