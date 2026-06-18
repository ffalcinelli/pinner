use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_cli_help() {
    let mut cmd = Command::cargo_bin("pinner").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("pinner [OPTIONS] <COMMAND>"));
}

#[test]
fn test_cli_version() {
    let mut cmd = Command::cargo_bin("pinner").unwrap();
    cmd.arg("--version");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("pinner"));
}

#[test]
fn test_cli_verify_failure() {
    let dir = tempdir().unwrap();
    let wf = dir.path().join("ci.yml");
    fs::write(&wf, "uses: actions/checkout@v3").unwrap();

    let mut cmd = Command::cargo_bin("pinner").unwrap();
    cmd.arg("--workflows")
        .arg(wf.to_str().unwrap())
        .arg("verify");
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("Verification failed"));
}

#[test]
fn test_cli_verify_success() {
    let dir = tempdir().unwrap();
    let wf = dir.path().join("ci.yml");
    fs::write(
        &wf,
        "uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("pinner").unwrap();
    cmd.arg("--workflows")
        .arg(wf.to_str().unwrap())
        .arg("verify");
    cmd.assert().success();
}

#[test]
fn test_cli_env_override() {
    let dir = tempdir().unwrap();
    let wf = dir.path().join("ci.yml");
    fs::write(&wf, "uses: actions/checkout@v3").unwrap();

    // Use a non-existent URL to force a failure that confirms the env var is read
    let mut cmd = Command::cargo_bin("pinner").unwrap();
    cmd.env("PINNER_GITHUB_URL", "http://invalid-url-pinner-test")
        .env("PINNER_NO_CACHE", "true")
        .arg("--workflows")
        .arg(wf.to_str().unwrap())
        .arg("--yes")
        .arg("pin");

    // It should try to reach the invalid URL and print a warning about failed requests
    cmd.assert()
        .success()
        .stderr(predicate::str::contains("Request failed"));
}
