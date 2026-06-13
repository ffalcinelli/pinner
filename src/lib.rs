pub mod cli;
pub mod error;
pub mod github;
pub mod operations;
pub mod yaml;

pub use cli::{Cli, Commands};
pub use error::PinnerError;
pub use github::{GithubProvider, ReqwestGithubProvider};
pub use operations::Operations;

use std::path::PathBuf;
use std::sync::Arc;

/// Runs the Pinner CLI logic.
///
/// # Arguments
/// * `cli` - Parsed command line arguments.
/// * `github` - An implementation of [`GithubProvider`].
/// * `paths` - Paths to workflow files or directories to process.
pub async fn run<G: GithubProvider + 'static>(
    cli: Cli,
    github: G,
    paths: Vec<PathBuf>,
) -> Result<(), PinnerError> {
    let ops = Operations::new(Arc::new(github), cli.yes, cli.quiet, cli.dry_run, cli.json);
    match cli.command {
        Commands::Pin => ops.pin(&paths).await,
        Commands::Upgrade => ops.upgrade(&paths).await,
        Commands::Set { action, hash } => ops.set(&paths, &action, &hash).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{ActionName, CommitSha, MockGithubProvider};
    use crate::operations::Config;
    use mockito::Server;
    use std::fs;
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

        let ops = Operations::new(Arc::new(mock), true, false, false, false);
        ops.pin(std::slice::from_ref(&wd)).await.unwrap();

        assert!(fs::read_to_string(wd.join("f.yml"))
            .unwrap()
            .contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));

        let with_comment = fs::read_to_string(wd.join("with_comment.yml")).unwrap();
        assert!(with_comment.contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1 # keep me"));

        let already_pinned = fs::read_to_string(wd.join("already_pinned.yml")).unwrap();
        assert!(already_pinned.contains("uses: o/r@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));
        // Check that it didn't add double comments or mess up
        assert!(!already_pinned.contains("# v1 # v1"));

        let mut mock2 = MockGithubProvider::new();
        mock2
            .expect_get_latest_release()
            .returning(|_| Ok("v3".into()));
        mock2
            .expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("692973e3d937129bcbf40652eb9f2f61becf3332".into())));
        let ops2 = Operations::new(Arc::new(mock2), true, false, false, false);
        ops2.upgrade(std::slice::from_ref(&wd)).await.unwrap();
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
                dry_run: false,
                token: None,
                json: false,
            },
            mock3,
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
                dry_run: false,
                token: None,
                json: false,
            },
            MockGithubProvider::new(),
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

        let ops = Operations::new(Arc::new(mock), true, false, false, true);
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

        let _m_bad_json = s
            .mock("GET", "/repos/o/r/commits/vbad")
            .with_status(200)
            .with_body("not valid json")
            .create_async()
            .await;
        assert!(p
            .get_commit_sha(&ActionName::from("o/r"), "vbad")
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

        let ops = Operations::new(Arc::new(mock), true, true, false, false);
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

        let ops = Operations::new(Arc::new(mock), true, false, true, false);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        assert_eq!(fs::read_to_string(&f).unwrap(), "uses: o/r@v1");
    }

    #[tokio::test]
    async fn test_operations_quiet() {
        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok(CommitSha("newhash".into())));
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: o/r@v1").unwrap();

        let ops = Operations::new(Arc::new(mock), true, true, false, false);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();

        assert!(fs::read_to_string(&f).unwrap().contains("newhash"));
    }

    #[tokio::test]
    async fn test_find_uses_nodes_nested() {
        let _mock = MockGithubProvider::new();
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
        assert!(results.iter().any(|(_, _, v)| v == "actions/checkout@v3"));
        assert!(results.iter().any(|(_, _, v)| v == "owner/repo@v1"));
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

        let cli_upgrade = Cli {
            command: Commands::Upgrade,
            workflows: vec![],
            yes: true,
            quiet: true,
            dry_run: false,
            token: None,
            json: false,
        };
        run(cli_upgrade, mock, vec![f.clone()]).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("o/r@h # v2"));

        let mock2 = MockGithubProvider::new();
        let cli_set = Cli {
            command: Commands::Set {
                action: "o/r".into(),
                hash: "sethash".into(),
            },
            workflows: vec![],
            yes: true,
            quiet: true,
            dry_run: false,
            token: None,
            json: false,
        };
        run(cli_set, mock2, vec![f.clone()]).await.unwrap();
        assert!(fs::read_to_string(&f).unwrap().contains("o/r@sethash"));
    }

    #[tokio::test]
    async fn test_error_path_not_found() {
        let mock = MockGithubProvider::new();
        let ops = Operations::new(Arc::new(mock), true, true, false, false);
        let res = ops.pin(&[PathBuf::from("/non/existent/path")]).await;
        assert!(matches!(res, Err(PinnerError::PathNotFound(_))));
    }

    #[tokio::test]
    async fn test_operations_config_ignore() {
        let _mock = MockGithubProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: ignore/me@v1\nuses: keep/me@v1").unwrap();

        let mut config = Config::default();
        config.ignore_actions.insert(ActionName::from("ignore/me"));

        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let content = fs::read_to_string(&f).unwrap();
        let tree = parser.parse(&content, None).unwrap();
        let mut results = Vec::new();
        crate::yaml::find_uses_nodes(tree.root_node(), content.as_bytes(), &mut results);

        assert_eq!(results.len(), 2);

        let config_file = dir.path().join(".pinner.toml");
        fs::write(&config_file, "ignore_actions = [\"a\", \"b\"]").unwrap();

        let loaded =
            Operations::<ReqwestGithubProvider>::load_config_from_path(&config_file).unwrap();

        assert!(loaded.ignore_actions.contains(&ActionName::from("a")));
        assert!(loaded.ignore_actions.contains(&ActionName::from("b")));
    }

    #[test]
    fn test_load_config_invalid_toml() {
        let dir = tempdir().unwrap();
        let config_file = dir.path().join(".pinner.toml");
        fs::write(&config_file, "ignore_actions = [\"a\", \"b\"").unwrap();

        let result = Operations::<ReqwestGithubProvider>::load_config_from_path(&config_file);

        assert!(result.is_err());
        match result.unwrap_err() {
            PinnerError::Config(msg) => {
                assert!(msg.contains("Failed to parse"));
            }
            _ => panic!("Expected PinnerError::Config"),
        }
    }

    #[test]
    fn test_find_uses_nodes_empty() {
        let mut results = Vec::new();
        let content = "";
        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let tree = parser.parse(content, None).unwrap();
        crate::yaml::find_uses_nodes(tree.root_node(), content.as_bytes(), &mut results);
        assert!(results.is_empty());
    }

    #[test]
    fn test_find_uses_nodes_malformed() {
        let mut results = Vec::new();
        let content = "invalid: : yaml: -";
        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let tree = parser.parse(content, None).unwrap();
        crate::yaml::find_uses_nodes(tree.root_node(), content.as_bytes(), &mut results);
        // It shouldn't panic, and results should probably be empty or just what it could parse
        assert!(results.is_empty());
    }

    #[test]
    fn test_find_uses_nodes_no_uses() {
        let mut results = Vec::new();
        let content = "foo: bar\nbaz: qux";
        let mut parser = TSParser::new();
        parser.set_language(tree_sitter_yaml::language()).unwrap();
        let tree = parser.parse(content, None).unwrap();
        crate::yaml::find_uses_nodes(tree.root_node(), content.as_bytes(), &mut results);
        assert!(results.is_empty());
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.concurrency, 10);
        assert!(config.ignore_actions.is_empty());
    }

    #[test]
    fn test_config_deserialization_minimal() {
        let content = "concurrency = 5";
        let config: Config = toml::from_str(content).unwrap();
        assert_eq!(config.concurrency, 5);
        assert!(config.ignore_actions.is_empty());
    }

    #[tokio::test]
    async fn test_operations_idempotency() {
        let mock = MockGithubProvider::new();
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        let hash = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        fs::write(&f, format!("uses: o/r@{} # v1", hash)).unwrap();

        let ops = Operations::new(Arc::new(mock), true, true, false, false);
        ops.pin(std::slice::from_ref(&f)).await.unwrap();
        // Should not change anything
        assert_eq!(
            fs::read_to_string(&f).unwrap(),
            format!("uses: o/r@{} # v1", hash)
        );
    }
}
