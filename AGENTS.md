# AI Agent Context: Pinner

Welcome, AI Agent! This document provides the essential project context, architectural guidelines, development workflows, and testing instructions for the `pinner` repository. It is the single canonical reference for all AI pair-programming agents (Antigravity, Jules, Codex, Cursor, etc.).

---

## Detailed LLM Context Documents
To get deep contextual understanding of specific codebase components, refer to these dedicated documentation files:
* **[Architecture & Domain Pipeline](docs/llm/overview.md)**: High-level system goals, core pipeline stages, and file mappings.
* **[Scanner & Tree-Sitter Parser](docs/llm/scanner.md)**: File traversal details, YAML parsing queries, and trailing comment mapping logic.
* **[Resolver & Network Providers](docs/llm/resolver.md)**: Trait specifications, the `CachedProvider` wrapper, and OCI image digest fetching.
* **[Patcher & Surgical Mutation](docs/llm/patcher.md)**: Safe edit sequencing via reverse offset sorting, comment preservation, and security tag diff rendering.
* **[Developer & Extension Guide](docs/llm/development.md)**: Building, linting, unit & integration test strategies, and guidelines for adding new providers.

---

## 1. Project Overview
`pinner` is a high-performance Rust CLI utility designed to secure CI/CD workflows by automatically pinning mutable dependency tags (like `@v1` or `:latest`) to their immutable cryptographic references (SHA-1 hashes or OCI image digests) to mitigate supply chain risks.

- **Status**: Production-ready core with exceptionally high test coverage, comprehensive public API documentation (rustdoc), and detailed internal comments.
- **Dependency Injection**: Network clients and registries are heavily trait-based (`RemoteProvider`, `RegistryProvider`) allowing for extensive offline unit testing via `mockall` and HTTP mocking via `mockito`.

### Key Features:
- **Zero-Regress Formatting**: Modifies workflow files surgically using AST-based byte offsets to preserve exact spacing, indentation, and trailing comments. Never re-serializes YAML.
- **Multi-Platform Support**: Pin dependencies across virtually any CI/CD and container manifest ecosystem:
  - **GitHub Actions**: `uses`, `image`
  - **GitLab CI**: `include`, `image`, `ref`
  - **Bitbucket Pipelines**: `pipe`, `image`
  - **Forgejo / Gitea**: `uses`, `image`
  - **CircleCI**: `image`, `orbs`
  - **Azure DevOps**: `task`, `template`, `image`
  - **AWS CodeBuild**: `image`
  - **Tekton Pipelines**: `bundle`, `image`
  - **Kubernetes Manifests**: `image`
  - **OCI Container Registries**: Any standard registry image reference
- **Security Vetting**: Highlights vetted or compromised hashes in terminal diffs using inline tags, with OSV auditing integration.
- **Two-Tier Caching**: Combines memory caching (`moka`) and disk caching (`cacache`) with offline fallback support.

---

## 2. Technology Stack & Key Dependencies
- **Language**: Rust (2021 Edition, MSRV 1.80)
- **CLI Framework**: `clap` (v4 with derive)
- **Runtime**: `tokio` (Async orchestration) + `rayon` (Data-parallel AST parsing)
- **Syntax Parsing**: `tree-sitter` and `tree-sitter-yaml`
- **HTTP Client**: `reqwest` + `reqwest-middleware` + `reqwest-retry`
- **Caching**: `moka` (in-memory) and `cacache` (on-disk)
- **Error Handling**: `anyhow` for application flow and `thiserror` (e.g. `PinnerError`) for domain-specific errors
- **Testing**: `mockall` (unit mocks), `mockito` (HTTP mocking), `tempfile` (sandboxing), `serial_test` (sequential execution)
- **Git Hooks Automation**: `cargo-husky`

---

## 3. Workflow Commands & Development

### Standard Verification Commands:
Always run these commands from the root directory to verify changes:

```bash
# Build the binary
cargo build

# Run unit and integration tests
cargo test

# Run code linter
cargo clippy -- -D warnings

# Check code formatting
cargo fmt -- --check

# Test coverage analysis
cargo tarpaulin

# Run the local pinner CLI
cargo run -- --workflows .github/workflows verify
```

### Git Hooks
Git hooks are managed via `cargo-husky` (configured in `.cargo-husky/hooks/`):
- **Pre-commit**: Runs `cargo fmt` to enforce formatting consistency.
- **Pre-push**: Runs `cargo clippy` and `cargo audit` to enforce code quality and dependency safety before pushing.

---

## 4. CLI Reference

### Supported CLI Subcommands:
- `pin`: Surgically replaces mutable dependency tags with immutable hashes.
- `upgrade`: Upgrades pinned actions/images based on strategy (`latest`, `major`, `minor`, `commit`).
- `verify`: Verifies all dependencies are pinned, optionally checking OSV (`--check-osv`), enforcing strict rules (`--strict`), and formatting reports (`text`, `json`, `markdown`, `github`, `junit`).
- `set <action> <hash> [--tag <tag>]`: Forcibly sets an action across workflows to a specific commit SHA (preserves tag comments or overrides with `--tag`).
- `install-hook`: Installs a git pre-commit verification hook into `.git/hooks/`.
- `init`: Initializes a default configuration file (`.pinner.toml`).
- `export-sbom`: Exports SBOM metadata for CI dependencies.
- `scan`: Scans workflows and interactively updates `.pinner.toml` with OSV auditing feedback.
- `pr-create`: Automates git committing, branch creation, pushing, and PR/MR opening.
- `generate-completion`: Generates tab completion scripts for shells.

### Global Options:
- `--workflows` (`-w`): Specify one or more files or directories to process. Defaults to standard CI paths.
- `--yes` (`-y`): Automatically confirm all replacements without interactive prompt.
- `--dry-run`: Show diff preview without writing changes to disk.
- `--quiet` (`-q`): Suppress console output.
- `--verbose`: Enable detailed debug logging.

---

## 5. Architecture & Domain Pipeline
The codebase strictly follows a decoupled **Domain-Driven Pipeline**:

```
[ Filesystem ]
      │
      ▼ (Scanner Phase: walker.rs -> parser.rs)
[ UpdateTasks ]
      │
      ▼ (Resolver Phase: unified.rs -> provider.rs / registry.rs)
[ UpdateResults ]
      │
      ▼ (Patcher Phase: mutator.rs -> formatter.rs -> disk.rs)
[ Updated Files on Disk ]
```

### A. Scanner Phase (`src/scanner/`)
- **Traversal (`walker.rs`)**: Uses the `ignore` crate to traverse directories concurrently, honoring `.gitignore` files. Rayon parses files in parallel using thread-local tree-sitter parsers. Automatically discovers workflows, composite actions (`.github/actions/`, `action.yml`, `action.yaml`), and Kubernetes manifests.
- **Parsing (`parser.rs`)**: Uses `tree-sitter-yaml` to construct concrete syntax trees. Captures targets using a tree-sitter AST query (e.g. `uses`, `image`, `container`, `ref`, `pipe`, `orbs`, `bundle`, `task`, `template`).
- **Trailing Comments**: If a comment resides on the exact same line as a dependency node, it is captured (`UpdateTask::comment`).
- **GitLab Special Cases**: Resolves nested structure map references by looking up sibling nodes (combining `project` + `ref` keys into a virtual dependency string).

### B. Resolver Phase (`src/resolver/`)
- **Traits (`provider.rs`, `registry.rs`)**: Highly modular dependency injection via `RemoteProvider` (for code repos) and `RegistryProvider` (for container images).
- **CachedProvider**: A decorator that intercepts queries, caching lookups in `moka` and `cacache`. Bypasses requests if `offline` mode is enabled.
- **Batching & Concurrency**: Groups identical requests (e.g. 15 references of `actions/checkout@v3` are resolved once). Uses `futures::stream::StreamExt::buffer_unordered` to enforce concurrency limits.

### C. Patcher Phase (`src/patcher/`)
- **Surgical Mutator (`mutator.rs`)**: Overrides dependency tags surgically by applying string slicing based on AST byte offsets. **Never re-serialize the entire YAML** to avoid altering custom formatting.
- **Reverse Offset Execution (`disk.rs`)**: Modifies files starting from the **highest byte offset to the lowest**. This ensures upstream byte offsets remain valid even as insertions/deletions shift the file length downstream.
- **Comment Preservation**: Detects mutable versions in existing comments using `COMMENT_REGEX`, keeping other user annotations (e.g. `actions/checkout@<sha> # v3`).
- **Security Check (`formatter.rs`)**: Validates SHAs against `vetted` and `compromised` groups in `.pinner.toml`, writing green/red annotations next to diff lines.

---

## 6. Coding & Contribution Rules
When making changes, please adhere to these design rules:

1. **No Complete Re-serialization**: Never parse YAML into a hashmap and write it back. Use the surgical offset-based patcher (`src/patcher/mutator.rs`).
2. **Tag Preservation**: Replacements must append or maintain the original tag as a trailing comment (e.g., `@hash # v2`).
3. **Decoupling & Clean Code**: Keep domain structures (`src/core/`) completely free of network logic and side-effects. Logic is decoupled from side effects.
4. **Error Handling**: Use `anyhow::Result` for CLI commands, and domain-specific errors via `thiserror` (e.g., `PinnerError`) in the core layers.
5. **API Requests**: All outgoing HTTP requests must include a user-agent and abide by standard retry policies.
6. **Testing Requirements**:
   - Write unit tests for new logic directly in the target file.
   - Use `mockall` for trait mocking in unit tests.
   - Integration tests in `tests/` must use `mockito` for API interception and `tempfile::tempdir()` for filesystem isolation.
   - Annotate integration tests with `#[serial_test::serial]` to prevent async concurrency collisions.
7. **Documentation & Markdown Update**: Whenever you modify the codebase (such as adding/modifying subcommands, altering pipeline behavior, or changing configuration parameters), you must always update the corresponding markdown documentation (e.g., `README.md`, `docs/`, `docs/llm/`) to keep all documentation accurate and synchronized.
