# Pinner 🧪

[![CI](https://github.com/ffalcinelli/pinner/actions/workflows/ci.yml/badge.svg)](https://github.com/ffalcinelli/pinner/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/ffalcinelli/pinner/graph/badge.svg)](https://codecov.io/gh/ffalcinelli/pinner)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![GitHub release](https://img.shields.io/github/v/release/ffalcinelli/pinner)](https://github.com/ffalcinelli/pinner/releases)
[![Docs](https://img.shields.io/badge/docs-latest-blue.svg)](https://ffalcinelli.github.io/pinner/latest/pinner/index.html)

A high-performance Rust CLI utility to **hash-pin GitHub Actions** in your workflow files. Secure your CI/CD supply chain by converting volatile, mutable tags (like `@v2`) into immutable, cryptographic commit SHAs (like `@df4cb1c...`).

## Documentation 📚

The full documentation for the project is available at:
[https://ffalcinelli.github.io/pinner/](https://ffalcinelli.github.io/pinner/)

## Why Pin? 🔒

Using mutable tags like `@v2` or `@main` in GitHub Actions introduces a security risk. If an attacker gains access to an action's repository, they can move the tag to a malicious commit, leading to a supply chain attack on your infrastructure. 

Hash-pinning ensures that you run the **exact** code you've audited, every single time.

## Name Origin ⚗️

The name **Pinner** is inspired by the **Pinner reaction** in organic chemistry. Discovered by Adolf Pinner, this reaction involves the acid-catalyzed conversion of a reactive nitrile into a highly stable Pinner salt.

Just as the Pinner reaction acts as a catalyst to transform a volatile compound into a stable, fixed salt, this CLI acts as a catalyst for your CI/CD, transforming "floating" action tags into secure, immutable, and fixed commit SHAs.

## Features ✨

- **Safe Replacement**: Uses Regex-based parsing to preserve your YAML comments and formatting perfectly.
- **Tag Preservation**: Automatically appends the original tag as a comment for readability (e.g., `@<hash> # v2`).
- **GitHub API Integration**: Automatically fetches the correct commit SHA for any tag or branch.
- **Batch Processing**: Scans your entire `.github/workflows/` directory by default.
- **Targeted Updates**: Specify exactly which workflow files or directories to process with the `--workflow` flag.

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
# Install via cargo
cargo install pinner

# Alternatively, install via cargo-git
cargo install --git https://github.com/ffalcinelli/pinner.git
```

Alternatively, from source:
```bash
git clone https://github.com/ffalcinelli/pinner.git
cd pinner
cargo install --path .
```

## Usage 🚀

### 1. Pin all actions
Scans `.github/workflows/` and converts all tags to pinned hashes.
```bash
pinner pin
```
*Input:* `- uses: actions/checkout@v3`  
*Output:* `- uses: actions/checkout@8f4b7f84864484a7bf31766abe9204da3cbe65b3 # v3`

### 2. Specify specific workflows
You can target specific files or directories using the `--workflow` (or `-w`) flag.
```bash
# Pin actions in a single file
pinner pin -w .github/workflows/ci.yml

# Pin actions in multiple specific files
pinner pin -w .github/workflows/ci.yml -w .github/workflows/release.yml

# Pin actions in a custom directory
pinner pin -w my-custom-workflows/
```

### 3. Set a specific action hash
Forcibly updates a specific action across all workflows.
```bash
pinner set actions/checkout df4cb1c069e1874edd31b4311f1884172cec0e10
```

### 4. Upgrade to latest
Re-pins all actions to the latest commit on their `main` branch (or the latest release tag if available).
```bash
pinner upgrade
```

### Common Flags
- `--yes` (`-y`): Automatically confirm all replacements.
- `--dry-run`: Print diff without modifying files.
- `--quiet` (`-q`): Suppress all console output.
- `--workflow` (`-w`): Workflow files or directories to process.

## Rate Limiting & Authentication 🔑

Pinner uses the GitHub API to fetch commit SHAs. To avoid hitting rate limits (especially in CI or large projects), you should provide a GitHub token via the `GITHUB_TOKEN` environment variable.

```bash
export GITHUB_TOKEN=ghp_your_token_here
pinner pin
```

The token only needs `read-only` access to public repositories (or `repo` scope for private ones).

## Development 👩‍💻

This project is built with Rust and follows clean code principles.

- **Tests**: `cargo test`
- **Lints**: `cargo clippy`
- **Formatting**: `cargo fmt`
- **Coverage**: `cargo tarpaulin`

## Contributing 🤝

Contributions are welcome! Please feel free to submit a Pull Request or open an issue for any bugs or feature requests.

## License 📄

MIT License. See [LICENSE](LICENSE) for details.
