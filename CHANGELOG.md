# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.11] - 2026-06-20

### Added
- 🛡️ **Vulnerability and Strictness Checks in Verification**: Added `--check-osv` and `--strict` options to the `verify` command to query the OSV database for known vulnerabilities/compromised hashes and fail verification if dependencies are not explicitly vetted.
- 📁 **Global Configuration Support**: Automatically load and merge user configurations from global paths (e.g., config and home directories).
- 🔑 **CircleCI Token Configuration**: Added support for the `CIRCLECI_TOKEN` environment variable to configure the GraphQL API token.

### Changed
- 🛠️ **CLI Scoping and Refinements**:
  - Relocated `--upgrade-strategy` from a global option to a subcommand-specific argument for the `upgrade` and `scan` commands.
  - Added mutual exclusion check between `--quiet` and `--verbose`.
  - Added mutual dependency validation for `--oci-username` and `--oci-password`.
  - Deprecated and removed the global `--json` flag (use `--format json` instead).
  - Aligned environment variables for OCI registry to use `PINNER_OCI_USERNAME` and `PINNER_OCI_PASSWORD`.
- 📦 **Compact Configuration Format**: Serialized the `.pinner.toml` config's `vetted` and `compromised` security lists as compact inline arrays instead of verbose tables.

### Fixed
- 🐳 **Docker Port Registry Parsing**: Fixed a parsing issue where Docker image tags containing registry hosts with custom ports (e.g. `localhost:5000/my-image:v1.0.0`) were parsed incorrectly.
- 🧪 **Offline Mode Safeguards**: Added validation checks to prevent running online-only operations like OSV checks and scans in offline mode.

## [0.0.10] - 2026-06-19

### Added
- 💾 **Persistent Disk Caching**: Added persistent disk caching via `cacache` to drastically reduce API requests across runs.
- 🛡️ **OCI Provenance Verification**: Implemented OCI image provenance verification (Sigstore/Cosign structural integration).
- 🔑 **OCI Credential Lookup**: Added automatic OCI credential lookup using `docker-credential-helpers`.
- ☁️ **AWS ECR & Azure Marketplace**: Added AWS ECR and Azure Marketplace resolvers, and optimized CircleCI support.
- 🌀 **CircleCI Orb Upgrades**: Added support for upgrading CircleCI orbs via GraphQL API.
- 🐚 **Auto-Shell Detection**: Implemented automatic shell detection for the `generate-completion` command.
- 🚀 **Release Automation**: Added a reliable `scripts/release.sh` utility to automate version bumping, verification, and tagging.
- 🛡️ **Tag Safety Verification**: Added CI step to prevent releasing tags that do not match the version specified in `Cargo.toml`.

### Changed
- 📱 Improved mobile responsiveness of the documentation landing site.
- 📖 Aligned documentation and README with actual CLI subcommands.
- ⚙️ Enhanced GitLab, Forgejo, and CircleCI provider configurations.
- 🧹 Cleaned up dependencies and updated `deny.toml` rules.

### Fixed
- 🔗 Fixed broken license badge and updated badge style.
- 🐛 Fixed bugs in repository/tag resolution and version tag comparisons.
- 🧪 Fixed unused variable warnings in tests.

## [0.0.6] - 2026-06-16

### Added
- Git pre-commit hook installation via `pinner install-hook`.

### Changed
- ⚡ **Performance**: 30x speedup in YAML parsing by caching `TSParser` in thread-local storage.
- ⚡ **Performance**: Optimized concurrent execution by wrapping Rayon calls in `tokio::task::spawn_blocking`.
- Improved error handling for `ReqwestGithubProvider` and better retry policies.
- Enhanced GitLab project resolution and added exhaustive unit tests.

### Fixed
- Fixed a panic during reqwest client initialization on some platforms.
- Fixed git hook installation when `.git/hooks` directory is missing.

## [0.0.5] - 2026-06-13

### Changed
- Modernized landing page and documentation to reflect multi-forge support.
- Grouped CLI options into configuration structs to address architectural concerns.
- Synchronized installation commands across all platforms for better reliability.

## [0.0.4] - 2026-06-13

### Added
- New `verify` subcommand to ensure all actions in workflows are correctly pinned (ideal for CI).
- New `generate-completion` subcommand to generate shell autocompletion scripts.
- Support for `.pinner.toml` configuration file for repo-wide settings (ignore lists, concurrency, custom URLs).
- Support for GitHub Enterprise via `--github-url` flag or `GITHUB_URL` environment variable.
- New `--json` output flag for machine-readable results.
- Advanced `upgrade` strategies: `latest`, `major`, `minor`, and `commit`.
- Support for pinning Docker-based actions (`docker://...`).

### Changed
- Migrated from Regex-based parsing to `tree-sitter-yaml` for surgical precision and better comment/formatting preservation.
- Improved progress reporting with multi-threaded execution.
- Enhanced CLI with more descriptive help and global flags.
- Refactored core logic to support multiple git forges (GitHub, GitLab, Bitbucket, Forgejo).
- Deduplicated HTTP client and improved error handling across all repository providers.
- Increased overall test coverage to >90%.

## [0.0.3] - 2026-06-10

### Fixed
- CI: Refactored release workflow to prevent race conditions in parallel jobs by using a dedicated `create-release` job.

## [0.0.2] - 2026-06-09

### Fixed
- CI: Removed unsupported FreeBSD targets from release matrix.
- CI: Fixed `upload-rust-binary-action` version and archive naming.

## [0.0.1] - 2026-06-09

### Added
- Initial release of `pinner`.
- Core pinning logic using Regex-based parsing to preserve YAML comments and formatting.
- `pin` subcommand to convert mutable tags to immutable commit SHAs.
- `upgrade` subcommand to update actions to their latest release or commit.
- `set` subcommand to forcibly update a specific action across all workflows.
- Comprehensive test suite with offline mocking and HTTP interception.
- GitHub API integration with rate limit handling via `GITHUB_TOKEN`.
- Support for multiple workflow paths and dry-run mode.
