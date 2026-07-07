//! Pinner is a high-performance utility for securing CI/CD workflows by pinning
//! mutable tags (like `@v1`) to immutable commit SHAs.
//!
//! It supports GitHub Actions, GitLab CI/CD, Bitbucket Pipelines, Forgejo,
//! and OCI container registries. It uses a precise AST-based parser to
//! ensure that YAML formatting and comments are preserved.

pub mod cli;
pub mod config;
pub mod core;
pub mod error;
pub mod patcher;
pub mod pipeline;
pub mod resolver;
pub mod scanner;

pub use cli::{Cli, Commands};
pub use error::PinnerError;
pub use patcher::{Formatter, Patcher};
pub use pipeline::init::{init_project, init_project_with_selection, install_git_hook};
pub use pipeline::Pipeline;
pub use resolver::{CachedProvider, RegistryProvider, RemoteProvider, Resolver};
pub use scanner::Scanner;

use std::path::PathBuf;
use std::sync::Arc;

pub async fn run<G: RemoteProvider + 'static, R: RegistryProvider + 'static>(
    cli: Cli,
    remote: G,
    registry: R,
    paths: Vec<PathBuf>,
) -> Result<(), PinnerError> {
    if cli.offline {
        match cli.command {
            Commands::Verify {
                check_osv: true, ..
            } => {
                return Err(PinnerError::Config(
                    "Cannot check OSV when offline mode is enabled".into(),
                ));
            }
            Commands::Scan { .. } => {
                return Err(PinnerError::Config(
                    "Cannot run scan in offline mode".into(),
                ));
            }
            _ => {}
        }
    }
    let upgrade_strategy = match &cli.command {
        Commands::Upgrade {
            upgrade_strategy, ..
        }
        | Commands::Scan { upgrade_strategy } => upgrade_strategy.clone(),
        _ => crate::cli::UpgradeStrategy::Latest,
    };
    let config = crate::config::Config::load();
    let scanner = Scanner::new(cli.ignore.clone());
    let local_vetted: Vec<String> = config
        .vetted
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.reference)
        .collect();
    let local_compromised: Vec<String> = config
        .compromised
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.reference)
        .collect();

    let global_config = crate::config::Config::load_global();
    let global_vetted: Vec<String> = global_config
        .vetted
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.reference)
        .collect();
    let global_compromised: Vec<String> = global_config
        .compromised
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.reference)
        .collect();

    let mut vetted = local_vetted;
    for item in global_vetted {
        if !vetted.contains(&item) && !local_compromised.contains(&item) {
            vetted.push(item);
        }
    }

    let mut compromised = local_compromised;
    for item in global_compromised {
        if !compromised.contains(&item) && !vetted.contains(&item) {
            compromised.push(item);
        }
    }

    let formatter = Formatter::new(
        cli.format.clone(),
        cli.quiet,
        vetted,
        compromised,
        !config.no_security_feedback.unwrap_or(false),
    );
    let disk_cache = if cli.no_cache {
        None
    } else {
        dirs::cache_dir().map(|mut p| {
            p.push("pinner");
            p
        })
    };

    let cache_ttl = if cli.no_cache {
        std::time::Duration::from_secs(0)
    } else {
        std::time::Duration::from_secs(cli.cache_ttl.unwrap_or(3600))
    };

    let osv_client = Arc::new(resolver::OsvClient::new(
        disk_cache.clone(),
        cli.offline,
        cache_ttl,
    ));
    let resolver = Resolver::new(
        Arc::new(CachedProvider::new(
            remote,
            disk_cache,
            cli.offline,
            cache_ttl,
        )),
        Arc::new(registry),
        osv_client,
        upgrade_strategy,
        cli.concurrency.unwrap_or(10),
    );
    let ui = Arc::new(crate::patcher::ui::ConsoleUi::new(cli.yes));
    let patcher = Patcher::new(formatter, ui, cli.dry_run);

    let pipeline = Pipeline::new(scanner, resolver, patcher);

    match cli.command {
        Commands::Pin => pipeline.pin(&paths).await?,
        Commands::Upgrade { interactive, .. } => pipeline.upgrade(&paths, interactive).await?,
        Commands::Verify { check_osv, strict } => {
            let result = pipeline.verify(&paths, check_osv, strict).await?;
            if cli.format == crate::cli::OutputFormat::Json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result)
                        .map_err(|e| PinnerError::Api(e.to_string()))?
                );
            }
            if !result.is_success() {
                return Err(PinnerError::VerificationFailed(
                    "Some actions are not pinned to a SHA, are compromised, or are not vetted"
                        .into(),
                ));
            }
        }
        Commands::Set { action, hash } => pipeline.set(&paths, &action, &hash).await?,
        Commands::InstallHook => install_git_hook()?,
        Commands::Init => init_project()?,
        Commands::ExportSbom => pipeline.export_sbom(&paths).await?,
        Commands::Scan { .. } => pipeline.scan(&paths, cli.yes).await?,
        Commands::PrCreate { message, branch } => {
            pipeline.pr_create(&paths, &branch, &message).await?;
        }
        Commands::GenerateCompletion { .. } => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::UpgradeStrategy;
    use crate::resolver::provider::MockRemoteProvider;
    use crate::resolver::registry::MockRegistryProvider;
    use std::fs;
    use std::time::Duration;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_pipeline_verify() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let osv_client = Arc::new(resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let res = pipeline
            .verify(std::slice::from_ref(&f), false, false)
            .await
            .unwrap();
        assert!(!res.is_success()); // v3 is not pinned

        fs::write(
            &f,
            "uses: actions/checkout@a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
        )
        .unwrap();
        let res = pipeline
            .verify(std::slice::from_ref(&f), false, false)
            .await
            .unwrap();
        assert!(res.is_success());
    }

    #[tokio::test]
    async fn test_pipeline_set() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let osv_client = Arc::new(resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        pipeline
            .set(std::slice::from_ref(&f), "actions/checkout", "newhash")
            .await
            .unwrap();
        let content = fs::read_to_string(f).unwrap();
        assert!(content.contains("actions/checkout@newhash"));
    }

    #[tokio::test]
    async fn test_pipeline_scan() {
        let mut osv_server = mockito::Server::new_async().await;
        std::env::set_var("PINNER_OSV_URL", osv_server.url());

        // Mock clean commit
        let _m1 = osv_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"1111111111111111111111111111111111111111"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"vulns":[]}"#)
            .create_async()
            .await;

        // Mock compromised commit (supply chain attack)
        let _m2 = osv_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"2222222222222222222222222222222222222222"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"vulns":[{"id":"GHSA-1","summary":"Malicious package backdoored"}]}"#)
            .create_async()
            .await;

        // Mock standard vulnerable commit
        let _m3 = osv_server
            .mock("POST", "/")
            .match_body(mockito::Matcher::JsonString(
                r#"{"commit":"3333333333333333333333333333333333333333"}"#.to_string(),
            ))
            .with_status(200)
            .with_body(r#"{"vulns":[{"id":"GHSA-2","summary":"Standard DoS vulnerability"}]}"#)
            .create_async()
            .await;

        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "jobs:\n  test:\n    steps:\n      - uses: clean@1111111111111111111111111111111111111111\n      - uses: comp@2222222222222222222222222222222222222222\n      - uses: vuln@3333333333333333333333333333333333333333").unwrap();

        let scanner = Scanner::new(vec![]);

        let mut remote = MockRemoteProvider::new();
        remote.expect_get_latest_release().returning(|action, _| {
            if action.0 == "clean" {
                Ok("v1.2.3".to_string())
            } else if action.0 == "comp" {
                Ok("v2.0.0".to_string())
            } else {
                Ok("v3.0.0".to_string())
            }
        });

        remote.expect_get_commit_sha().returning(|action, _tag, _| {
            if action.0 == "clean" {
                Ok(crate::core::DependencyRef::GitSha(
                    "9999999999999999999999999999999999999999".to_string(),
                ))
            } else if action.0 == "comp" {
                Ok(crate::core::DependencyRef::GitSha(
                    "8888888888888888888888888888888888888888".to_string(),
                ))
            } else {
                Ok(crate::core::DependencyRef::GitSha(
                    "7777777777777777777777777777777777777777".to_string(),
                ))
            }
        });

        let osv_client = Arc::new(resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        // Run scan with yes=true
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();

        pipeline.scan(std::slice::from_ref(&f), true).await.unwrap();

        // Check that .pinner.toml was updated
        let toml_content = fs::read_to_string(".pinner.toml").unwrap();
        // Assert structured format is serialized properly
        assert!(toml_content.contains("ref = \"clean@1111111111111111111111111111111111111111\""));
        assert!(toml_content.contains("ref = \"comp@2222222222222222222222222222222222222222\""));
        assert!(toml_content.contains("timestamp ="));
        assert!(!toml_content.contains("vuln@3333333333333333333333333333333333333333")); // standard vulnerable NOT auto-added

        std::env::set_current_dir(original_dir).unwrap();
        std::env::remove_var("PINNER_OSV_URL");
    }

    #[tokio::test]
    async fn test_local_override_precedence() {
        let local_vetted = vec!["actions/checkout@v3".to_string()];
        let local_compromised = vec![];
        let global_vetted = vec![];
        let global_compromised = vec!["actions/checkout@v3".to_string()];

        let mut vetted = local_vetted;
        for item in global_vetted {
            if !vetted.contains(&item) && !local_compromised.contains(&item) {
                vetted.push(item);
            }
        }
        let mut compromised = local_compromised;
        for item in global_compromised {
            if !compromised.contains(&item) && !vetted.contains(&item) {
                compromised.push(item);
            }
        }

        let formatter = Formatter::new(
            crate::cli::OutputFormat::Text,
            true,
            vetted,
            compromised,
            true,
        );

        let status = formatter.check_hash_security("actions/checkout", "v3");
        assert_eq!(
            status,
            crate::patcher::formatter::HashSecurityStatus::Vetted
        );
    }

    #[tokio::test]
    async fn test_pipeline_getters() {
        let scanner = Scanner::new(vec![]);
        let osv_client = Arc::new(resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(MockRemoteProvider::new()),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        let _ = pipeline.scanner();
        let _ = pipeline.resolver();
        let _ = pipeline.patcher();
    }

    #[tokio::test]
    async fn test_pipeline_upgrade_interactive() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("f.yml");
        fs::write(&f, "uses: actions/checkout@v3").unwrap();

        let scanner = Scanner::new(vec![]);
        let mut remote = MockRemoteProvider::new();
        remote
            .expect_get_latest_release()
            .returning(|_, _| Ok("v4".to_string()));
        remote
            .expect_get_commit_sha()
            .returning(|_, tag, _| Ok(crate::core::DependencyRef::GitSha(format!("{}sha", tag))));

        let osv_client = Arc::new(resolver::OsvClient::new(
            None,
            false,
            Duration::from_secs(0),
        ));
        let resolver = Resolver::new(
            Arc::new(remote),
            Arc::new(MockRegistryProvider::new()),
            osv_client,
            UpgradeStrategy::Latest,
            1,
        );
        let ui = Arc::new(crate::patcher::ui::TestUi { response: true });
        let patcher = Patcher::new(
            Formatter::new(crate::cli::OutputFormat::Text, true, vec![], vec![], true),
            ui,
            false,
        );
        let pipeline = Pipeline::new(scanner, resolver, patcher);

        pipeline
            .upgrade(std::slice::from_ref(&f), true)
            .await
            .unwrap();
    }
}
