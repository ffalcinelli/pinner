# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
