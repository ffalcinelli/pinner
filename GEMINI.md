# Pinner Project Context

## Project Overview
`pinner` is a high-performance Rust CLI utility designed to hash-pin GitHub Actions in workflow files. It automates the security best practice of replacing mutable tags (like `@v1`) with immutable commit SHA-1 hashes (e.g., `@a1b2c3d...`) to prevent supply chain attacks.

- **Status**: Production-ready core with comprehensive tests.
- **Architecture**: Separated into a testable library (`src/lib.rs`) and a thin CLI wrapper (`src/main.rs`).
- **Parsing Strategy**: Uses Regex-based surgical replacement to ensure 100% preservation of YAML comments, indentation, and formatting.
- **Dependency Injection**: Uses the `GithubProvider` trait to allow full offline testing via `MockGithubProvider`.

## Technology Stack
- **Language**: Rust (2021 Edition)
- **CLI**: `clap` (v4 with derive)
- **Runtime**: `tokio` (Async)
- **HTTP**: `reqwest`
- **Testing**: `mockall` (Mocking), `mockito` (HTTP Interception), `tempfile` (File system isolation).
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
- `pin`: Automatically converts all action tags in `.github/workflows/` (or specified paths) to hashes.
- `upgrade`: Re-pins all actions to the latest commit on their `main` branch (or latest release).
- `set <action> <hash>`: Forcibly updates a specific action across all workflows to a provided SHA.

### Global Options
- `--workflow` (`-w`): Specify one or more files or directories to process. Defaults to `.github/workflows/`.
- `--yes` (`-y`): Automatically confirm all replacements.
- `--dry-run`: Show diff without writing changes.
- `--quiet` (`-q`): Suppress console output.

## Development Conventions
- **Clean Code**: Logic is decoupled from side effects.
- **Safety**: GitHub API requests include a mandatory User-Agent.
- **Tag Preservation**: Replacements must append the original tag as a comment (e.g., `@hash # v2`).
- **Error Handling**: Uses `Result<T, String>` for propagation, focusing on clear CLI feedback.
