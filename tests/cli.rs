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

#[test]
fn test_cli_quiet_verbose_conflict_binary() {
    let mut cmd = Command::cargo_bin("pinner").unwrap();
    cmd.arg("--quiet").arg("--verbose").arg("verify");
    cmd.assert().failure().stderr(predicate::str::contains(
        "the argument '--quiet' cannot be used with '--verbose'",
    ));
}

#[test]
fn test_cli_oci_username_requires_password_binary() {
    let mut cmd = Command::cargo_bin("pinner").unwrap();
    cmd.arg("--oci-username").arg("foo").arg("verify");
    cmd.assert().failure().stderr(predicate::str::contains(
        "the following required arguments were not provided:\n  --oci-password <OCI_PASSWORD>",
    ));
}

#[test]
fn test_cli_json_removed_binary() {
    let mut cmd = Command::cargo_bin("pinner").unwrap();
    cmd.arg("--json").arg("verify");
    cmd.assert().failure().stderr(predicate::str::contains(
        "unexpected argument '--json' found",
    ));
}
