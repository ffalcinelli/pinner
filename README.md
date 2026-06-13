# Pinner 🧪

[![CI](https://github.com/ffalcinelli/pinner/actions/workflows/ci.yml/badge.svg)](https://github.com/ffalcinelli/pinner/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/ffalcinelli/pinner/graph/badge.svg)](https://codecov.io/gh/ffalcinelli/pinner)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![GitHub release](https://img.shields.io/github/v/release/ffalcinelli/pinner)](https://github.com/ffalcinelli/pinner/releases)
[![Docs.rs](https://docs.rs/pinner/badge.svg)](https://docs.rs/pinner)

A high-performance Rust CLI utility to **hash-pin GitHub Actions** in your workflow files. Secure your CI/CD supply chain by converting volatile, mutable tags (like `@v2`) into immutable, cryptographic commit SHAs (like `@df4cb1c...`).

## Why Pin? 🔒

Using mutable tags like `@v2` or `@main` in GitHub Actions introduces a security risk. If an attacker gains access to an action's repository, they can move the tag to a malicious commit, leading to a supply chain attack on your infrastructure. 

Hash-pinning ensures that you run the **exact** code you've audited, every single time. Pinner automates this process while keeping your workflows readable by appending the original tag as a comment.

## Features ✨

- **Surgical Replacement**: Uses `tree-sitter` for precise YAML parsing, preserving comments, indentation, and formatting perfectly.
- **Tag Preservation**: Automatically appends the original tag as a comment (e.g., `@<hash> # v2`).
- **GitHub API Integration**: Fetches the correct commit SHA for any tag or branch.
- **Enterprise Support**: Works with GitHub Enterprise via custom API URLs.
- **Flexible Upgrades**: Multiple strategies to keep your actions up to date (Major, Minor, Latest).
- **CI Ready**: Includes a `verify` mode to ensure all actions remain pinned in your PRs.

## Installation 🛠️

### One-line installation (Recommended)

**macOS/Linux:**
```bash
curl -LsSf https://raw.githubusercontent.com/ffalcinelli/pinner/main/install.sh | sh
```

**Windows:**
```powershell
powershell -ExecutionPolicy ByPass -c "irm https://raw.githubusercontent.com/ffalcinelli/pinner/main/install.ps1 | iex"
```

### From source

```bash
cargo install pinner
```

## Usage 🚀

### 1. Pin all actions
Scans `.github/workflows/` and converts all tags to pinned hashes.
```bash
pinner pin
```
*Input:* `- uses: actions/checkout@v3`  
*Output:* `- uses: actions/checkout@8f4b7f84864484a7bf31766abe9204da3cbe65b3 # v3`

### 2. Upgrade to latest
Update pinned actions to their latest versions based on a strategy.
```bash
# Default: Upgrade to latest available release
pinner upgrade

# Upgrade only within the current major version (e.g., v2.1.0 -> v2.4.5)
pinner upgrade --upgrade-strategy major

# Upgrade to the latest commit on the default branch
pinner upgrade --upgrade-strategy commit
```

### 3. Verify pinning
Ensure that all actions in your workflows are pinned. Perfect for CI pipelines.
```bash
pinner verify
```

### 4. Set a specific action
Forcibly updates a specific action across all workflows.
```bash
pinner set actions/checkout df4cb1c069e1874edd31b4311f1884172cec0e10
```

### 5. Generate Shell Completions
Generate autocompletion scripts for your favorite shell.
```bash
pinner generate-completion bash > /usr/local/etc/bash_completion.d/pinner
```

## Configuration ⚙️

Pinner can be configured via a `.pinner.toml` file in your repository root.

```toml
# List of actions to ignore during pinning/upgrading
ignore_actions = ["my-org/private-action"]

# Number of concurrent GitHub API requests (default: 10)
concurrency = 5

# Custom GitHub API URL (for GitHub Enterprise)
github_url = "https://github.mycompany.com/api/v3"
```

## Global Flags 🚩

- `-w, --workflows <PATH>`: Files or directories to process (default: `.github/workflows/`).
- `-y, --yes`: Automatically confirm all replacements.
- `--dry-run`: Show diff without modifying files.
- `--json`: Output results in JSON format.
- `--token <TOKEN>`: GitHub API token (can also be set via `GITHUB_TOKEN` env).
- `--github-url <URL>`: Custom GitHub API URL (for GHE).
- `-q, --quiet`: Suppress all console output.
- `-v, --verbose`: Print verbose output.

## Rate Limiting & Authentication 🔑

To avoid GitHub API rate limits, provide a token:
```bash
export GITHUB_TOKEN=ghp_your_token_here
pinner pin
```

## CI/CD Integration 🤖

Add this to your workflow to ensure all actions stay pinned:

```yaml
jobs:
  verify-pinning:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install Pinner
        run: curl -LsSf https://raw.githubusercontent.com/ffalcinelli/pinner/main/install.sh | sh
      - name: Verify Pinning
        run: pinner verify
```

## Name Origin ⚗️

The name **Pinner** is inspired by the **Pinner reaction** in organic chemistry. Discovered by Adolf Pinner, this reaction involves the acid-catalyzed conversion of a reactive nitrile into a highly stable Pinner salt.

Just as the Pinner reaction transforms a volatile compound into a stable, fixed salt, this CLI transforms "floating" action tags into secure, immutable, and fixed commit SHAs.

## License 📄

MIT License. See [LICENSE](LICENSE) for details.
