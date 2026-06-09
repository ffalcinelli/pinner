# Pinner 🧪

[![CI](https://github.com/fabiofalcinelli/pinner/actions/workflows/ci.yml/badge.svg)](https://github.com/fabiofalcinelli/pinner/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

A high-performance Rust CLI utility to **hash-pin GitHub Actions** in your workflow files. Secure your CI/CD supply chain by converting volatile, mutable tags (like `@v2`) into immutable, cryptographic commit SHAs (like `@df4cb1c...`).

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
- **Batch Processing**: Scans your entire `.github/workflows/` directory in one go.

## Installation 🛠️

```bash
# Clone the repository
git clone https://github.com/fabiofalcinelli/pinner.git
cd pinner

# Build and install
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

### 2. Set a specific action hash
Forcibly updates a specific action across all workflows.
```bash
pinner set actions/checkout df4cb1c069e1874edd31b4311f1884172cec0e10
```

### 3. Upgrade to latest
Re-pins all actions to the latest commit on their `main` branch.
```bash
pinner upgrade
```

## Development 👩‍💻

This project is built with Rust and follows clean code principles.

- **Tests**: `cargo test` (includes unit and integration tests with 95%+ logical coverage).
- **Hooks**: Managed by `cargo-husky` (runs `fmt` and `clippy` on commit).
- **CI**: GitHub Actions workflow runs on every push.

## License 📄

MIT License. See [LICENSE](LICENSE) for details.
