use crate::cli::UpgradeStrategy;
use crate::core::{DependencyRef, UpdateResult, UpdateTask};
use crate::error::PinnerError;
use crate::resolver::provider::RemoteProvider;
use crate::resolver::registry::RegistryProvider;
use futures::stream::{self, StreamExt};
use std::sync::Arc;

/// The `Resolver` is the high-level engine that maps `UpdateTask`s to `UpdateResult`s.
///
/// It orchestrates the network-intensive process of resolving symbolic tags to hashes,
/// using a `RemoteProvider` for repository-based actions and a `RegistryProvider`
/// for container images. It handles task grouping to minimize redundant requests
/// and manages concurrency.
pub struct Resolver {
    /// The provider used to fetch data from remote CI platforms.
    pub remote: Arc<dyn RemoteProvider>,
    /// The provider used to fetch digests from OCI registries.
    pub registry: Arc<dyn RegistryProvider>,
    /// The strategy to use when upgrading (e.g., Latest, Major, Minor).
    pub upgrade_strategy: UpgradeStrategy,
    /// Maximum number of concurrent network requests.
    pub concurrency: usize,
}

impl Resolver {
    /// Creates a new `Resolver`.
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

    /// Resolves a batch of tasks into results.
    ///
    /// This method:
    /// 1. Groups tasks by action and tag to avoid fetching the same hash multiple times.
    /// 2. Dispatches resolution tasks to an asynchronous stream.
    /// 3. Processes tasks concurrently up to the `concurrency` limit.
    /// 4. Filters out non-fatal errors (e.g., a single action failing to resolve)
    ///    while propagating fatal errors (e.g., rate limits).
    pub async fn resolve_tasks(
        &self,
        tasks: Vec<UpdateTask>,
        is_pin: bool,
    ) -> Result<Vec<UpdateResult>, PinnerError> {
        // Group tasks by (action, current_tag, key) to avoid redundant network requests.
        // For example, if 'actions/checkout@v3' is used in 10 files, we only fetch its hash once.
        let mut groups: std::collections::HashMap<
            (String, Option<String>, String),
            Vec<UpdateTask>,
        > = std::collections::HashMap::new();

        for task in tasks {
            let key = (
                task.action.0.clone(),
                task.current_tag.clone(),
                task.key.clone(),
            );
            groups.entry(key).or_default().push(task);
        }

        let futs = groups.into_values().map(|tasks| {
            let remote = self.remote.clone();
            let registry = self.registry.clone();
            let strategy = self.upgrade_strategy.clone();
            // We only need to resolve the first task in the group to get the new hash/tag.
            let sample_task = tasks[0].clone();
            async move {
                let res = if is_pin {
                    Self::resolve_pin(&sample_task, remote, registry).await
                } else {
                    Self::resolve_upgrade(&sample_task, remote, registry, strategy).await
                };

                match res {
                    Ok(Some((sha, tag))) => {
                        let mut results = Vec::new();
                        for task in tasks {
                            results.push(UpdateResult {
                                action: task.action.clone(),
                                path: task.path.clone(),
                                old_tag: task.current_tag.clone(),
                                task,
                                new_sha: sha.clone(),
                                new_tag: tag.clone(),
                            });
                        }
                        Ok(Some(results))
                    }
                    Ok(None) => Ok(None),
                    Err(e) => Err(e),
                }
            }
        });

        // Use a futures stream to handle concurrency. `buffer_unordered` allows up to `self.concurrency`
        // tasks to run in parallel, completing in any order.
        let results: Vec<Result<Vec<UpdateResult>, PinnerError>> = stream::iter(futs)
            .buffer_unordered(self.concurrency)
            .filter_map(|res| async {
                match res {
                    Ok(Some(r)) => Some(Ok(r)),
                    Ok(None) => None,
                    Err(e) if e.is_fatal() => Some(Err(e)),
                    Err(e) => {
                        // Non-fatal errors are reported as warnings and skipped.
                        eprintln!("Warning: Skipping action due to error: {}", e);
                        None
                    }
                }
            })
            .collect()
            .await;

        let mut all_results = Vec::new();
        for res in results {
            all_results.extend(res?);
        }
        Ok(all_results)
    }

    async fn resolve_pin(
        task: &UpdateTask,
        remote: Arc<dyn RemoteProvider>,
        registry: Arc<dyn RegistryProvider>,
    ) -> Result<Option<(DependencyRef, Option<String>)>, PinnerError> {
        if task.key == "orbs" {
            // Orbs are strictly semantic versioned and immutable, no need to hash-pin.
            return Ok(None);
        }

        if let Some(ver) = &task.current_tag {
            if task.action.is_docker() || task.key == "image" {
                if !ver.starts_with("sha256:") {
                    let image = task.action.trim_docker_prefix();
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
        if task.key == "orbs" {
            let tag = remote.get_latest_release(&task.action, &task.key).await?;
            let current_tag = task.logical_tag().unwrap_or_default();
            if is_newer(&tag, &current_tag) {
                return Ok(Some((DependencyRef::Version(tag), None)));
            }
            return Ok(None);
        }

        if task.action.is_docker() || task.key == "image" {
            let image = task.action.trim_docker_prefix();
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
            let tag = remote.get_latest_release(&task.action, &task.key).await?;
            let current_tag = task.logical_tag().unwrap_or_default();
            if is_newer(&tag, &current_tag) {
                Some(tag)
            } else {
                None
            }
        } else {
            let tags = remote.list_tags(&task.action, &task.key).await?;
            let current_tag = task.logical_tag().unwrap_or_default();
            let current_version = parse_relaxed_semver(&current_tag);

            let mut filtered_tags: Vec<_> = tags
                .into_iter()
                .filter_map(|t| parse_relaxed_semver(&t).map(|v| (t, v)))
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

    /// Gets the upgrade candidate version/ref for a task, without applying is_newer checks.
    pub async fn get_upgrade_candidate(
        &self,
        task: &UpdateTask,
    ) -> Result<Option<(DependencyRef, Option<String>)>, PinnerError> {
        let remote = self.remote.clone();
        let registry = self.registry.clone();
        let strategy = self.upgrade_strategy.clone();

        if task.key == "orbs" {
            let tag = remote.get_latest_release(&task.action, &task.key).await?;
            return Ok(Some((DependencyRef::Version(tag.clone()), Some(tag))));
        }

        if task.action.is_docker() || task.key == "image" {
            let image = task.action.trim_docker_prefix();
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
            let current_tag = task.logical_tag().unwrap_or_default();
            let current_version = parse_relaxed_semver(&current_tag);

            let mut filtered_tags: Vec<_> = tags
                .into_iter()
                .filter_map(|t| parse_relaxed_semver(&t).map(|v| (t, v)))
                .collect();

            filtered_tags.sort_by(|a, b| b.1.cmp(&a.1));

            if let Some(cv) = current_version {
                filtered_tags
                    .into_iter()
                    .find(|(_, v)| match strategy {
                        UpgradeStrategy::Major => v.major == cv.major,
                        UpgradeStrategy::Minor => v.major == cv.major && v.minor == cv.minor,
                        _ => false,
                    })
                    .map(|(t, _)| t)
            } else {
                None
            }
        };

        if let Some(tag) = latest_tag {
            let sha = remote.get_commit_sha(&task.action, &tag, &task.key).await?;
            return Ok(Some((sha, Some(tag))));
        }

        Ok(None)
    }
}

fn normalize_semver(s: &str) -> String {
    let s = s.trim_start_matches('v');
    let parts: Vec<&str> = s.split('.').collect();
    if parts.is_empty() || parts[0].is_empty() {
        return s.to_string();
    }
    if !parts[0].chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return s.to_string();
    }

    if parts.len() == 1 {
        let part = parts[0];
        if let Some((num, rest)) = split_numeric_prefix(part) {
            if rest.is_empty() {
                format!("{}.0.0", num)
            } else {
                format!("{}.0.0{}", num, rest)
            }
        } else {
            s.to_string()
        }
    } else if parts.len() == 2 {
        let major = parts[0];
        let minor_part = parts[1];
        if let Some((num, rest)) = split_numeric_prefix(minor_part) {
            if rest.is_empty() {
                format!("{}.{}.0", major, num)
            } else {
                format!("{}.{}.0{}", major, num, rest)
            }
        } else {
            s.to_string()
        }
    } else {
        s.to_string()
    }
}

fn split_numeric_prefix(s: &str) -> Option<(String, String)> {
    let mut num_end = 0;
    for c in s.chars() {
        if c.is_ascii_digit() {
            num_end += 1;
        } else {
            break;
        }
    }
    if num_end > 0 {
        Some((s[..num_end].to_string(), s[num_end..].to_string()))
    } else {
        None
    }
}

fn parse_relaxed_semver(s: &str) -> Option<semver::Version> {
    semver::Version::parse(&normalize_semver(s)).ok()
}

fn is_newer(new_tag: &str, current_tag: &str) -> bool {
    match (
        parse_relaxed_semver(new_tag),
        parse_relaxed_semver(current_tag),
    ) {
        (Some(new_ver), Some(curr_ver)) => new_ver > curr_ver,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        _ => new_tag != current_tag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{BranchName, DependencyName};
    use crate::resolver::provider::MockRemoteProvider;
    use crate::resolver::registry::MockRegistryProvider;
    use mockall::predicate::{always, eq};

    #[test]
    fn test_normalize_semver() {
        assert_eq!(normalize_semver("v3"), "3.0.0");
        assert_eq!(normalize_semver("3-alpha"), "3.0.0-alpha");
        assert_eq!(normalize_semver("4.1"), "4.1.0");
        assert_eq!(normalize_semver("4.1-beta"), "4.1.0-beta");
        assert_eq!(normalize_semver("v1.2.3"), "1.2.3");
        assert_eq!(normalize_semver("main"), "main");
    }

    #[test]
    fn test_is_newer() {
        assert!(is_newer("v4", "v3"));
        assert!(is_newer("v6.0.3", "v6.0.2"));
        assert!(!is_newer("v4", "v6.0.2"));
        assert!(!is_newer("v6.0.2", "v6.0.2"));
        assert!(is_newer("v4", "main"));
        assert!(!is_newer("main", "v4"));
        assert!(!is_newer("main", "v1.0.0"));
    }

    #[tokio::test]
    async fn test_resolve_upgrade_pinned_higher_than_remote() {
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_latest_release()
            .returning(|_, _| Ok("v4".to_string()));

        let registry = MockRegistryProvider::new();
        let task = UpdateTask {
            path: "f.yml".into(),
            action: "actions/checkout".into(),
            current_tag: Some("de0fac2e4500dabe0009e67214ff5f5447ce83dd".to_string()),
            comment: Some("# v6.0.2".to_string()),
            key: "uses".to_string(),
            provider: crate::core::CiProvider::GitHub,
            ..Default::default()
        };

        let res = Resolver::resolve_upgrade(
            &task,
            Arc::new(remote),
            Arc::new(registry),
            UpgradeStrategy::Latest,
        )
        .await
        .unwrap();

        assert_eq!(res, None);
    }

    #[tokio::test]
    async fn test_resolve_upgrade_pinned_equal_to_remote() {
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_latest_release()
            .returning(|_, _| Ok("v6.0.2".to_string()));

        let registry = MockRegistryProvider::new();
        let task = UpdateTask {
            path: "f.yml".into(),
            action: "actions/checkout".into(),
            current_tag: Some("de0fac2e4500dabe0009e67214ff5f5447ce83dd".to_string()),
            comment: Some("# v6.0.2".to_string()),
            key: "uses".to_string(),
            provider: crate::core::CiProvider::GitHub,
            ..Default::default()
        };

        let res = Resolver::resolve_upgrade(
            &task,
            Arc::new(remote),
            Arc::new(registry),
            UpgradeStrategy::Latest,
        )
        .await
        .unwrap();

        assert_eq!(res, None);
    }

    #[tokio::test]
    async fn test_resolve_upgrade_pinned_older_than_remote() {
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_latest_release()
            .returning(|_, _| Ok("v6.0.3".to_string()));
        remote
            .expect_get_commit_sha()
            .with(
                eq(DependencyName::from("actions/checkout")),
                eq("v6.0.3"),
                eq("uses"),
            )
            .returning(|_, _, _| Ok(DependencyRef::GitSha("new_sha".to_string())));

        let registry = MockRegistryProvider::new();
        let task = UpdateTask {
            path: "f.yml".into(),
            action: "actions/checkout".into(),
            current_tag: Some("de0fac2e4500dabe0009e67214ff5f5447ce83dd".to_string()),
            comment: Some("# v6.0.2".to_string()),
            key: "uses".to_string(),
            provider: crate::core::CiProvider::GitHub,
            ..Default::default()
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
                DependencyRef::GitSha("new_sha".to_string()),
                Some("v6.0.3".to_string())
            ))
        );
    }

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
            line: 1,
            column: 1,
            provider: crate::core::CiProvider::GitHub,
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
            line: 1,
            column: 1,
            provider: crate::core::CiProvider::GitHub,
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
            line: 1,
            column: 1,
            provider: crate::core::CiProvider::GitHub,
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
            line: 1,
            column: 1,
            provider: crate::core::CiProvider::GitHub,
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

    #[tokio::test]
    async fn test_resolve_circleci_orb() {
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_latest_release()
            .returning(|_, _| Ok("5.1.0".to_string()));
        let remote = Arc::new(remote);

        let registry = Arc::new(MockRegistryProvider::new());
        let task = UpdateTask {
            path: ".circleci/config.yml".into(),
            start: 0,
            end: 0,
            action: "circleci/node".into(),
            current_tag: Some("5.0.0".to_string()),
            comment: None,
            key: "orbs".to_string(),
            line: 1,
            column: 1,
            provider: crate::core::CiProvider::CircleCI,
        };

        // Test Pin (should skip)
        let pin_res = Resolver::resolve_pin(&task, remote.clone(), registry.clone())
            .await
            .unwrap();
        assert!(pin_res.is_none());

        // Test Upgrade
        let upgrade_res =
            Resolver::resolve_upgrade(&task, remote, registry, UpgradeStrategy::Latest)
                .await
                .unwrap();
        assert_eq!(
            upgrade_res,
            Some((DependencyRef::Version("5.1.0".to_string()), None))
        );
    }

    #[tokio::test]
    async fn test_resolver_concurrency_and_grouping() {
        let mut remote = MockRemoteProvider::new();
        // We expect only ONE call despite TWO tasks, due to grouping.
        remote
            .expect_get_commit_sha()
            .times(1)
            .returning(|_, _, _| Ok(DependencyRef::GitSha("hash".to_string())));

        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            UpgradeStrategy::Latest,
            2,
        );

        let tasks = vec![
            UpdateTask {
                path: "f1.yml".into(),
                action: "a/b".into(),
                current_tag: Some("v1".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
            UpdateTask {
                path: "f2.yml".into(),
                action: "a/b".into(),
                current_tag: Some("v1".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
        ];

        let results = resolver.resolve_tasks(tasks, true).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].new_sha.to_string(), "hash");
        assert_eq!(results[1].new_sha.to_string(), "hash");
    }

    #[tokio::test]
    async fn test_resolver_partial_failure() {
        let mut remote = MockRemoteProvider::new();
        // One success, one non-fatal failure
        remote
            .expect_get_commit_sha()
            .with(eq(DependencyName::from("success")), always(), always())
            .returning(|_, _, _| Ok(DependencyRef::GitSha("ok".to_string())));
        remote
            .expect_get_commit_sha()
            .with(eq(DependencyName::from("fail")), always(), always())
            .returning(|_, _, _| Err(PinnerError::Api("non-fatal".into())));

        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            UpgradeStrategy::Latest,
            2,
        );

        let tasks = vec![
            UpdateTask {
                action: "success".into(),
                current_tag: Some("v1".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
            UpdateTask {
                action: "fail".into(),
                current_tag: Some("v1".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
        ];

        let results = resolver.resolve_tasks(tasks, true).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].action.0, "success");
    }
}
