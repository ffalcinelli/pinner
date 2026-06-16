# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
