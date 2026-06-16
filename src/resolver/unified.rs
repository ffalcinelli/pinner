use crate::cli::UpgradeStrategy;
use crate::core::{DependencyRef, UpdateResult, UpdateTask};
use crate::error::PinnerError;
use crate::resolver::provider::RemoteProvider;
use crate::resolver::registry::RegistryProvider;
use futures::stream::{self, StreamExt};
use std::sync::Arc;

pub struct Resolver {
    pub remote: Arc<dyn RemoteProvider>,
    pub registry: Arc<dyn RegistryProvider>,
    pub upgrade_strategy: UpgradeStrategy,
    pub concurrency: usize,
}

impl Resolver {
    pub fn new(
        remote: Arc<dyn RemoteProvider>,
        registry: Arc<dyn RegistryProvider>,
        upgrade_strategy: UpgradeStrategy,
        concurrency: usize,
    ) -> Self {
        Self {
            remote,
            registry,
            upgrade_strategy,
            concurrency,
        }
    }

    pub async fn resolve_tasks(
        &self,
        tasks: Vec<UpdateTask>,
        is_pin: bool,
    ) -> Result<Vec<UpdateResult>, PinnerError> {
        let futs = tasks.into_iter().map(|task| {
            let remote = self.remote.clone();
            let registry = self.registry.clone();
            let strategy = self.upgrade_strategy.clone();
            async move {
                let res = if is_pin {
                    Self::resolve_pin(&task, remote, registry).await
                } else {
                    Self::resolve_upgrade(&task, remote, registry, strategy).await
                };

                match res {
                    Ok(Some((sha, tag))) => Ok(Some(UpdateResult {
                        action: task.action.clone(),
                        path: task.path.clone(),
                        old_tag: task.current_tag.clone(),
                        task,
                        new_sha: sha,
                        new_tag: tag,
                    })),
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                }
            }
        });

        let results: Vec<Result<UpdateResult, PinnerError>> = stream::iter(futs)
            .buffer_unordered(self.concurrency)
            .filter_map(|res| async {
                match res {
                    Ok(Some(r)) => Some(Ok(r)),
                    Ok(None) => None,
                    Err(e) if e.is_fatal() => Some(Err(e)),
                    Err(e) => {
                        eprintln!("Warning: Skipping action due to error: {}", e);
                        None
                    }
                }
            })
            .collect()
            .await;

        results.into_iter().collect()
    }

    async fn resolve_pin(
        task: &UpdateTask,
        remote: Arc<dyn RemoteProvider>,
        registry: Arc<dyn RegistryProvider>,
    ) -> Result<Option<(DependencyRef, Option<String>)>, PinnerError> {
        if let Some(ver) = &task.current_tag {
            if task.action.0.starts_with("docker://") || task.key == "image" {
                if !ver.starts_with("sha256:") {
                    let image = task.action.0.trim_start_matches("docker://");
                    let digest = registry.resolve_digest(image, ver).await?;
                    return Ok(Some((DependencyRef::from(digest), Some(ver.clone()))));
                }
            } else if ver.len() != 40 {
                let sha = remote.get_commit_sha(&task.action, ver, &task.key).await?;
                return Ok(Some((sha, Some(ver.clone()))));
            }
        }
        Ok(None)
    }

    async fn resolve_upgrade(
        task: &UpdateTask,
        remote: Arc<dyn RemoteProvider>,
        registry: Arc<dyn RegistryProvider>,
        strategy: UpgradeStrategy,
    ) -> Result<Option<(DependencyRef, Option<String>)>, PinnerError> {
        if task.action.0.starts_with("docker://") || task.key == "image" {
            let image = task.action.0.trim_start_matches("docker://");
            let tag = task.current_tag.as_deref().unwrap_or("latest");
            let digest = registry.resolve_digest(image, tag).await?;
            return Ok(Some((DependencyRef::from(digest), Some(tag.to_string()))));
        }

        if strategy == UpgradeStrategy::Commit {
            let branch = remote.get_default_branch(&task.action, &task.key).await?;
            let sha = remote
                .get_commit_sha(&task.action, &branch.0, &task.key)
                .await?;
            return Ok(Some((sha, Some(branch.0))));
        }

        let latest_tag = if strategy == UpgradeStrategy::Latest {
            Some(remote.get_latest_release(&task.action, &task.key).await?)
        } else {
            let tags = remote.list_tags(&task.action, &task.key).await?;
            let current_tag = task.current_tag.as_deref().unwrap_or("");
            let current_version = semver::Version::parse(current_tag.trim_start_matches('v')).ok();

            let mut filtered_tags: Vec<_> = tags
                .into_iter()
                .filter_map(|t| {
                    semver::Version::parse(t.trim_start_matches('v'))
                        .ok()
                        .map(|v| (t, v))
                })
                .collect();

            filtered_tags.sort_by(|a, b| b.1.cmp(&a.1));

            if let Some(cv) = current_version {
                filtered_tags
                    .into_iter()
                    .find(|(_, v)| match strategy {
                        UpgradeStrategy::Major => v.major == cv.major && v > &cv,
                        UpgradeStrategy::Minor => {
                            v.major == cv.major && v.minor == cv.minor && v > &cv
                        }
                        _ => false,
                    })
                    .map(|(t, _)| t)
            } else {
                None
            }
        };

        if let Some(tag) = latest_tag {
            if Some(&tag) != task.current_tag.as_ref() {
                let sha = remote.get_commit_sha(&task.action, &tag, &task.key).await?;
                return Ok(Some((sha, Some(tag))));
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{BranchName, DependencyName};
    use crate::resolver::provider::MockRemoteProvider;
    use crate::resolver::registry::MockRegistryProvider;
    use mockall::predicate::*;

    #[tokio::test]
    async fn test_resolve_pin_action() {
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_commit_sha()
            .with(
                eq(DependencyName::from("actions/checkout")),
                eq("v3"),
                eq("uses"),
            )
            .returning(|_, _, _| Ok(DependencyRef::GitSha("hash".to_string())));

        let registry = MockRegistryProvider::new();
        let task = UpdateTask {
            path: "f.yml".into(),
            start: 0,
            end: 0,
            action: "actions/checkout".into(),
            current_tag: Some("v3".to_string()),
            comment: None,
            key: "uses".to_string(),
        };

        let res = Resolver::resolve_pin(&task, Arc::new(remote), Arc::new(registry))
            .await
            .unwrap();
        assert_eq!(
            res,
            Some((
                DependencyRef::GitSha("hash".to_string()),
                Some("v3".to_string())
            ))
        );
    }

    #[tokio::test]
    async fn test_resolve_upgrade_latest() {
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_latest_release()
            .returning(|_, _| Ok("v4".to_string()));
        remote
            .expect_get_commit_sha()
            .returning(|_, tag, _| Ok(DependencyRef::GitSha(format!("{}sha", tag))));

        let registry = MockRegistryProvider::new();
        let task = UpdateTask {
            path: "f.yml".into(),
            start: 0,
            end: 0,
            action: "actions/checkout".into(),
            current_tag: Some("v3".to_string()),
            comment: None,
            key: "uses".to_string(),
        };

        let res = Resolver::resolve_upgrade(
            &task,
            Arc::new(remote),
            Arc::new(registry),
            UpgradeStrategy::Latest,
        )
        .await
        .unwrap();
        assert_eq!(
            res,
            Some((
                DependencyRef::GitSha("v4sha".to_string()),
                Some("v4".to_string())
            ))
        );
    }

    #[tokio::test]
    async fn test_resolve_upgrade_major() {
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_list_tags()
            .returning(|_, _| Ok(vec!["v1.1.0".to_string(), "v2.0.0".to_string()]));
        remote
            .expect_get_commit_sha()
            .returning(|_, tag, _| Ok(DependencyRef::GitSha(format!("{}sha", tag))));

        let registry = MockRegistryProvider::new();
        let task = UpdateTask {
            path: "f.yml".into(),
            start: 0,
            end: 0,
            action: "actions/checkout".into(),
            current_tag: Some("v1.0.0".to_string()),
            comment: None,
            key: "uses".to_string(),
        };

        let res = Resolver::resolve_upgrade(
            &task,
            Arc::new(remote),
            Arc::new(registry),
            UpgradeStrategy::Major,
        )
        .await
        .unwrap();
        assert_eq!(
            res,
            Some((
                DependencyRef::GitSha("v1.1.0sha".to_string()),
                Some("v1.1.0".to_string())
            ))
        );
    }

    #[tokio::test]
    async fn test_resolve_upgrade_commit() {
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_default_branch()
            .returning(|_, _| Ok(BranchName("main".to_string())));
        remote
            .expect_get_commit_sha()
            .returning(|_, tag, _| Ok(DependencyRef::GitSha(format!("{}sha", tag))));

        let registry = MockRegistryProvider::new();
        let task = UpdateTask {
            path: "f.yml".into(),
            start: 0,
            end: 0,
            action: "actions/checkout".into(),
            current_tag: Some("v3".to_string()),
            comment: None,
            key: "uses".to_string(),
        };

        let res = Resolver::resolve_upgrade(
            &task,
            Arc::new(remote),
            Arc::new(registry),
            UpgradeStrategy::Commit,
        )
        .await
        .unwrap();
        assert_eq!(
            res,
            Some((
                DependencyRef::GitSha("mainsha".to_string()),
                Some("main".to_string())
            ))
        );
    }
}
