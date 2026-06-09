use clap::Parser;
use pinner::{run, Cli, ReqwestGithubProvider};
use std::path::Path;

#[cfg(not(tarpaulin))]
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let github = ReqwestGithubProvider::default();
    let workflows_dir = Path::new(".github/workflows");
    if let Err(e) = run(cli, github, workflows_dir).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
