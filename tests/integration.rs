use clap::Parser;
use mockito::Server;
use pinner::providers::{UnifiedProvider, UnifiedProviderConfig};
use pinner::{run, Cli, OciRegistryProvider};
use std::fs;
use tempfile::tempdir;

#[tokio::test]
async fn test_full_pin_cycle() {
    let mut github_server = Server::new_async().await;
    let _m1 = github_server
        .mock("GET", "/repos/actions/checkout/commits/v3")
        .with_status(200)
        .with_body(r#"{"sha":"hashv3"}"#)
        .create_async()
        .await;

    let dir = tempdir().unwrap();
    let workflows = dir.path().join(".github/workflows");
    fs::create_dir_all(&workflows).unwrap();
    let wf_path = workflows.join("ci.yml");
    fs::write(&wf_path, "uses: actions/checkout@v3").unwrap();

    let cli = Cli::try_parse_from([
        "pinner",
        "--github-url",
        &github_server.url(),
        "--workflows",
        workflows.to_str().unwrap(),
        "--yes",
        "pin",
    ])
    .unwrap();

    let provider = UnifiedProvider::new(UnifiedProviderConfig {
        github_url: cli.github_url.clone(),
        github_token: cli.github_token.clone(),
        bitbucket_url: cli.bitbucket_url.clone(),
        bitbucket_token: cli.bitbucket_token.clone(),
        gitlab_url: cli.gitlab_url.clone(),
        gitlab_token: cli.gitlab_token.clone(),
        forgejo_url: cli.forgejo_url.clone(),
        forgejo_token: cli.forgejo_token.clone(),
    });
    let registry = OciRegistryProvider::new(None, None);

    run(cli, provider, registry, vec![workflows]).await.unwrap();

    let content = fs::read_to_string(wf_path).unwrap();
    assert!(content.contains("actions/checkout@hashv3 # v3"));
}

#[tokio::test]
async fn test_verify_command() {
    let dir = tempdir().unwrap();
    let workflows = dir.path().join(".github/workflows");
    fs::create_dir_all(&workflows).unwrap();

    // Unpinned
    let unpinned_path = workflows.join("unpinned.yml");
    fs::write(&unpinned_path, "uses: actions/checkout@v3").unwrap();

    let cli = Cli::try_parse_from([
        "pinner",
        "--workflows",
        workflows.to_str().unwrap(),
        "verify",
    ])
    .unwrap();

    let provider = UnifiedProvider::new(UnifiedProviderConfig::default());
    let registry = OciRegistryProvider::new(None, None);

    let res = run(
        cli,
        provider.clone(),
        registry.clone(),
        vec![workflows.clone()],
    )
    .await;
    assert!(res.is_err());

    // Pinned
    fs::write(
        workflows.join("pinned.yml"),
        "uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
    )
    .unwrap();
    fs::remove_file(unpinned_path).unwrap();

    let cli = Cli::try_parse_from([
        "pinner",
        "--workflows",
        workflows.to_str().unwrap(),
        "verify",
    ])
    .unwrap();

    run(cli, provider, registry, vec![workflows]).await.unwrap();
}

#[tokio::test]
async fn test_verify_false_positive() {
    let dir = tempdir().unwrap();
    let workflows = dir.path().join(".github/workflows");
    fs::create_dir_all(&workflows).unwrap();

    let yaml = r#"
name: Release
on:
  push:
    tags:
      - 'v*'

jobs:
  build:
    strategy:
      matrix:
        include:
          - target: aarch64-unknown-linux-gnu
            os: ubuntu-latest
            build-tool: cargo-zigbuild
          - target: aarch64-apple-darwin
            os: macos-latest
            build-tool: cargo
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v4
      - name: Build
        run: echo building for ${{ matrix.target }}
"#;

    fs::write(workflows.join("release.yml"), yaml).unwrap();

    let cli = Cli::try_parse_from([
        "pinner",
        "--workflows",
        workflows.to_str().unwrap(),
        "verify",
    ])
    .unwrap();

    let provider = UnifiedProvider::new(UnifiedProviderConfig::default());
    let registry = OciRegistryProvider::new(None, None);

    run(cli, provider, registry, vec![workflows]).await.unwrap();
}

#[tokio::test]
async fn test_github_url_env() {
    let mut server = Server::new_async().await;
    let _m = server
        .mock("GET", "/repos/o/r/commits/v1")
        .with_status(200)
        .with_body(r#"{"sha":"h"}"#)
        .create_async()
        .await;

    let dir = tempdir().unwrap();
    let f = dir.path().join("f.yml");
    fs::write(&f, "uses: o/r@v1").unwrap();

    std::env::set_var("GITHUB_URL", server.url());

    let cli = Cli::try_parse_from(["pinner", "--workflows", f.to_str().unwrap(), "--yes", "pin"])
        .unwrap();

    let provider = UnifiedProvider::new(UnifiedProviderConfig {
        github_url: cli.github_url.clone(),
        ..Default::default()
    });
    let registry = OciRegistryProvider::new(None, None);

    run(cli, provider, registry, vec![f.clone()]).await.unwrap();

    assert!(fs::read_to_string(&f).unwrap().contains("o/r@h"));
    std::env::remove_var("GITHUB_URL");
}

#[tokio::test]
async fn test_upgrade_command() {
    let mut github_server = Server::new_async().await;
    let _m1 = github_server
        .mock("GET", "/repos/actions/checkout/releases/latest")
        .with_status(200)
        .with_body(r#"{"tag_name":"v4"}"#)
        .create_async()
        .await;
    let _m2 = github_server
        .mock("GET", "/repos/actions/checkout/commits/v4")
        .with_status(200)
        .with_body(r#"{"sha":"hashv4"}"#)
        .create_async()
        .await;

    let dir = tempdir().unwrap();
    let wf = dir.path().join("ci.yml");
    fs::write(&wf, "uses: actions/checkout@v3").unwrap();

    let cli = Cli::try_parse_from([
        "pinner",
        "--github-url",
        &github_server.url(),
        "--workflows",
        wf.to_str().unwrap(),
        "--yes",
        "upgrade",
    ])
    .unwrap();

    let provider = UnifiedProvider::new(UnifiedProviderConfig {
        github_url: cli.github_url.clone(),
        ..Default::default()
    });
    let registry = OciRegistryProvider::new(None, None);

    run(cli, provider, registry, vec![wf.clone()])
        .await
        .unwrap();

    let content = fs::read_to_string(&wf).unwrap();
    assert!(content.contains("actions/checkout@hashv4 # v4"));
}

#[tokio::test]
async fn test_set_command() {
    let dir = tempdir().unwrap();
    let wf = dir.path().join("ci.yml");
    fs::write(&wf, "uses: actions/checkout@v3").unwrap();

    let cli = Cli::try_parse_from([
        "pinner",
        "--workflows",
        wf.to_str().unwrap(),
        "--yes",
        "set",
        "actions/checkout",
        "fixedhash",
    ])
    .unwrap();

    let provider = UnifiedProvider::new(UnifiedProviderConfig::default());
    let registry = OciRegistryProvider::new(None, None);

    run(cli, provider, registry, vec![wf.clone()])
        .await
        .unwrap();

    let content = fs::read_to_string(&wf).unwrap();
    assert!(content.contains("actions/checkout@fixedhash"));
}

#[tokio::test]
async fn test_install_hook_command() {
    let dir = tempdir().unwrap();
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    // Create a mock .git directory
    fs::create_dir(".git").unwrap();

    let cli = Cli::try_parse_from(["pinner", "install-hook"]).unwrap();
    let provider = UnifiedProvider::new(UnifiedProviderConfig::default());
    let registry = OciRegistryProvider::new(None, None);

    run(cli, provider, registry, vec![]).await.unwrap();

    assert!(dir.path().join(".git/hooks/pre-commit").exists());

    std::env::set_current_dir(original_dir).unwrap();
}

#[tokio::test]
async fn test_generate_completion_command() {
    let cli = Cli::try_parse_from(["pinner", "generate-completion", "bash"]).unwrap();
    let provider = UnifiedProvider::new(UnifiedProviderConfig::default());
    let registry = OciRegistryProvider::new(None, None);

    // This command currently returns Ok(()) in pinner::run and is handled in main.rs
    // But we still want to cover the match arm in pinner::run
    run(cli, provider, registry, vec![]).await.unwrap();
}
