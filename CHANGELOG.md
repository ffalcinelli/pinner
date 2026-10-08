# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- 🔐 **Unsigned images are no longer "compromised"**: `verify --check-osv` reported every image without a cosign signature as a supply-chain attack, and `scan --yes` wrote them to the `compromised` list. They now get a new **unsigned** status: a warning by default, and a failure only with `--strict`. `scan` lists them separately and never writes them to either list.
- ⚙️ **Invalid configuration is an error**: An invalid `.pinner.toml`/`.pinner.yaml` or `PINNER_*` variable was silently replaced by defaults, which dropped the `vetted`/`compromised` lists so `verify` passed. Pinner now stops with an error naming the file. Invalid global files are skipped with a warning.
- ⚖️ **Local overrides now work**: A global `compromised` entry beat a local `vetted` override, contrary to the documented precedence.
- 🐳 **Registries using token challenges**: Anonymous pulls from GHCR, Quay, GCR and other registries failed with HTTP 401, so those images were silently skipped. Pinner now follows the standard `WWW-Authenticate: Bearer` challenge.
- 🧩 **Multi-platform digests**: Manifest requests accept OCI index and Docker manifest-list types first, so tags resolve to the digest that covers all platforms. Requests use `HEAD` (free under Docker Hub pull limits) with a `GET` fallback, and `docker.io/...` names resolve against the correct registry host.
- ⬆️ **Image upgrades**: `upgrade` never refreshed pinned images, because it re-resolved the digest instead of the tag from the version comment. Untagged images (`image: alpine`) are now pinned as `latest` instead of being flagged by `verify` while `pin` skipped them.
- 🏷️ **`name:tag@digest` references**: These are now parsed correctly and keep their inline tag when updated.
- 💬 **Comment handling**: Free-form comments such as `# mainly for X` or `# 3 retries` were mistaken for version comments and truncated, and a preceding `# v1 note` line lost its note. CRLF line endings are now preserved on patched lines.
- 🧪 **Templated values**: Values such as `${{ matrix.image }}` and `$CI_REGISTRY_IMAGE` are skipped instead of being reported as unpinned.
- 🗄️ **Cache isolation**: Disk cache entries are scoped by provider URL, so a GitHub Enterprise repository is never served a SHA cached from github.com.
- 📄 **Report escaping**: JUnit output is XML-escaped, GitHub annotations escape their values, and Markdown table cells escape `|`.
- 🛡️ **Scan safety**: A failed OSV or registry lookup is reported as a warning. Previously it counted as clean, so `scan --yes` vetted the reference.
- CircleCI `volatile` orbs are now reported as unpinned.
- 🔁 **`verify` and `scan` agree**: `verify --check-osv` reported any OSV advisory as a supply-chain attack, while `scan` separated malicious releases from ordinary CVEs, and `scan` ignored the configured `compromised` list (offering blacklisted commits for vetting). Both now share one OSV classifier, which also recognizes OSV `MAL-` advisories. `verify` has a new **vulnerable** status, and the JSON output has a `vulnerable` list. Advisory IDs appear in every report format. Vulnerable commits still fail `verify`, as before; vet them to accept them.
- 🌐 The OSV client now sends a user-agent and retries transient failures.

### Changed
- 💾 **Atomic writes**: Patched files are written to a temporary file and renamed into place, which preserves permissions and symlinks.
- ⚡ **Concurrent security checks**: `verify --check-osv` and `scan` run OSV/provenance lookups and upgrade-candidate resolution concurrently, deduplicated by reference.
- 🧱 **Internals**: Shared SHA/digest helpers in `core`. `verify` output moved to a new `patcher::report` module. Upgrade-candidate selection is deduplicated. Configuration is loaded once and passed to the new `run_with_config`.
- 🌍 **`PINNER_NO_GLOBAL_CONFIG`**: New variable that skips global configuration files. It replaces a test-only check based on the executable path.
- `vetted`/`compromised` entries accept `reference` as an alias of `ref`.

## [0.0.17] - 2026-09-24

### Added
- 🏷️ **Tag Override for Set Command**: Added `--tag` (`-t`) option to `pinner set` to specify a custom tag comment (e.g., `--tag v4.0.0`) while setting commit SHA-1 hashes, preserving existing comments if omitted.
- ☸️ **Kubernetes & Composite Actions Support**: Added automatic scanning and pinning for Kubernetes manifests and composite GitHub Actions (`action.yml`, `action.yaml`).

### Performance
- ⚡ **Dry-Run Memory Optimization**: Avoided unnecessary file content cloning during dry-run executions when files are unmodified.

### Changed
- 🤖 **Consolidated AI Context**: Merged `GEMINI.md` and `JULES.md` into a single canonical `AGENTS.md` context document.
- 🎨 **Top-Level Error Diagnostics**: Formatted top-level CLI error outputs using alternate display formatting and stripped redundant wrapping from `PathNotFound` errors for cleaner diagnostics.
- 🛠️ **Dependency Bumps**: Upgraded `reqwest` to `0.13.5`, `dirs` to `7.0.0`, `toml` to `1.1.6`, `clap_complete` to `4.6.11`, and updated CI actions.

### Fixed
- 🛡️ **Hook Installation Security**: Resolved TOCTOU race vulnerability during git pre-commit hook file creation in `install-hook`.

## [0.0.16] - 2026-09-09

### Changed
- 🛠️ **Dependency Bumps**: Upgraded `tree-sitter` to `0.27.0`.
- 📖 **Documentation**: Added comparison guide between `pinner verify` GitHub Action and GitHub's built-in immutable action settings.

### Fixed
- 🚀 **Release Automation Safety**: Enforced pre-flight remote tracking verification, race-condition guards during test execution, and atomic pushes (`--atomic`) in `scripts/release.sh` to prevent branch desynchronization and orphan release tags.

## [0.0.15] - 2026-09-09

### Added
- ⌨️ **CLI Command Aliases**: Added ergonomic shorthand aliases for frequently used subcommands: `up` (`upgrade`), `check` (`verify`), `sbom` (`export-sbom`), and `pr` (`pr-create`).
- 💡 **Actionable Verification Hints**: Added styled, actionable hints when `verify` fails, guiding users toward running `pinner pin` or checking options with `pinner verify --help`.

### Performance
- ⚡ **Patcher Allocation & Clone Elimination**: Avoided expensive string cloning and heap allocations during patch calculation by transferring ownership of file contents and using `HashMap::remove`.
- ⚡ **Pipeline Scan Optimization**: Eliminated redundant struct clones in intermediate scan target collections and streamlined deduplication using `DependencyName`.
- ⚡ **Zero-Allocation Formatting**: Replaced intermediate heap allocations (`push_str(&format!(...))`) with direct in-place buffer writing (`write!` and `writeln!`) across pipeline reporting, config generation, rate-limit parsing, and `format_security_list`.

### Changed
- 🎨 **CLI Help Text & UX Polish**: Added comprehensive `about` and `long_about` descriptions to the root CLI command, standardized argument help text punctuation, and styled warning prefixes across the pipeline.
- 🛠️ **Dependency Bumps**: Upgraded `base64` to `0.23.1`, `similar` to `3.2.0`, `tree-sitter` to `0.26.13`, `moka` to `0.12.16`, `ignore` to `0.4.33`, `globset` to `0.4.20`, `toml` to `1.1.5`, and `futures` to `0.3.34`. Upgraded CI toolchain and `taiki-e/install-action` to `2.87.6`.

### Fixed
- 🛡️ **Secure Configuration File Permissions**: Prevented TOCTOU vulnerabilities and exposure of sensitive credentials (e.g., API tokens or OCI passwords) by enforcing strict `0o600` permissions on Unix platforms directly upon `.pinner.toml` creation in `init` and `scan`.
- 🛡️ **Vulnerability Resolution**: Updated dependencies to address vulnerability advisories in `h2` and resolved yanked `chacha20` crate.

## [0.0.14] - 2026-08-12

### Added
- 🎨 **Top-Level Error Formatting**: Applied styled color palettes to top-level unhandled CLI errors for clearer diagnostic feedback.

### Performance
- ⚡ **Concurrent Vulnerability Scanning**: Parallelized network requests during OSV vulnerability scans (`scan` subcommand) for faster workflow auditing.
- ⚡ **Allocation Optimizations**: Optimized string and buffer allocations during diff formatting rendering and dependency filtering.

### Changed
- 🧪 **Expanded Test Suite**: Added comprehensive unit and integration tests covering core pipeline operations (`pin`, `upgrade`, `scan`), patcher mutations (`apply_changes`), rate-limiting middleware (`RateLimitMiddleware`), configuration merging (`merge_all`), serialization (`JsonOutput`), and utility functions.
- 🧹 **Internal Refactoring**: Modularized `ProviderRegistry::new`, `init_project_internal`, `merge_with_cli`, and `CachedProvider` disk cache operations into smaller helper functions.
- 🛠️ **Dependency Bumps**: Upgraded `tokio`, `tree-sitter`, `clap`, `thiserror`, `serde`, `serde_json`, `regex`, `serial_test`, `anyhow`, and updated CI action dependencies.

### Fixed
- 🛡️ **Git Command Security**: Resolved command injection risks in git operations and `git push` by introducing `--` argument separators and sanitizing shell calls.
- 🐛 **Configuration File Parsing**: Fixed unhandled parsing errors when loading `.pinner.toml`.
- 🐛 **Docker Image Parsing**: Fixed a potential panic in Docker/OCI image path parsing.

## [0.0.13] - 2026-07-08

### Added
- 🚀 **Auto-Mitigation & PR Creation**: Added a new `pr-create` subcommand to automatically run pinning on workflows, create a git branch, commit changes, push to the remote repository, and open a Pull Request (GitHub) or Merge Request (GitLab) via REST API.
- 💬 **Preceding Comments Support**: Added surgical updating of version-only preceding line comments (e.g. `# v1` -> `# v2`) above workflow dependencies.
- 📊 **SBOM Exporting**: Added an SBOM exporter (`export_sbom`) to output dependencies in CycloneDX format.
- 📡 **Enterprise & Test API Overrides**: Added `PINNER_GITHUB_URL` and `PINNER_GITLAB_URL` environment variable overrides to allow target host configuration (including Mockito testing).
- 🏷️ **Dynamic Release Version Badge**: Replaced the static release tag in documentation and landing pages with an asynchronous lookup to the GitHub Releases API (with instant local fallback).

### Changed
- 🧪 **Pipeline Test Coverage Boost**: Added robust unit and integration testing blocks for Git PR creation flow, project initialization (`init`), and SBOM generation, boosting total line coverage to `87.15%` (logical ~100% coverage ceiling).

### Fixed
- 🛡️ **Vulnerability Resolution**: Upgraded `crossbeam-epoch` to `0.9.20` to resolve pointer dereferencing vulnerability `RUSTSEC-2026-0204`.

## [0.0.12] - 2026-06-27

### Added
- 💾 **Cache Controls Override**: Added `--no-cache` global flag (env `PINNER_NO_CACHE`) to completely bypass persistent cache and `--cache-ttl` global option (env `PINNER_CACHE_TTL`) to customize cache validation duration.
- 🚫 **Mutual Parameter Validation**: Validated that `--no-cache` and `--cache-ttl` cannot be used together.

### Changed
- 🛠️ **Dependency Bumps**:
  - Upgraded `toml_edit` to `0.25.12+spec-1.1.0`.
  - Upgraded `actions/checkout` from `v6.0.3` to `v7.0.0` in CI/CD workflows.
  - Upgraded `taiki-e/install-action` from `2.82.0` to `2.82.2`.
- 📖 **Documentation Improvements**: Refactored landing pages using reusable custom Web Components, extracted shared styles, aligned content with version 0.0.12, and optimized SEO metadata.

### Fixed
- 🛡️ **Vulnerability Resolution**: Upgraded `quinn-proto` to `0.11.15` to resolve security vulnerability `RUSTSEC-2026-0185`.

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
