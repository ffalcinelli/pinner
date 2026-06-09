use async_trait::async_trait;
use clap::{Parser, Subcommand};
use colored::Colorize;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[cfg(test)]
use mockall::automock;

#[derive(Parser)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    /// Automatically confirm all replacements
    #[arg(short, long, global = true)]
    pub yes: bool,
    /// Suppress all console output
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand, Debug, PartialEq)]
pub enum Commands {
    Pin,
    Upgrade,
    Set { action: String, hash: String },
}

pub async fn run<G: GithubProvider>(
    cli: Cli,
    github: G,
    workflows_dir: &Path,
) -> Result<(), String> {
    let ops = Operations::new(github, cli.yes, cli.quiet);
    match cli.command {
        Commands::Pin => ops.pin(workflows_dir).await,
        Commands::Upgrade => ops.upgrade(workflows_dir).await,
        Commands::Set { action, hash } => ops.set(workflows_dir, &action, &hash).await,
    }
}

#[derive(Debug, Deserialize)]
struct RefResponse {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseResponse {
    tag_name: String,
}

#[cfg_attr(test, automock)]
#[async_trait]
pub trait GithubProvider: Send + Sync {
    async fn get_commit_sha(&self, action: &str, tag: &str) -> Result<String, String>;
    async fn get_latest_release(&self, action: &str) -> Result<String, String>;
}

pub struct ReqwestGithubProvider {
    client: reqwest::Client,
    base_url: String,
}

#[cfg(not(tarpaulin))]
impl Default for ReqwestGithubProvider {
    fn default() -> Self {
        Self::new("https://api.github.com".to_string())
    }
}

impl ReqwestGithubProvider {
    pub fn new(base_url: String) -> Self {
        let mut h = HeaderMap::new();
        h.insert(USER_AGENT, HeaderValue::from_static("pinner"));
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            if let Ok(auth) = HeaderValue::from_str(&format!("Bearer {}", token)) {
                h.insert(AUTHORIZATION, auth);
            }
        }
        let client = reqwest::Client::builder()
            .default_headers(h)
            .build()
            .unwrap();
        Self { client, base_url }
    }
}

#[async_trait]
impl GithubProvider for ReqwestGithubProvider {
    async fn get_commit_sha(&self, action: &str, tag: &str) -> Result<String, String> {
        let url = format!("{}/repos/{}/commits/{}", self.base_url, action, tag);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if resp.status().is_success() {
            let res: RefResponse = resp.json().await.map_err(|e| e.to_string())?;
            Ok(res.sha)
        } else {
            Err(format!(
                "HTTP {}: Could not resolve ref '{}' for {}",
                resp.status(),
                tag,
                action
            ))
        }
    }

    async fn get_latest_release(&self, action: &str) -> Result<String, String> {
        let url = format!("{}/repos/{}/releases/latest", self.base_url, action);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if resp.status().is_success() {
            let rel: ReleaseResponse = resp.json().await.map_err(|e| e.to_string())?;
            Ok(rel.tag_name)
        } else if resp.status().as_u16() == 404 {
            Ok("main".to_string())
        } else {
            Err(format!(
                "HTTP {}: Could not fetch latest release for {}",
                resp.status(),
                action
            ))
        }
    }
}

pub struct Operations<G: GithubProvider> {
    github: G,
    regex: Regex,
    yes: bool,
    quiet: bool,
}
impl<G: GithubProvider> Operations<G> {
    pub fn new(github: G, yes: bool, quiet: bool) -> Self {
        Self {
            github,
            yes,
            quiet,
            regex: Regex::new(r"(?m)^(.*?uses:\s*([^@\s#\n]+))(?:@([a-zA-Z0-9.\-_/]+))?(.*?)$")
                .unwrap(),
        }
    }

    pub async fn pin(&self, dir: &Path) -> Result<(), String> {
        self.process(dir, |a, v| {
            let (a, v) = (a.to_string(), v.map(|s| s.to_string()));
            async move {
                if let Some(ver) = v {
                    if ver.len() != 40 {
                        return self
                            .github
                            .get_commit_sha(&a, &ver)
                            .await
                            .ok()
                            .map(|s| (s, Some(ver)));
                    }
                }
                None
            }
        })
        .await
    }

    pub async fn set(&self, dir: &Path, action: &str, hash: &str) -> Result<(), String> {
        let (a, h) = (action.to_string(), hash.to_string());
        self.process(dir, move |act, _| {
            let (a, h, act_owned) = (a.clone(), h.clone(), act.to_string());
            async move {
                if act_owned == a {
                    Some((h, None))
                } else {
                    None
                }
            }
        })
        .await
    }

    pub async fn upgrade(&self, dir: &Path) -> Result<(), String> {
        self.process(dir, |a, _| {
            let a = a.to_string();
            async move {
                if let Ok(tag) = self.github.get_latest_release(&a).await {
                    if let Ok(sha) = self.github.get_commit_sha(&a, &tag).await {
                        return Some((sha, Some(tag)));
                    }
                }
                None
            }
        })
        .await
    }

    async fn process<F, Fut>(&self, dir: &Path, f: F) -> Result<(), String>
    where
        F: Fn(&str, Option<&str>) -> Fut,
        Fut: std::future::Future<Output = Option<(String, Option<String>)>>,
    {
        if !dir.exists() {
            return Err(format!("Directory not found: {}", dir.display()));
        }
        let comment_regex =
            Regex::new(r"^#\s*(v\d[a-zA-Z0-9.\-_]*|main|\d[a-zA-Z0-9.\-_]*)\s*").unwrap();
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
                let content = fs::read_to_string(&path).map_err(|e| e.to_string())?;
                let mut new = content.clone();
                let mut changes = Vec::new();
                for cap in self.regex.captures_iter(&content) {
                    if let Some((sha, tag)) = f(&cap[2], cap.get(3).map(|m| m.as_str())).await {
                        let _old_v = cap.get(3).map(|m| m.as_str()).unwrap_or("");
                        let suffix = &cap[4];

                        // Intelligent comment replacement:
                        let mut final_suffix = suffix.trim_start().to_string();
                        // If it starts with "# v" or "# main" or "# <numbers>", strip that specific part.
                        if let Some(mat) = comment_regex.find(&final_suffix) {
                            final_suffix = final_suffix[mat.end()..].trim_start().to_string();
                            if final_suffix.starts_with('#') {
                                final_suffix = final_suffix[1..].trim_start().to_string();
                            }
                        }

                        let new_comment = if let Some(t) = tag {
                            format!(" # {}", t)
                        } else {
                            "".to_string()
                        };
                        let extra_suffix = if final_suffix.is_empty() {
                            "".to_string()
                        } else {
                            format!(" # {}", final_suffix)
                        };
                        let new_line =
                            format!("{}@{}{}{}", &cap[1], sha, new_comment, extra_suffix);

                        changes.push((cap[0].to_string(), new_line.to_string()));
                        new = new.replace(&cap[0], &new_line);
                    }
                }
                if !changes.is_empty() && !self.quiet {
                    println!("\n{} {}", "File:".bold(), path.display().to_string().cyan());
                    for (old, new_ln) in &changes {
                        println!("  {} {}", "-".red(), old.trim().dimmed());
                        println!("  {} {}", "+".green(), new_ln.trim().yellow());
                    }
                    let mut should_write = self.yes;
                    if !should_write {
                        use std::io::Write;
                        print!(
                            "{} {}? [y/N]: ",
                            "Apply changes to".bold(),
                            path.display().to_string().cyan()
                        );
                        std::io::stdout().flush().unwrap();
                        let mut input = String::new();
                        if std::io::stdin().read_line(&mut input).is_ok() {
                            let input = input.trim().to_lowercase();
                            should_write = input == "y" || input == "yes";
                        }
                    }
                    if should_write {
                        fs::write(path, new).map_err(|e| e.to_string())?;
                        println!("{}", "✔ Updated successfully".green());
                    } else {
                        println!("{}", "✘ Skipped".yellow());
                    }
                } else if !changes.is_empty() && self.yes {
                    fs::write(path, new).map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::Server;
    use tempfile::tempdir;

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

        let p = ReqwestGithubProvider::new(s.url());
        assert!(p.get_commit_sha("o/r", "v1").await.is_ok());
        assert_eq!(p.get_latest_release("o/r").await.unwrap(), "v2");

        let mut mock = MockGithubProvider::new();
        mock.expect_get_commit_sha()
            .returning(|_, _| Ok("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".into()));
        mock.expect_get_latest_release()
            .returning(|_| Ok("v2".into()));

        let dir = tempdir().unwrap();
        let wd = dir.path().join("w");
        fs::create_dir_all(&wd).unwrap();
        fs::write(wd.join("f.yml"), "uses: o/r@v1").unwrap();
        fs::write(wd.join("untagged.yml"), "uses: actions/checkout").unwrap();

        let ops = Operations::new(mock, true, false);
        ops.pin(&wd).await.unwrap();
        assert!(fs::read_to_string(wd.join("f.yml"))
            .unwrap()
            .contains("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2 # v1"));

        let mut mock2 = MockGithubProvider::new();
        mock2
            .expect_get_latest_release()
            .returning(|_| Ok("v3".into()));
        mock2
            .expect_get_commit_sha()
            .returning(|_, _| Ok("692973e3d937129bcbf40652eb9f2f61becf3332".into()));
        let ops2 = Operations::new(mock2, true, false);
        ops2.upgrade(&wd).await.unwrap();
        let ut = fs::read_to_string(wd.join("untagged.yml")).unwrap();
        assert!(ut.contains("actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332 # v3"));

        let mut mock3 = MockGithubProvider::new();
        mock3
            .expect_get_commit_sha()
            .returning(|_, _| Ok("s".into()));
        run(
            Cli {
                command: Commands::Pin,
                yes: true,
                quiet: true,
            },
            mock3,
            &wd,
        )
        .await
        .unwrap();
        assert!(run(
            Cli {
                command: Commands::Pin,
                yes: true,
                quiet: true
            },
            MockGithubProvider::new(),
            Path::new("/n")
        )
        .await
        .is_err());
    }
}
