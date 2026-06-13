# Pinner 🧪

[![CI](https://github.com/ffalcinelli/pinner/actions/workflows/ci.yml/badge.svg)](https://github.com/ffalcinelli/pinner/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/ffalcinelli/pinner/graph/badge.svg)](https://codecov.io/gh/ffalcinelli/pinner)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![GitHub release](https://img.shields.io/github/v/release/ffalcinelli/pinner)](https://github.com/ffalcinelli/pinner/releases)
[![Docs.rs](https://docs.rs/pinner/badge.svg)](https://docs.rs/pinner)

A high-performance Rust CLI utility to **hash-pin your CI/CD dependencies**. Secure your supply chain by converting volatile, mutable tags (like `@v2`) into immutable, cryptographic commit SHAs (like `@df4cb1c...`).

[**🚀 Get Started Guide**](https://ffalcinelli.github.io/pinner/getting-started.html)

## Why Pin? 🔒

Using mutable tags like `@v2` or `@main` in GitHub Actions or other CI/CD providers introduces a security risk. If an attacker gains access to a dependency's repository, they can move the tag to a malicious commit, leading to a supply chain attack on your infrastructure. 

Hash-pinning ensures that you run the **exact** code you've audited, every single time. Pinner automates this process while keeping your workflows readable by appending the original tag as a comment.

## Features ✨

- **Surgical Replacement**: Uses `tree-sitter` for precise YAML parsing, preserving comments, indentation, and formatting perfectly.
- **Multi-Forge Support**: Works with GitHub, GitLab, Bitbucket, and Forgejo/Gitea.
- **Tag Preservation**: Automatically appends the original tag as a comment (e.g., `@<hash> # v2`).
- **Container Pinning**: Automatically pins Docker images to their immutable digests (e.g., `image: alpine@sha256:...`).
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
Scans workflows and converts all tags to pinned hashes.
```bash
pinner pin
```
*Input:* `- uses: actions/checkout@v3`  
*Output:* `- uses: actions/checkout@8f4b7f84864484a7bf31766abe9204da3cbe65b3 # v3`

### 2. Upgrade to latest
Update pinned actions to their latest versions based on a strategy.

> [!CAUTION]
> **Automatic upgrades can undermine your security.** The primary goal of hash-pinning is to ensure you run only code you have vetted. While `verify` should be run in every CI pipeline to enforce this, `upgrade` should be used as an intentional step in your development process, followed by a review of the new version to maintain the integrity of your supply chain. Automated, unvetted upgrades re-introduce the very supply chain risks that hash-pinning is designed to prevent.

```bash
# Default: Upgrade to latest available release
pinner upgrade
```

# Upgrade only within the current major version (e.g., v2.1.0 -> v2.4.5)
pinner upgrade --upgrade-strategy major
```

### 3. Verify pinning
Ensure that all actions in your workflows are pinned. Perfect for CI pipelines.
```bash
pinner verify
```

## Configuration ⚙️

Pinner can be configured via a `.pinner.toml` file in your repository root.

```toml
# List of actions to ignore during pinning/upgrading
ignore_actions = ["my-org/private-action"]

# Number of concurrent API requests (default: 10)
concurrency = 5

# Custom API URLs (for Enterprise instances)
github_url = "https://github.mycompany.com/api/v3"
gitlab_url = "https://gitlab.mycompany.com/api/v4"
```

## Supported Forges 🌐

Pinner supports multiple CI/CD and git hosting platforms:

| Forge | Command | Env Var for Token |
|-------|---------|-------------------|
| GitHub | `pinner pin` | `GITHUB_TOKEN` |
| GitLab | `pinner pin` | `GITLAB_TOKEN` |
| Bitbucket | `pinner pin` | `BITBUCKET_TOKEN` |
| Forgejo/Gitea | `pinner pin` | `FORGEJO_TOKEN` |

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

Just as the Pinner reaction transforms a volatile compound into a stable, fixed salt, this CLI transforms "floating" tags into secure, immutable, and fixed commit SHAs.

## License 📄

MIT License. See [LICENSE](LICENSE) for details.
