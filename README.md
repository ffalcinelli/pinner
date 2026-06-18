# Pinner 🧪

[![Crates.io Version](https://img.shields.io/crates/v/pinner?style=flat)](https://crates.io/crates/pinner)
[![CI Status](https://img.shields.io/github/actions/workflow/status/ffalcinelli/pinner/ci.yml?branch=main&style=flat&label=ci)](https://github.com/ffalcinelli/pinner/actions/workflows/ci.yml)
[![Codecov](https://img.shields.io/codecov/c/gh/ffalcinelli/pinner?style=flat)](https://codecov.io/gh/ffalcinelli/pinner)
[![Docs.rs](https://img.shields.io/docsrs/pinner?style=flat)](https://docs.rs/pinner)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat)](https://github.com/ffalcinelli/pinner/blob/main/LICENSE)
[![Rust Version](https://img.shields.io/badge/rust-1.80%2B-blue?style=flat)](https://www.rust-lang.org)

A high-performance Rust CLI utility to **hash-pin your CI/CD dependencies**. Secure your supply chain by converting volatile, mutable tags (like `@v2`) into immutable, cryptographic commit SHAs (like `@df4cb1c...`).

[**🚀 Get Started Guide**](https://ffalcinelli.github.io/pinner/getting-started.html)

## Why Pin? 🔒

Using mutable tags like `@v2` or `@main` in GitHub Actions or other CI/CD providers introduces a security risk. If an attacker gains access to a dependency's repository, they can move the tag to a malicious commit, leading to a supply chain attack on your infrastructure. 

Hash-pinning ensures that you run the **exact** code you've audited, every single time. Pinner automates this process while keeping your workflows readable by appending the original tag as a comment.

## Features ✨

- **Domain-Driven Pipeline**: Built on a strict Scanner -> Resolver -> Patcher architecture, ensuring high testability, concurrency, and safe mutations.
- **Surgical Replacement**: Uses `tree-sitter` for precise YAML parsing, preserving comments, indentation, and formatting perfectly.
- **Multi-Forge Support**: Works with GitHub, GitLab, Bitbucket, and Forgejo/Gitea.
- **Tag Preservation**: Automatically appends the original tag as a comment (e.g., `@<hash> # v2`).
- **Container Pinning**: Automatically pins Docker images to their immutable digests (e.g., `image: alpine@sha256:...`).
- **Flexible Upgrades**: Multiple strategies to keep your actions up to date (Major, Minor, Latest).
- **CI Ready**: Includes a `verify` mode to ensure all actions remain pinned in your PRs.
- **Security Scanning**: A `scan` subcommand to query the OpenSSF OSV database for known vulnerabilities and supply-chain compromises.
- **Visual Security Feedback**: Appends colorful indicators (`[✓ vetted]`, `[✗ compromised]`, or `[? not checked]`) during dry-runs and diff outputs.

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

```bash
# Upgrade only within the current major version (e.g., v2.1.0 -> v2.4.5)
pinner upgrade --upgrade-strategy major
```

### 3. Verify pinning
Ensure that all actions in your workflows are pinned. Perfect for CI pipelines.
```bash
pinner verify
```

### 4. Install Git Hook
Automatically install a pre-commit hook to verify pinning before every commit.
```bash
pinner install-hook
```

### 5. Manual Set
Forcibly update a specific action to a provided hash across all workflows.
```bash
pinner set actions/checkout 8f4b7f84864484a7bf31766abe9204da3cbe65b3
```

### 6. Initialize Configuration
Create a `.pinner.toml` with default settings.
```bash
pinner init
```

### 7. Export SBOM
Generate a Software Bill of Materials for your CI dependencies.
```bash
pinner export-sbom
```

### 8. Security Scan
Audits your dependencies for vulnerabilities. It queries the OpenSSF OSV database for both current hashes and proposed upgrade candidates, and executes Sigstore/Cosign provenance and signature verification for OCI container images. It presents an interactive report and updates your vetted whitelist or compromised blacklist.
```bash
# Scan workflows and interactively update your .pinner.toml config
pinner scan

# Scan workflows and automatically populate .pinner.toml config (great for automation)
pinner scan --yes
```

### 9. Shell Completions
Generate tab-completion scripts for your shell. Automatically detects your current shell if no argument is provided.
```bash
# Auto-detect current shell
pinner generate-completion > ~/.zshrc.d/_pinner

# Or specify explicitly
pinner generate-completion bash > /etc/bash_completion.d/pinner
```

## Configuration ⚙️

Pinner can be configured via a `.pinner.toml` file in your repository root.

```toml
# List of actions to ignore during pinning/upgrading
ignore = ["my-org/private-action"]

# Number of concurrent API requests (default: 10)
concurrency = 5

# Custom API URLs (for Enterprise instances)
github_url = "https://github.mycompany.com/api/v3"
gitlab_url = "https://gitlab.mycompany.com/api/v4"

# Vetted (trusted) dependency hashes/references (Whitelist)
# Supports plain strings or structured maps with tag versions and timestamps of insertion.
vetted = [
    # Plain string format (backwards compatible)
    "actions/checkout@692973e3d937129bcbf40652eb9f2f61becf3332",

    # Structured format generated automatically during "scan"
    { ref = "actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10", tag = "v6.0.3", timestamp = "2026-06-18T15:28:25Z" }
]

# Compromised dependency hashes/references (Blacklist)
compromised = [
    "actions/checkout@badhash1234567890badhash1234567890bad",
    { ref = "actions/checkout@evilhash1234567890evilhash1234567890bad", tag = "v3.1.0", timestamp = "2026-06-18T15:28:25Z" }
]

# Disable visual security feedback (default: false)
no_security_feedback = false
```

## Global Configuration & Overrides 🌍

Pinner automatically loads security configurations from global user locations, allowing you to share whitelists/blacklists across projects:
1. `~/.cache/pinner/config.toml` (Global cache file)
2. `~/.config/pinner/config.toml` (User configuration file)
3. `~/.pinner.toml` (Home directory configuration file)

**Precedence (Local Overrides)**:
The local project-level `.pinner.toml` works as a strict override. If a dependency is marked `vetted` locally, it will override any global `compromised` status, and if marked `compromised` locally, it overrides any global `vetted` status. Non-conflicting items are combined automatically.


## Supported Platforms 🌐

Pinner supports multiple CI/CD and git hosting platforms:

| Forge / Platform | Syntax Examples | Env Var for Token |
|-------|---------|-------------------|
| GitHub | `actions/checkout@v4` | `GITHUB_TOKEN` |
| GitLab | `include: project: 'org/repo'` | `GITLAB_TOKEN` |
| Bitbucket | `pipe: atlassian/aws-s3-deploy:1.0.0` | `BITBUCKET_TOKEN` |
| Forgejo/Gitea | `uses: forgejo/action@v1` | `FORGEJO_TOKEN` |
| **Azure Marketplace** | `task: NodeTool@0` | `GITHUB_TOKEN` (via monorepo) |
| **AWS ECR** | `image: <acc>.dkr.ecr.<reg>.amazonaws.com/repo:v1` | `PINNER_OCI_PASSWORD` |
| **CircleCI** | `image: cimg/node:16` | (Public images only) |

### AWS ECR Authentication
To pin private AWS ECR images, provide the authentication token generated by the AWS CLI:
```bash
PINNER_OCI_USERNAME=AWS PINNER_OCI_PASSWORD=$(aws ecr get-login-password --region us-east-1) pinner pin
```

### CircleCI Support
Pinner explicitly supports pinning **CircleCI Docker Images** (e.g., `cimg/*`) to their immutable digests. CircleCI Orbs are not hash-pinned as they use a centralized semantic versioning registry.

## CI/CD Integration 🤖

Add this to your workflow to ensure all actions stay pinned using the native GitHub Action:

```yaml
jobs:
  verify-pinning:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@8f4b7f84864484a7bf31766abe9204da3cbe65b3 # v4
      - name: Verify Pinning
        uses: ffalcinelli/pinner/action@main
        with:
          command: 'verify'
```

> [!TIP]
> **Pinnerception Warning 🌀**
> Remember to pin the pinner! Trusting a security tool to verify your pinned dependencies using a mutable tag is like hiring a security guard who leaves the keys under the doormat. If we didn't pin the pinner, who would pin the pinner's pinners? (Warning: may cause mild existential dread or recursive loops in your CI logs).

Alternatively, you can install and run the CLI directly in any custom pipeline:

```yaml
jobs:
  verify-pinning:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@8f4b7f84864484a7bf31766abe9204da3cbe65b3 # v4
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
