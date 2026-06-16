# Pinner Project Context

## Project Overview
`pinner` is a high-performance Rust CLI utility designed to hash-pin actions and Docker images in CI/CD workflow files. It automates the security best practice of replacing mutable tags (like `@v1` or `:latest`) with immutable commit SHA-1 hashes or digest hashes to prevent supply chain attacks. It supports multiple platforms including GitHub, GitLab, Bitbucket, Forgejo, and OCI registries.

- **Status**: Production-ready core with comprehensive tests and multi-platform support.
- **Architecture**: A strict Domain-Driven Pipeline separated into three phases:
  1. **Scanner**: Traverses files concurrently and uses `tree-sitter-yaml` for AST-based parsing, returning pure `UpdateTask` domain models.
  2. **Resolver**: Maps tasks to immutable hashes concurrently using `Reqwest`-based specialized clients (`github`, `gitlab`, `registry`, etc.) governed by a `UnifiedProvider`.
  3. **Patcher**: Surgically applies string mutations to preserve exact formatting and YAML comments, then handles file writing and diff formatting.
- **Dependency Injection**: Network clients and registries are heavily trait-based (`RemoteProvider`, `RegistryProvider`) allowing for 100% offline unit testing via `mockall`.

## Technology Stack
- **Language**: Rust (2021 Edition)
- **CLI**: `clap` (v4 with derive)
- **Runtime**: `tokio` (Async)
- **HTTP**: `reqwest` with `reqwest-middleware` and `reqwest-retry`.
- **Parsing**: `tree-sitter` and `tree-sitter-yaml`.
- **Caching**: `moka` for API response caching.
- **Error Handling**: `anyhow` for application-level context and `thiserror` for domain-specific errors.
- **Testing**: `mockall` (Mocking), `mockito` (HTTP Interception), `tempfile` (File system isolation), `serial_test`.
- **Automation**: `cargo-husky` for local git hooks.

## Building and Running
- **Build**: `cargo build`
- **Run**: `cargo run -- [subcommand]`
- **Test**: `cargo test`
- **Lint**: `cargo clippy`
- **Coverage**: `cargo tarpaulin` (Note: ~88% reported due to macro instrumentation, 100% logical coverage achieved).

## Git Hooks
- **Managed by**: `cargo-husky`
- **Pre-commit**: Runs `cargo fmt` to ensure consistent formatting.
- **Pre-push**: Runs `cargo clippy` and `cargo audit` to ensure code quality and security before pushing.
- **Customization**: Hooks are defined in `.cargo-husky/hooks/`.

### Subcommands
- `pin`: Automatically converts all action tags and container images to hashes.
- `upgrade`: Upgrades actions to newer versions based on the selected strategy (latest, major, minor, or commit).
- `verify`: Checks if all actions/images are pinned to hashes.
- `install-hook`: Installs a git pre-commit hook that runs `verify`.
- `set <action> <hash>`: Forcibly updates a specific action across all workflows to a provided SHA.
- `generate-completion`: Generates shell completions for bash, zsh, fish, etc.

### Global Options
- `--workflows` (`-w`): Specify one or more files or directories to process. Defaults to standard CI paths.
- `--yes` (`-y`): Automatically confirm all replacements.
- `--dry-run`: Show diff without writing changes.
- `--quiet` (`-q`): Suppress console output.
- `--verbose`: Enable debug logging.
- `--json`: Output results in JSON format.

## Development Conventions
- **Clean Code**: Logic is decoupled from side effects.
- **Safety**: API requests include mandatory User-Agents and follow retry policies.
- **Tag Preservation**: Replacements must append the original tag as a comment (e.g., `@hash # v2`).
- **Error Handling**: Uses `anyhow::Result` for application flow and `PinnerError` for specific failure modes.
