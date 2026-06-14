use mockito::Server;
use std::fs;
use std::process::Command;
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
    fs::write(workflows.join("ci.yml"), "uses: actions/checkout@v3").unwrap();

    let status = Command::new("cargo")
        .arg("run")
        .arg("--")
        .arg("--github-url")
        .arg(github_server.url())
        .arg("--workflows")
        .arg(workflows.to_str().unwrap()) // Point directly to workflows dir
        .arg("--yes")
        .arg("pin")
        .status()
        .unwrap();

    assert!(status.success());
    let content = fs::read_to_string(workflows.join("ci.yml")).unwrap();
    assert!(content.contains("actions/checkout@hashv3 # v3"));
}

#[tokio::test]
async fn test_verify_command() {
    let dir = tempdir().unwrap();
    let workflows = dir.path().join(".github/workflows");
    fs::create_dir_all(&workflows).unwrap();

    // Unpinned
    fs::write(workflows.join("unpinned.yml"), "uses: actions/checkout@v3").unwrap();

    let status = Command::new("cargo")
        .arg("run")
        .arg("--")
        .arg("--workflows")
        .arg(workflows.to_str().unwrap())
        .arg("verify")
        .status()
        .unwrap();

    assert!(!status.success());

    // Pinned
    fs::write(
        workflows.join("pinned.yml"),
        "uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
    )
    .unwrap();
    fs::remove_file(workflows.join("unpinned.yml")).unwrap();

    let status = Command::new("cargo")
        .arg("run")
        .arg("--")
        .arg("--workflows")
        .arg(workflows.to_str().unwrap())
        .arg("verify")
        .status()
        .unwrap();

    assert!(status.success());
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

    let status = Command::new("cargo")
        .arg("run")
        .arg("--")
        .arg("--workflows")
        .arg(workflows.to_str().unwrap())
        .arg("verify")
        .status()
        .unwrap();

    assert!(status.success());
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

    let status = Command::new("cargo")
        .arg("run")
        .arg("--")
        .env("GITHUB_URL", server.url())
        .arg("--workflows")
        .arg(&f)
        .arg("--yes")
        .arg("pin")
        .status()
        .unwrap();

    assert!(status.success());
    assert!(fs::read_to_string(&f).unwrap().contains("o/r@h"));
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

    let status = Command::new("cargo")
        .arg("run")
        .arg("--")
        .arg("--github-url")
        .arg(github_server.url())
        .arg("--workflows")
        .arg(wf.to_str().unwrap())
        .arg("--yes")
        .arg("upgrade")
        .status()
        .unwrap();

    assert!(status.success());
    let content = fs::read_to_string(&wf).unwrap();
    assert!(content.contains("actions/checkout@hashv4 # v4"));
}

#[tokio::test]
async fn test_set_command() {
    let dir = tempdir().unwrap();
    let wf = dir.path().join("ci.yml");
    fs::write(&wf, "uses: actions/checkout@v3").unwrap();

    let status = Command::new("cargo")
        .arg("run")
        .arg("--")
        .arg("--workflows")
        .arg(wf.to_str().unwrap())
        .arg("--yes")
        .arg("set")
        .arg("actions/checkout")
        .arg("fixedhash")
        .status()
        .unwrap();

    assert!(status.success());
    let content = fs::read_to_string(&wf).unwrap();
    assert!(content.contains("actions/checkout@fixedhash"));
}
