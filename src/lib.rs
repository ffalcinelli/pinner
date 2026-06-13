pub mod cli;
pub mod error;
pub mod github;
pub mod operations;
pub mod registry;
pub mod yaml;

pub use cli::{Cli, Commands};
pub use error::PinnerError;
pub use github::{GithubProvider, ReqwestGithubProvider};
pub use operations::Operations;
pub use registry::{OciRegistryProvider, RegistryProvider};

use std::path::PathBuf;
use std::sync::Arc;

/// Runs the Pinner CLI logic.
///
/// # Arguments
/// * `cli` - Parsed command line arguments.
/// * `github` - An implementation of [`GithubProvider`].
/// * `paths` - Paths to workflow files or directories to process.
pub async fn run<G: GithubProvider + 'static, R: RegistryProvider + 'static>(
    cli: Cli,
    github: G,
    registry: R,
    paths: Vec<PathBuf>,
) -> Result<(), PinnerError> {
    let ops = Operations::new(
        Arc::new(github),
        Arc::new(registry),
        cli.yes,
        cli.quiet,
        cli.dry_run,
        cli.json,
        cli.upgrade_strategy,
    );
    match cli.command {
        Commands::Pin => ops.pin(&paths).await,
        Commands::Upgrade => ops.upgrade(&paths).await,
        Commands::Verify => ops.verify(&paths).await,
        Commands::Set { action, hash } => ops.set(&paths, &action, &hash).await,
        Commands::GenerateCompletion { .. } => Ok(()), // Handled in main
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::UpgradeStrategy;
    use crate::github::{ActionName, CommitSha, MockGithubProvider};
    use crate::registry::{MockRegistryProvider, OciRegistryProvider};
    use ignore::WalkBuilder;
    use mockito::Server;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tree_sitter::Parser as TSParser;

    #[tokio::test]
    async fn test_all() {
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
        let _m3 = s
            .mock("GET", "/repos/o/r/commits/v2")
            .with_status(200)
            .with_body(r#"{"sha":"692973e3d937129bcbf40652eb9f2f61becf3332"}"#)
            .create_async()
            .await;

        let p = ReqwestGithubProvider::new(s.url(), None);
        assert!(p
            .get_commit_sha(&ActionName::from("o/r"), "v1")
            .await
            .is_ok());
        assert_eq!(
            p.get_latest_release(&ActionName::from("o/r"))
                .await
                .unwrap(),
            "v2"
        );

        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".into())));
        mock.expect_get_latest_release()
            .returning(|_| Ok("v2".into()));

        let dir = tempdir().unwrap();
        let wd = dir.path().join("w");
        fs::create_dir_all(&wd).unwrap();
        fs::write(wd.join("f.yml"), "uses: o/r@v1").unwrap();
        fs::write(wd.join("untagged.yml"), "uses: actions/checkout").unwrap();
        fs::write(wd.join("with_comment.yml"), "uses: o/r@v1 # keep me").unwrap();
        fs::write(
            wd.join("already_pinned.yml"),
            "uses: o/r@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1",
        )
        .unwrap();

        let mock_reg = OciRegistryProvider::new();
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            true,
            false,
            false,
            false,
            UpgradeStrategy::Latest,
        );
        ops.pin(std::slice::from_ref(&wd)).await.unwrap();

        assert!(fs::read_to_string(wd.join("f.yml"))
            .unwrap()
            .contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));

        let with_comment = fs::read_to_string(wd.join("with_comment.yml")).unwrap();
        assert!(with_comment.contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1 # keep me"));

        let already_pinned = fs::read_to_string(wd.join("already_pinned.yml")).unwrap();
        assert!(already_pinned.contains("uses: o/r@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));

        let mut mock2 = MockGithubProvider::new();
        mock2
            .expect_get_latest_release()
            .returning(|_| Ok("v3".into()));
        mock2
            .expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("692973e3d937129bcbf40652eb9f2f61becf3332".into())));
        let mock_reg = OciRegistryProvider::new();
        let ops = Operations::new(
            Arc::new(mock2),
            Arc::new(mock_reg),
            true,
            false,
            false,
            false,
            UpgradeStrategy::Latest,
        );

        ops.upgrade(std::slice::from_ref(&wd)).await.unwrap();
        let ut = fs::read_to_string(wd.join("untagged.yml")).unwrap();
        assert!(ut.contains("actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332 # v3"));

        let mut mock3 = MockGithubProvider::new();
        mock3
            .expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("s".into())));
        run(
            Cli {
                command: Commands::Pin,
                workflows: vec![],
                yes: true,
                quiet: true,
                verbose: false,
                dry_run: false,
                token: None,
                json: false,
                github_url: None,
                upgrade_strategy: UpgradeStrategy::Latest,
            },
            mock3,
            OciRegistryProvider::new(),
            vec![wd.clone()],
        )
        .await
        .unwrap();
        assert!(run(
            Cli {
                command: Commands::Pin,
                workflows: vec![],
                yes: true,
                quiet: true,
                verbose: false,
                dry_run: false,
                token: None,
                json: false,
                github_url: None,
                upgrade_strategy: UpgradeStrategy::Latest,
            },
            MockGithubProvider::new(),
            OciRegistryProvider::new(),
            vec![PathBuf::from("/n")]
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn test_operations_json() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("newhash".into())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mock_reg = OciRegistryProvider::new();
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            true,
            false,
            false,
            true,
            UpgradeStrategy::Latest,
        );

        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("newhash"));
    }

    #[tokio::test]
    async fn test_github_provider_errors() {
        let mut s = Server::new_async().await;
        let p = ReqwestGithubProvider::new(s.url(), None);

        let _m = s
            .mock("GET", "/repos/o/r/commits/v1")
            .with_status(404)
            .create_async()
            .await;
        assert!(p
            .get_commit_sha(&ActionName::from("o/r"), "v1")
            .await
            .is_err());

        let _m_500 = s
            .mock("GET", "/repos/o/r/commits/v500")
            .with_status(500)
            .create_async()
            .await;
        assert!(p
            .get_commit_sha(&ActionName::from("o/r"), "v500")
            .await
            .is_err());

        let p_bad_url = ReqwestGithubProvider::new("http://127.0.0.1:0".to_string(), None);
        assert!(p_bad_url
            .get_commit_sha(&ActionName::from("o/r"), "v1")
            .await
            .is_err());

        let _m2 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(500)
            .create_async()
            .await;
        assert!(p
            .get_latest_release(&ActionName::from("o/r"))
            .await
            .is_err());

        let _m3 = s
            .mock("GET", "/repos/o/r/releases/latest")
            .with_status(404)
            .create_async()
            .await;
        assert_eq!(
            p.get_latest_release(&ActionName::from("o/r"))
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
            p.get_latest_release(&ActionName::from("o/r2"))
                .await
                .unwrap(),
            "develop"
        );
    }

    #[tokio::test]
    async fn test_operations_set() {
        let mock = MockGithubProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mock_reg = OciRegistryProvider::new();
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            true,
            true,
            false,
            false,
            UpgradeStrategy::Latest,
        );
        ops.set(std::slice::from_ref(&f), "o/r", "newhash")
            .await
            .unwrap();

        assert!(fs::read_to_string(&f).unwrap().contains("o/r@newhash"));
    }

    #[tokio::test]
    async fn test_operations_dry_run() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("newhash".into())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mock_reg = OciRegistryProvider::new();
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            true,
            false,
            true,
            false,
            UpgradeStrategy::Latest,
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
        let mut results = Vec::new();
        crate::yaml::find_uses_nodes(tree.root_node(), content.as_bytes(), &mut results);

        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .any(|(_, _, v, _)| v == "actions/checkout@v3"));
        assert!(results.iter().any(|(_, _, v, _)| v == "owner/repo@v1"));
    }

    #[tokio::test]
    async fn test_yaml_comment_capture() {
        let content = "uses: o/r@v1 # comment";
        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let tree = parser.parse(content, None).unwrap();
        let mut results = Vec::new();
        crate::yaml::find_uses_nodes(tree.root_node(), content.as_bytes(), &mut results);
        assert!(results[0].3.is_some());
    }

    #[tokio::test]
    async fn test_run_subcommands() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("h".into())));
        mock.expect_get_latest_release()
            .returning(|_| Ok("v2".into()));

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
            token: None,
            json: false,
            github_url: None,
            upgrade_strategy: UpgradeStrategy::Latest,
        };
        run(cli, mock, OciRegistryProvider::new(), vec![f.clone()])
            .await
            .unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("o/r@h # v2"));
    }

    #[tokio::test]
    async fn test_docker_pinning() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: docker://alpine:3.18").unwrap();

        let mock = MockGithubProvider::new();
        let mut mock_reg = MockRegistryProvider::new();
        mock_reg
            .expect_resolve_digest()
            .returning(|_, _| Ok("sha256:digest".into()));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg),
            true,
            true,
            false,
            false,
            UpgradeStrategy::Latest,
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

        let mut mock = MockGithubProvider::new();
        mock.expect_list_tags().returning(|_| {
            Ok(vec![
                "v1.1.0".into(),
                "v1.1.1".into(),
                "v1.2.0".into(),
                "v2.0.0".into(),
            ])
        });
        mock.expect_get_commit_sha()
            .returning(|_, tag| Ok(CommitSha(format!("hash-{}", tag))));

        let mock_reg = OciRegistryProvider::new();

        // Minor strategy
        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(mock_reg.clone()),
            true,
            true,
            false,
            false,
            UpgradeStrategy::Minor,
        );
        ops.upgrade(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("v1.1.1"));

        // Major strategy
        let mut mock2 = MockGithubProvider::new();
        mock2
            .expect_list_tags()
            .returning(|_| Ok(vec!["v1.1.0".into(), "v1.2.0".into(), "v2.0.0".into()]));
        mock2
            .expect_get_commit_sha()
            .returning(|_, tag| Ok(CommitSha(format!("hash-{}", tag))));
        let ops2 = Operations::new(
            Arc::new(mock2),
            Arc::new(mock_reg.clone()),
            true,
            true,
            false,
            false,
            UpgradeStrategy::Major,
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

        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("h".into())));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new()),
            false,
            false,
            true,
            false,
            UpgradeStrategy::Latest,
        );
        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        let mut mock2 = MockGithubProvider::new();
        mock2
            .expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("h".into())));
        let ops2 = Operations::new(
            Arc::new(mock2),
            Arc::new(OciRegistryProvider::new()),
            true,
            true,
            false,
            true,
            UpgradeStrategy::Latest,
        );
        ops2.pin(std::slice::from_ref(&f)).await.unwrap();
    }

    #[tokio::test]
    async fn test_operations_interactive_accept() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("h".into())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mut ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new()),
            false,
            false,
            false,
            false,
            UpgradeStrategy::Latest,
        );
        ops.force_confirm = Some(true);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("h"));
    }

    #[tokio::test]
    async fn test_operations_interactive_skip() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("h".into())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let mut ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new()),
            false,
            false,
            false,
            false,
            UpgradeStrategy::Latest,
        );
        ops.force_confirm = Some(false);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("@v1"));
    }

    #[tokio::test]
    async fn test_verify_fail_multiple() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r1@v1\nuses: o/r2@v2").unwrap();

        let ops = Operations::new(
            Arc::new(MockGithubProvider::new()),
            Arc::new(OciRegistryProvider::new()),
            true,
            false,
            false,
            false,
            UpgradeStrategy::Latest,
        );
        assert!(ops.verify(std::slice::from_ref(&f)).await.is_err());
    }

    #[tokio::test]
    async fn test_skip_local_actions() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: ./local\nuses: o/r@v1").unwrap();

        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("h".into())));

        let ops = Operations::new(
            Arc::new(mock),
            Arc::new(OciRegistryProvider::new()),
            true,
            true,
            false,
            false,
            UpgradeStrategy::Latest,
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
            Operations::<MockGithubProvider, OciRegistryProvider>::load_config_from_path(&f)
                .unwrap();
        assert_eq!(config.concurrency, 42);
    }

    #[tokio::test]
    async fn test_operations_print_diffs() {
        let ops = Operations::new(
            Arc::new(MockGithubProvider::new()),
            Arc::new(OciRegistryProvider::new()),
            true,
            false,
            false,
            false,
            UpgradeStrategy::Latest,
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
            action: ActionName::from("o/r"),
            current_tag: Some("v1".into()),
            comment: None,
        };
        let res = UpdateResult {
            task,
            action: ActionName::from("o/r"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".into()),
            new_sha: CommitSha("h".into()),
            new_tag: Some("v1".into()),
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
            Arc::new(MockGithubProvider::new()),
            Arc::new(OciRegistryProvider::new()),
            true,
            true,
            false,
            false,
            UpgradeStrategy::Latest,
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
}
