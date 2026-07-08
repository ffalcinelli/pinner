use crate::error::PinnerError;
use colored::Colorize;
use std::fs;
use std::path::PathBuf;

/// Initializes a new `.pinner.toml` configuration file with sensible defaults, using the specified selection for vetted Actions.
pub fn init_project_with_selection(selection: usize) -> Result<(), PinnerError> {
    init_project_internal(Some(selection))
}

/// Initializes a new `.pinner.toml` configuration file with sensible defaults.
pub fn init_project() -> Result<(), PinnerError> {
    init_project_internal(None)
}

fn init_project_internal(selection_opt: Option<usize>) -> Result<(), PinnerError> {
    let mut config_lines = Vec::new();
    config_lines.push("# Pinner configuration file".to_string());
    config_lines
        .push("# For full documentation, see: https://github.com/ffalcinelli/pinner".to_string());
    config_lines.push("".to_string());

    let mut detected = Vec::new();
    if std::path::Path::new(".github/workflows").exists() {
        detected.push("GitHub Actions");
    }
    if std::path::Path::new(".gitlab-ci.yml").exists() {
        detected.push("GitLab CI");
    }
    if std::path::Path::new("bitbucket-pipelines.yml").exists()
        || std::path::Path::new("bitbucket-pipelines.yaml").exists()
    {
        detected.push("Bitbucket Pipelines");
    }
    if std::path::Path::new(".forgejo/workflows").exists() {
        detected.push("Forgejo/Gitea");
    }
    if std::path::Path::new(".circleci/config.yml").exists() {
        detected.push("CircleCI");
    }

    if !detected.is_empty() {
        println!(
            "{} Detected CI systems: {}",
            "✔".green().bold(),
            detected.join(", ").cyan()
        );
    } else {
        println!(
            "{} No CI systems detected, using defaults.",
            "⚠".yellow().bold()
        );
    }

    config_lines.push("# Automatically confirm all replacements".to_string());
    config_lines.push("yes = false".to_string());
    config_lines.push("".to_string());
    config_lines.push("# Upgrade strategy: latest, major, minor, commit".to_string());
    config_lines.push("upgrade_strategy = \"latest\"".to_string());
    config_lines.push("".to_string());
    config_lines.push("# Actions or images to ignore".to_string());
    config_lines.push("ignore = []".to_string());
    config_lines.push("".to_string());
    config_lines.push("# Number of concurrent API requests".to_string());
    config_lines.push("concurrency = 10".to_string());
    config_lines.push("".to_string());

    let config_path = std::path::PathBuf::from(".pinner.toml");
    if config_path.exists() {
        println!(
            "{} .pinner.toml already exists, skipping creation.",
            "ℹ".blue().bold()
        );
    } else {
        let selection = match selection_opt {
            Some(s) => s,
            None => {
                let options = vec![
                    "None (start empty)",
                    "Default/GitHub (pre-populate with popular GitHub Actions)",
                ];
                dialoguer::Select::new()
                    .with_prompt("Select a service to populate the vetted whitelist")
                    .items(&options)
                    .default(1)
                    .interact()
                    .unwrap_or(0)
            }
        };

        let mut vetted_lines = Vec::new();
        if selection == 1 {
            vetted_lines.push("vetted = [".to_string());
            vetted_lines.push(
                "    \"actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332\", # v4.1.7"
                    .to_string(),
            );
            vetted_lines.push(
                "    \"actions/setup-node@601291da96165b6a1d4b1fb337131252d6e2735d\", # v4.0.3"
                    .to_string(),
            );
            vetted_lines.push(
                "    \"actions/setup-python@82c7e60c44059a00283f090ceb68f6854d17dcef\", # v5.1.0"
                    .to_string(),
            );
            vetted_lines.push(
                "    \"actions/setup-go@cd9a547d6d5b9454b6754024774b752817bf0a26\", # v5.0.2"
                    .to_string(),
            );
            vetted_lines.push(
                "    \"actions/cache@0c45773b623bea8c8e75f6c82b208c3cf94ea4f9\", # v4.0.2"
                    .to_string(),
            );
            vetted_lines.push("    \"actions/upload-artifact@65462800fd760344b1a7b4382951275a0abb4808\", # v4.3.3".to_string());
            vetted_lines.push("    \"actions/download-artifact@65a9edc5881444af0b9093a5e628f2fe47ea3d2e\"  # v4.1.7".to_string());
            vetted_lines.push("]".to_string());
        } else {
            vetted_lines.push("vetted = [".to_string());
            vetted_lines.push("    # \"actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332\", # Example vetted action".to_string());
            vetted_lines.push("]".to_string());
        }

        config_lines
            .push("# Vetted (trusted) dependency hashes or references (Whitelist)".to_string());
        config_lines.extend(vetted_lines);
        config_lines.push("".to_string());
        config_lines.push("# Compromised dependency hashes or references (Blacklist)".to_string());
        config_lines.push("compromised = [".to_string());
        config_lines.push("    # \"actions/checkout@badhash1234567890badhash1234567890bad\", # Example compromised action".to_string());
        config_lines.push("]".to_string());
        config_lines.push("".to_string());
        config_lines.push("# Disable visual security feedback".to_string());
        config_lines.push("no_security_feedback = false".to_string());

        fs::write(&config_path, config_lines.join("\n"))?;
        println!("{} Created .pinner.toml", "✔".green().bold());
    }

    Ok(())
}

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
    use std::env;
    use tempfile::tempdir;

    #[test]
    #[serial_test::serial]
    fn test_init_project_with_selection_0() {
        let dir = tempdir().unwrap();
        let orig_dir = env::current_dir().unwrap();
        env::set_current_dir(dir.path()).unwrap();

        // Create dummy CI paths to hit the detection branches
        fs::create_dir_all(".github/workflows").unwrap();
        fs::create_dir_all(".forgejo/workflows").unwrap();
        fs::write(".gitlab-ci.yml", "").unwrap();
        fs::write("bitbucket-pipelines.yml", "").unwrap();
        fs::create_dir_all(".circleci").unwrap();
        fs::write(".circleci/config.yml", "").unwrap();

        let res = init_project_with_selection(0);
        assert!(res.is_ok());

        // Config path should exist
        let config_path = std::path::PathBuf::from(".pinner.toml");
        assert!(config_path.exists());

        // Running it again should skip creation
        let res_again = init_project_with_selection(0);
        assert!(res_again.is_ok());

        env::set_current_dir(orig_dir).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_init_project_with_selection_1() {
        let dir = tempdir().unwrap();
        let orig_dir = env::current_dir().unwrap();
        env::set_current_dir(dir.path()).unwrap();

        let res = init_project_with_selection(1);
        assert!(res.is_ok());

        let config_path = std::path::PathBuf::from(".pinner.toml");
        assert!(config_path.exists());

        env::set_current_dir(orig_dir).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_install_git_hook_no_git() {
        let dir = tempdir().unwrap();
        let orig_dir = env::current_dir().unwrap();
        env::set_current_dir(dir.path()).unwrap();

        let res = install_git_hook();
        assert!(res.is_err());

        env::set_current_dir(orig_dir).unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn test_install_git_hook_success() {
        let dir = tempdir().unwrap();
        let orig_dir = env::current_dir().unwrap();
        env::set_current_dir(dir.path()).unwrap();

        fs::create_dir_all(".git/hooks").unwrap();
        let res = install_git_hook();
        assert!(res.is_ok());

        let hook_path = std::path::PathBuf::from(".git/hooks/pre-commit");
        assert!(hook_path.exists());

        env::set_current_dir(orig_dir).unwrap();
    }
}
