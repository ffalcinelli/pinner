# Pinner: Developer & Extension Guide

This guide describes how to compile, test, maintain, and extend the `pinner` utility.

---

## Technical Stack & Tooling

*   **Language**: Rust (2021 Edition).
*   **Concurrency**: `tokio` for async orchestration; `rayon` for data-parallel AST scanning.
*   **AST Parsing**: `tree-sitter` and `tree-sitter-yaml` for syntax tree traversal.
*   **Networking**: `reqwest` + `reqwest-middleware` + `reqwest-retry`.
*   **Unit Mocking**: `mockall` (generates mock implementations of traits).
*   **HTTP Interception**: `mockito` (starts local HTTP servers to intercept backend API calls).
*   **Hooks**: `cargo-husky` automatically configures local Git hooks during build.

---

## Workflow Commands

```bash
# Build the binary
cargo build

# Run the CLI locally
cargo run -- --workflows .github/workflows pin

# Run all unit and integration tests
cargo test

# Run linter checks
cargo clippy -- -D warnings

# Check code formatting
cargo fmt -- --check
```

### Git Hooks (via `cargo-husky`)
*   **Pre-commit hook**: Checks/formats code with `cargo fmt`.
*   **Pre-push hook**: Executes `cargo clippy` and `cargo audit` to detect dependency vulnerabilities.

---

## Testing Architecture

`pinner` has a strict test suite achieving high logical coverage:

### 1. Unit Tests
*   Located directly inside the respective source files within a `tests` module block (e.g., at the end of `src/resolver/provider.rs`).
*   Mock providers are auto-generated using `#[cfg_attr(test, mockall::automock)]` annotations on traits like `RemoteProvider`.

### 2. Integration Tests
*   Located under the `tests/` directory (`tests/cli.rs` and `tests/integration.rs`).
*   **Mockito HTTP Mocks**: Starts standard async servers using `mockito::Server::new_async()`. The mock server URLs are injected into the CLI configuration args (e.g. `--github-url`) to redirect remote requests.
*   **Sandbox Isolation**: Uses the `tempfile::tempdir` crate to create temporary workspace environments, avoiding interference with actual system configuration files.
*   **Sequential Run**: Integration tests are annotated with `#[serial_test::serial]` to prevent port conflicts or overlapping resources during async execution.

---

## How to Extend Pinner

### 1. Adding a new CI Provider (Scanner Phase)
If you want `pinner` to parse a new CI platform's configuration files:
1.  **Add Enum Variant**: In `src/core/dependency.rs`, add the new platform to `CiProvider`.
2.  **Add File Path Heuristic**: In `src/scanner/parser.rs`, update `CiProvider::from_path` to map the file directory layout (e.g., `.circleci` -> `CiProvider::CircleCI`).
3.  **Define YAML Keys**: Update `CiProvider::supports_key` to define which keys contain dependency tags (e.g. `uses`, `image`).
4.  **Extend AST Query**: If the new keys are not handled by the default `USES_QUERY` matching expression, update the tree-sitter query string in `src/scanner/parser.rs`.

### 2. Adding a new Remote Resolution Provider (Resolver Phase)
If you want to resolve tags on a new repository platform (e.g., Gitea, Sourcehut):
1.  **Create Provider Module**: Create a struct (e.g., `ReqwestMyPlatformProvider`) inside `src/resolver/`.
2.  **Implement Trait**: Implement the `RemoteProvider` trait for your struct (from `src/resolver/provider.rs`).
3.  **Register Provider**:
    *   Add your provider to `ProviderRegistry::new` inside `src/resolver/provider.rs`.
    *   Configure its routing rules in `ProviderTypeInfo` by specifying the target **domain names** (e.g. `myplatform.org`) and unique **YAML keys** it supports.
