# AI Agent Context: Pinner

Welcome, AI Agent! This document provides the essential project context, architectural guidelines, development workflows, and testing instructions for the `pinner` repository.

---

## 1. Project Overview
`pinner` is a high-performance Rust CLI utility designed to secure CI/CD workflows (GitHub Actions, GitLab CI, Bitbucket Pipelines, Forgejo/Gitea, CircleCI, Azure DevOps, AWS CodeBuild, Tekton, Kubernetes) by automatically pinning mutable dependency tags (like `@v1` or `:latest`) to their immutable cryptographic references (SHA-1 hashes or OCI image digests).

### Key Features:
- **Zero-Regress Formatting**: Modifies workflow files surgically using AST-based byte offsets to preserve exact spacing, indentation, and trailing comments.
- **Multi-Platform Support**: Works with GitHub, GitLab, Bitbucket, Forgejo/Gitea, CircleCI, Azure DevOps, AWS CodeBuild, Tekton, Kubernetes, and any standard OCI container registry.
- **Security Vetting**: Highlights vetted or compromised hashes in terminal diffs using inline tags.
- **Two-Tier Caching**: Combines memory caching (`moka`) and disk caching (`cacache`) with offline fallback support.

---

## 2. Technology Stack & Key Dependencies
- **Language**: Rust (2021 Edition, MSRV 1.80)
- **CLI Framework**: `clap` (v4 with derive)
- **Runtime**: `tokio` (Async orchestration) + `rayon` (Data-parallel AST parsing)
- **Syntax Parsing**: `tree-sitter` and `tree-sitter-yaml`
- **HTTP client**: `reqwest` + `reqwest-middleware` + `reqwest-retry`
- **Caching**: `moka` (in-memory) and `cacache` (on-disk)
- **Testing**: `mockall` (unit mocks), `mockito` (HTTP mocking), `tempfile` (sandboxing), `serial_test` (sequential execution)

---

## 3. Workflow Commands
Always run these commands from the root directory to verify your changes:

```bash
# Build the binary
cargo build

# Run unit and integration tests
cargo test

# Run code linter
cargo clippy -- -D warnings

# Check code formatting
cargo fmt -- --check

# Run the local pinner CLI
cargo run -- --workflows .github/workflows verify
```

### Supported CLI Subcommands:
- `pin`: Surgically replaces mutable dependency tags with immutable hashes.
- `upgrade`: Upgrades pinned actions/images based on strategy (latest, major, minor, commit).
- `verify`: Verifies all dependencies are pinned, optionally checking OSV (`--check-osv`) or enforcing strict rules (`--strict`).
- `set`: Forcibly sets an action to a specific commit SHA.
- `install-hook`: Installs a git pre-commit verification hook.
- `init`: Initializes a default configuration file (`.pinner.toml`).
- `export-sbom`: Exports SBOM metadata for CI dependencies.
- `scan`: Scans workflows and interactively updates `.pinner.toml` with OSV auditing feedback.
- `pr-create`: Automates git committing, branch creation, pushing, and PR/MR opening.
- `generate-completion`: Generates tab completion scripts for shells.


---

## 4. Architecture & Domain Pipeline
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
- **Traversal (`walker.rs`)**: Uses the `ignore` crate to traverse directories concurrently, honoring `.gitignore` files. Rayon parses files in parallel using thread-local tree-sitter parsers.
- **Parsing (`parser.rs`)**: Uses `tree-sitter-yaml` to construct concrete syntax trees. Captures targets using a tree-sitter AST query (e.g. `uses`, `image`, `ref`, `pipe`, `orbs`).
- **Trailing Comments**: If a comment resides on the exact same line as a dependency node, it is captured (`UpdateTask::comment`).
- **GitLab Special Cases**: Resolves nested structure map references by looking up sibling nodes (combining `project` + `ref` keys into a virtual dependency string).

### B. Resolver Phase (`src/resolver/`)
- **Traits (`provider.rs`, `registry.rs`)**: Highly modular dependency injection via `RemoteProvider` (for code repos) and `RegistryProvider` (for container images).
- **CachedProvider**: A decorator that intercepts queries, caching lookups in `moka` and `cacache`. Bypasses requests if `offline` mode is enabled.
- **Batching & Concurrency**: Groups identical requests (e.g. 15 references of `actions/checkout@v3` are resolved once). Uses `futures::stream::StreamExt::buffer_unordered` to enforce concurrency limits.

### C. Patcher Phase (`src/patcher/`)
- **Surgical Mutator (`mutator.rs`)**: Overrides dependency tags surgically by applying string slicing based on AST byte offsets. **Never re-serialize the entire YAML** to avoid altering custom formatting.
- **Reverse Offset Execution (`disk.rs`)**: Modifies files starting from the **highest byte offset to the lowest**. This ensures upstream byte offsets remain valid even as insertions/deletions shift the file length downstream.
- **Comment Preservation**: Detects mutable versions in existing comments using `COMMENT_REGEX`, keeping other user annotations.
- **Security Check (`formatter.rs`)**: Validates SHAs against `vetted` and `compromised` groups in `.pinner.toml`, writing green/red annotations next to diff lines.

---

## 5. Coding & Contribution Rules
When making changes, please adhere to these design rules:

1. **No Complete Re-serialization**: Never parse YAML into a hashmap and write it back. Use the surgical offset-based patcher (`src/patcher/mutator.rs`).
2. **Error Handling**: Use `anyhow::Result` for CLI commands, and domain-specific errors via `thiserror` (e.g., `PinnerError`) in the core layers.
3. **Decoupling**: Keep domain structures (`src/core/`) completely free of network logic and side-effects.
4. **API Requests**: All outgoing HTTP requests must include a user-agent and abide by standard retry policies.
5. **Testing Requirements**:
   - Write unit tests for new logic directly in the target file.
   - Use `mockall` for trait mocking in unit tests.
   - Integration tests in `tests/` must use `mockito` for API interception and `tempfile::tempdir()` for filesystem isolation.
   - Annotate integration tests with `#[serial_test::serial]` to prevent async concurrency collisions.
6. **Documentation & Markdown Update**: Whenever you modify the codebase (such as adding/modifying subcommands, altering pipeline behavior, or changing configuration parameters), you must always update the corresponding markdown documentation (e.g., `README.md`, `docs/`, `docs/llm/`) to keep all documentation accurate and synchronized.

