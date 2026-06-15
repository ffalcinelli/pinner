//! # Pinner
//!
//! `pinner` is a high-performance Rust library and CLI utility designed to hash-pin
//! dependencies in CI/CD workflow files. It currently supports GitHub Actions,
//! GitLab CI, Bitbucket Pipelines, and Docker images.
//!
//! The core logic resides in the [`Operations`] struct, which coordinates between
//! repository providers (like GitHub) and YAML parsing logic to perform surgical
//! replacements of mutable tags with immutable commit SHAs or digests.

pub mod cli;
pub mod error;
pub mod operations;
pub mod providers;
pub mod registry;
pub mod yaml;

pub use cli::{Cli, Commands};
pub use error::PinnerError;
pub use operations::{Operations, OperationsOptions};
pub use providers::{RemoteProvider, ReqwestGithubProvider};
pub use registry::{OciRegistryProvider, RegistryProvider};

use colored::Colorize;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

/// Runs the Pinner CLI logic based on the provided configuration.
///
/// This is the main entry point for the CLI application. It initializes the
/// necessary providers and delegates the command execution to [`Operations`].
///
/// # Errors
///
/// Returns a [`PinnerError`] if any operation fails, such as network issues,
/// file system errors, or parsing failures.
pub async fn run<G: RemoteProvider + 'static, R: RegistryProvider + 'static>(
    cli: Cli,
    github: G,
    registry: R,
    paths: Vec<PathBuf>,
) -> Result<(), PinnerError> {
    let ops = Operations::new(
        Arc::new(github),
        Arc::new(registry),
        OperationsOptions {
            yes: cli.yes,
            quiet: cli.quiet,
            dry_run: cli.dry_run,
            format: cli.output_format(),
            upgrade_strategy: cli.upgrade_strategy,
            concurrency: cli.concurrency,
            ignore: cli.ignore,
        },
    );
    match cli.command {
        Commands::Pin => ops.pin(&paths).await,
        Commands::Upgrade => ops.upgrade(&paths).await,
        Commands::Verify => ops.verify(&paths).await,
        Commands::Set { action, hash } => ops.set(&paths, &action, &hash).await,
        Commands::InstallHook => install_git_hook(),
        Commands::GenerateCompletion { .. } => Ok(()), // Handled in main
    }
}

/// Installs a pre-commit git hook that runs `pinner verify`.
///
/// This function creates a `.git/hooks/pre-commit` file that executes the verify
/// command before each commit, helping ensure all actions remain pinned.
pub fn install_git_hook() -> Result<(), PinnerError> {
    let git_dir = PathBuf::from(".git");
    if !git_dir.exists() {
        return Err(PinnerError::Config(
            "Not a git repository (no .git directory found)".into(),
        ));
    }

    let hooks_dir = git_dir.join("hooks");
    if !hooks_dir.exists() {
        fs::create_dir_all(&hooks_dir)?;
    }

    let hook_path = hooks_dir.join("pre-commit");

    let hook_content = r#"#!/bin/sh
# Pinner pre-commit hook: Verify that all actions are pinned to a SHA.
pinner verify --quiet
"#;

    fs::write(&hook_path, hook_content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms)?;
    }

    println!(
        "{} Git pre-commit hook installed successfully at {}",
        "✔".green().bold(),
        hook_path.display().to_string().cyan()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::UpgradeStrategy;
    use crate::providers::{DependencyName, DependencyRef, MockRemoteProvider};
    use crate::registry::{MockRegistryProvider, OciRegistryProvider};
    use ignore::WalkBuilder;
    use mockito::Server;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tree_sitter::Parser as TSParser;

    #[tokio::test]
    async fn test_reqwest_github_provider() {
        let mut s = Server::new_async().await;
        let _m = s
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(200)
            .with_body(r#"{"sha":"a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"}"#)
            .create_async()
            .await;
        let _m2 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(200)
            .with_body(r#"{"tag_name":"v2"}"#)
            .create_async()
            .await;

        let p = ReqwestGithubProvider::new(s.url(), None).unwrap();
        assert!(p
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await
            .is_ok());
        assert_eq!(
            p.get_latest_release(&DependencyName::from("o/r"), "uses")
                .await
                .unwrap(),
            "v2"
        );
    }

    #[tokio::test]
    async fn test_operations_pin() {
        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha().returning(|_, _, _| {
            Ok(DependencyRef::from(
                "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string(),
            ))
        });
        mock.expect_get_latest_release()
            .returning(|_, _| Ok("v2".to_string()));

        let dir = tempdir().unwrap();
        let wd = dir.path().join("w");
        fs::create_dir_all(&wd).unwrap();
        fs::write(wd.join("f.yml"), "uses: o/r@v1").unwrap();
        fs::write(wd.join("with_comment.yml"), "uses: o/r@v1 # keep me").unwrap();
        fs::write(
            wd.join("already_pinned.yml"),
            "uses: o/r@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1",
        )
        .unwrap();

        let mock_reg = OciRegistryProvider::new(None, None);
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: false,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.pin(std::slice::from_ref(&wd)).await.unwrap();

        assert!(fs::read_to_string(wd.join("f.yml"))
            .unwrap()
            .contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));

        let with_comment = fs::read_to_string(wd.join("with_comment.yml")).unwrap();
        assert!(with_comment.contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1 # keep me"));

        let already_pinned = fs::read_to_string(wd.join("already_pinned.yml")).unwrap();
        assert!(already_pinned.contains("uses: o/r@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));
    }

    #[tokio::test]
    async fn test_operations_upgrade() {
        let dir = tempdir().unwrap();
        let wd = dir.path().join("w");
        fs::create_dir_all(&wd).unwrap();
        fs::write(wd.join("untagged.yml"), "uses: actions/checkout").unwrap();

        let mut mock2 = MockRemoteProvider::new();
        mock2
            .expect_get_latest_release()
            .returning(|_, _| Ok("v3".to_string()));
        mock2.expect_get_commit_sha().returning(|_, _, _| {
            Ok(DependencyRef::from(
                "692973e3d937129bcbf40652eb9f2f61becf3332".to_string(),
            ))
        });
        let mock_reg = OciRegistryProvider::new(None, None);
        let ops = Operations::new(
            Arc::new(mock2),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: false,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.upgrade(std::slice::from_ref(&wd)).await.unwrap();
        let ut = fs::read_to_string(wd.join("untagged.yml")).unwrap();
        assert!(ut.contains("actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332 # v3"));
    }

    #[tokio::test]
    async fn test_run_pin_success() {
        let dir = tempdir().unwrap();
        let wd = dir.path().join("w");
        fs::create_dir_all(&wd).unwrap();
        fs::write(wd.join("f.yml"), "uses: o/r@v1").unwrap();

        let mut mock3 = MockRemoteProvider::new();
        mock3
            .expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("s".to_string())));
        run(
            Cli {
                command: Commands::Pin,
                workflows: vec![],
                yes: true,
                quiet: true,
                verbose: false,
                dry_run: false,
                github_token: None,
                bitbucket_token: None,
                gitlab_token: None,
                forgejo_token: None,
                format: crate::cli::OutputFormat::Text,
                json: false,
                github_url: "https://api.github.com".to_string(),
                bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
                gitlab_url: "https://gitlab.com".to_string(),
                forgejo_url: "https://codeberg.org".to_string(),
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
                oci_username: None,
                oci_password: None,
            },
            mock3,
            OciRegistryProvider::new(None, None),
            vec![wd.clone()],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_run_pin_failure() {
        assert!(run(
            Cli {
                command: Commands::Pin,
                workflows: vec![],
                yes: true,
                quiet: true,
                verbose: false,
                dry_run: false,
                github_token: None,
                bitbucket_token: None,
                gitlab_token: None,
                forgejo_token: None,
                format: crate::cli::OutputFormat::Text,
                json: false,
                github_url: "https://api.github.com".to_string(),
                bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
                gitlab_url: "https://gitlab.com".to_string(),
                forgejo_url: "https://codeberg.org".to_string(),
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
                oci_username: None,
                oci_password: None,
            },
            MockRemoteProvider::new(),
            OciRegistryProvider::new(None, None),
            vec![PathBuf::from("/n")]
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn test_operations_json() {
        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("newhash".to_string())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mock_reg = OciRegistryProvider::new(None, None);
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: false,
                dry_run: false,
                format: crate::cli::OutputFormat::Json,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("newhash"));
    }

    #[tokio::test]
    async fn test_github_provider_errors() {
        let mut s = Server::new_async().await;
        let p = ReqwestGithubProvider::new(s.url(), None).unwrap();

        let _m = s
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(404)
            .create_async()
            .await;
        assert!(p
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await
            .is_err());

        let _m_500 = s
            .mock("GET", "/repos/o/r/commits/v500")
            .with_status(500)
            .create_async()
            .await;
        assert!(p
            .get_commit_sha(&DependencyName::from("o/r"), "v500", "uses")
            .await
            .is_err());

        let p_bad_url = ReqwestGithubProvider::new("http://127.0.0.1:0".to_string(), None).unwrap();
        assert!(p_bad_url
            .get_commit_sha(&DependencyName::from("o/r"), "v1", "uses")
            .await
            .is_err());

        let _m2 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(500)
            .create_async()
            .await;
        assert!(p
            .get_latest_release(&DependencyName::from("o/r"), "uses")
            .await
            .is_err());

        let _m3 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(404)
            .create_async()
            .await;
        assert_eq!(
            p.get_latest_release(&DependencyName::from("o/r"), "uses")
                .await
                .unwrap(),
            "main"
        );

        let _m4 = s
            .mock("GET", "/repos/o/r2/releases/latest")
            .with_status(404)
            .create_async()
            .await;
        let _m5 = s
            .mock("GET", "/repos/o/r2")
            .with_status(200)
            .with_body(r#"{"default_branch":"develop"}"#)
            .create_async()
            .await;
        assert_eq!(
            p.get_latest_release(&DependencyName::from("o/r2"), "uses")
                .await
                .unwrap(),
            "develop"
        );
    }

    #[tokio::test]
    async fn test_operations_set() {
        let mock = MockRemoteProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mock_reg = OciRegistryProvider::new(None, None);
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.set(std::slice::from_ref(&f), "o/r", "newhash")
            .await
            .unwrap();

        assert!(fs::read_to_string(&f).unwrap().contains("o/r@newhash"));
    }

    #[tokio::test]
    async fn test_operations_dry_run() {
        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("newhash".to_string())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mock_reg = OciRegistryProvider::new(None, None);
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: false,
                dry_run: true,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert_eq!(fs::read_to_string(&f).unwrap(), "uses: o/r@v1");
    }

    #[tokio::test]
    async fn test_find_uses_nodes_nested() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(
            &f,
            "
jobs:
  test:
    steps:
      - name: Checkout
        uses: actions/checkout@v3
      - name: Custom
        uses: 
          owner/repo@v1
",
        )
        .unwrap();

        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let content = fs::read_to_string(&f).unwrap();
        let tree = parser.parse(&content, None).unwrap();
        let results = crate::yaml::find_uses_nodes(
            tree.root_node(),
            content.as_bytes(),
            crate::yaml::CiProvider::GitHub,
        )
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .any(|r| r.value == "actions/checkout@v3" && r.key == "uses"));
        assert!(results
            .iter()
            .any(|r| r.value == "owner/repo@v1" && r.key == "uses"));
    }

    #[tokio::test]
    async fn test_yaml_comment_capture() {
        let content = "uses: o/r@v1 # comment";
        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let tree = parser.parse(content, None).unwrap();
        let results = crate::yaml::find_uses_nodes(
            tree.root_node(),
            content.as_bytes(),
            crate::yaml::CiProvider::GitHub,
        )
        .unwrap();
        assert!(results[0].comment.is_some());
        assert_eq!(results[0].key, "uses");
    }

    #[tokio::test]
    async fn test_run_subcommands() {
        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("h".to_string())));
        mock.expect_get_latest_release()
            .returning(|_, _| Ok("v2".to_string()));

        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let cli = Cli {
            command: Commands::Upgrade,
            workflows: vec![],
            yes: true,
            quiet: true,
            verbose: false,
            dry_run: false,
            github_token: None,
            bitbucket_token: None,
            gitlab_token: None,
            forgejo_token: None,
            format: crate::cli::OutputFormat::Text,
            json: false,
            github_url: "https://api.github.com".to_string(),
            bitbucket_url: "https://api.bitbucket.org/2.0".to_string(),
            gitlab_url: "https://gitlab.com".to_string(),
            forgejo_url: "https://codeberg.org".to_string(),
            upgrade_strategy: UpgradeStrategy::Latest,
            concurrency: None,
            ignore: vec![],
            oci_username: None,
            oci_password: None,
        };
        run(
            cli,
            mock,
            OciRegistryProvider::new(None, None),
            vec![f.clone()],
        )
        .await
        .unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("o/r@h # v2"));
    }

    #[tokio::test]
    async fn test_docker_pinning() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: docker://alpine:3.18").unwrap();

        let mock = MockRemoteProvider::new();
        let mut mock_reg = MockRegistryProvider::new();
        mock_reg
            .expect_resolve_digest()
            .returning(|_, _| Ok("sha256:digest".to_string()));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f)
            .unwrap()
            .contains("docker://alpine@sha256:digest # 3.18"));
    }

    #[tokio::test]
    async fn test_semver_upgrades_exhaustive() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1.1.0").unwrap();

        let mut mock = MockRemoteProvider::new();
        mock.expect_list_tags().returning(|_, _| {
            Ok(vec![
                "v1.1.0".into(),
                "v1.1.1".into(),
                "v1.2.0".into(),
                "v2.0.0".into(),
            ])
        });
        mock.expect_get_commit_sha()
            .returning(|_, tag, _| Ok(DependencyRef::from(format!("hash-{}", tag))));

        let mock_reg = OciRegistryProvider::new(None, None);

        // Minor strategy
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg.clone()),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Minor,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.upgrade(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("v1.1.1"));

        // Major strategy
        let mut mock2 = MockRemoteProvider::new();
        mock2
            .expect_list_tags()
            .returning(|_, _| Ok(vec!["v1.1.0".into(), "v1.2.0".into(), "v2.0.0".into()]));
        mock2
            .expect_get_commit_sha()
            .returning(|_, tag, _| Ok(DependencyRef::from(format!("hash-{}", tag))));
        let ops2 = Operations::new(
            Arc::new(mock2),
            Arc::new(mock_reg.clone()),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Major,
                concurrency: None,
                ignore: vec![],
            },
        );
        fs::write(&f, "uses: o/r@v1.1.0").unwrap();
        ops2.upgrade(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("v1.2.0"));
    }

    #[tokio::test]
    async fn test_operations_exhaustive_coverage() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1 # v1").unwrap();

        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("h".to_string())));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: false,
                quiet: false,
                dry_run: true,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        let mut mock2 = MockRemoteProvider::new();
        mock2
            .expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("h".to_string())));
        let ops2 = Operations::new(
            Arc::new(mock2),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Json,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops2.pin(std::slice::from_ref(&f)).await.unwrap();
    }

    #[tokio::test]
    async fn test_operations_interactive_accept() {
        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("h".to_string())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mut ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: false,
                quiet: false,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.force_confirm = Some(true);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("h"));
    }

    #[tokio::test]
    async fn test_operations_interactive_skip() {
        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("h".to_string())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mut ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: false,
                quiet: false,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.force_confirm = Some(false);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("@v1"));
    }

    #[tokio::test]
    async fn test_gitlab_pinning() {
        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .with(
                mockall::predicate::eq(DependencyName::from("my-group/my-project")),
                mockall::predicate::eq("v1.0.0"),
                mockall::predicate::eq("ref"),
            )
            .returning(|_, _, _| Ok(DependencyRef::from("gitlabsha".to_string())));

        let dir = tempdir().unwrap();
        let f = dir.path().join(".gitlab-ci.yml");
        fs::write(
            &f,
            r#"
include:
  - project: 'my-group/my-project'
    ref: 'v1.0.0'
    file: 'template.yml'
"#,
        )
        .unwrap();

        let mock_reg = OciRegistryProvider::new(None, None);
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        let updated = fs::read_to_string(&f).unwrap();
        println!("Updated content: {}", updated);
        assert!(updated.contains("ref: gitlabsha # v1.0.0"));
    }
    #[tokio::test]
    async fn test_bitbucket_pinning() {
        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .with(
                mockall::predicate::eq(DependencyName::from("atlassian/slack-notify")),
                mockall::predicate::eq("2.1.0"),
                mockall::predicate::eq("pipe"),
            )
            .returning(|_, _, _| Ok(DependencyRef::from("pipehash".to_string())));

        let mut mock_reg = MockRegistryProvider::new();
        mock_reg
            .expect_resolve_digest()
            .with(mockall::predicate::eq("node"), mockall::predicate::eq("20"))
            .returning(|_, _| Ok("imghash".to_string()));

        let dir = tempdir().unwrap();
        let f = dir.path().join("bitbucket-pipelines.yml");
        fs::write(
            &f,
            "
image: node:20
pipelines:
  default:
    - step:
        script:
          - pipe: atlassian/slack-notify:2.1.0
",
        )
        .unwrap();

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );

        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        let content = fs::read_to_string(&f).unwrap();
        assert!(content.contains("pipe: atlassian/slack-notify:pipehash # 2.1.0"));
        assert!(content.contains("image: node@imghash # 20"));
    }

    #[tokio::test]
    async fn test_verify_fail_multiple() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r1@v1\nuses: o/r2@v2").unwrap();

        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: false,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        assert!(ops.verify(std::slice::from_ref(&f)).await.is_err());
    }

    #[tokio::test]
    async fn test_skip_local_actions() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: ./local\nuses: o/r@v1").unwrap();

        let mut mock = MockRemoteProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _, _| Ok(DependencyRef::from("h".to_string())));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        let c = fs::read_to_string(&f).unwrap();
        assert!(c.contains("./local"));
        assert!(c.contains("o/r@h"));
    }

    #[test]
    fn test_config_load_existing() {
        let dir = tempdir().unwrap();
        let f = dir.path().join(".pinner.toml");
        fs::write(&f, "concurrency = 42").unwrap();
        let config =
            Operations::<MockRemoteProvider, OciRegistryProvider>::load_config_from_path(&f)
                .unwrap();
        assert_eq!(config.concurrency, 42);
    }

    #[tokio::test]
    async fn test_operations_print_diffs() {
        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: false,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.print_diff("old\n", "new\n");
        ops.print_inline_diff("old", "new");
    }

    #[test]
    fn test_json_output_serialization() {
        use crate::operations::{JsonOutput, UpdateResult, UpdateTask};
        let task = UpdateTask {
            path: PathBuf::from("f.yml"),
            start: 0,
            end: 10,
            action: DependencyName::from("o/r"),
            current_tag: Some("v1".to_string()),
            comment: None,
            key: "uses".to_string(),
        };
        let res = UpdateResult {
            task,
            action: DependencyName::from("o/r"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            new_sha: DependencyRef::from("h".to_string()),
            new_tag: Some("v1".to_string()),
        };
        let output = JsonOutput { updates: vec![res] };
        assert!(serde_json::to_string(&output).is_ok());
    }

    #[tokio::test]
    async fn test_operations_idempotency() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(
            &f,
            "uses: o/r@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1",
        )
        .unwrap();
        let ops = Operations::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(OciRegistryProvider::new(None, None)),
            OperationsOptions {
                yes: true,
                quiet: true,
                dry_run: false,
                format: crate::cli::OutputFormat::Text,
                upgrade_strategy: UpgradeStrategy::Latest,
                concurrency: None,
                ignore: vec![],
            },
        );
        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("# v1"));
    }

    #[tokio::test]
    async fn test_error_conversions() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "err");
        assert!(format!("{}", PinnerError::from(io_err)).contains("err"));
        let ignore_err = WalkBuilder::new("/non/existent")
            .build()
            .next()
            .unwrap()
            .unwrap_err();
        assert!(format!("{}", PinnerError::from(ignore_err)).contains("non/existent"));
    }

    #[test]
    fn test_install_git_hook_no_git_dir() {
        let dir = tempdir().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        let result = install_git_hook();
        assert!(result.is_err());

        if let Err(PinnerError::Config(msg)) = result {
            assert_eq!(msg, "Not a git repository (no .git directory found)");
        } else {
            panic!("Expected Config error, got {:?}", result);
        }

        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    fn test_install_git_hook() {
        let dir = tempdir().unwrap();
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        // Should fail if .git doesn't exist
        assert!(install_git_hook().is_err());

        // Setup mock .git
        fs::create_dir(".git").unwrap();
        assert!(install_git_hook().is_ok());

        let hook_path = PathBuf::from(".git/hooks/pre-commit");
        assert!(hook_path.exists());
        let content = fs::read_to_string(hook_path).unwrap();
        assert!(content.contains("pinner verify"));

        std::env::set_current_dir(original_dir).unwrap();
    }
}
