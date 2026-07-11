# Google Jules Agent Context: Pinner

Welcome, Jules! This file provides the project context, design architecture, development commands, and coding guidelines tailored for your tasks in the `pinner` repository.

> [!NOTE]
> For the universal machine-readable context file, see [AGENTS.md](AGENTS.md).

---

## 1. Project Overview
`pinner` is a high-performance Rust CLI tool that secures CI/CD workflows by replacing mutable tags (like `@v1` or `:latest`) with immutable commit SHAs or image digests.

It supports:
- GitHub Actions (`uses`, `image`)
- GitLab CI (`include`, `image`, `ref`)
- Bitbucket Pipelines (`pipe`, `image`)
- Forgejo / Gitea (`uses`, `image`)
- CircleCI (`image`, `orbs`)
- Azure DevOps (`task`, `template`, `image`)
- AWS CodeBuild (`image`)
- Tekton Pipelines (`bundle`, `image`)
- Kubernetes manifest (`image`)
- OCI container registries

---

## 2. Technical Stack & Commands
- **Language**: Rust (2021 Edition, MSRV 1.80)
- **Frameworks**: `tokio`, `rayon`, `tree-sitter` (`tree-sitter-yaml`), `reqwest`, `moka`, `cacache`

### Key Workflow Commands:
```bash
# Build the project
cargo build

# Run all unit/integration tests
cargo test

# Run code linter
cargo clippy -- -D warnings

# Verify formatting
cargo fmt -- --check
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

## 3. Pipeline Architecture
`pinner` employs a decoupled three-stage pipeline:

1. **Scanner (`src/scanner/`)**: Runs concurrently (via Rayon and tokio blocking tasks) to traverse the workspace and build an AST query of YAML configurations via `tree-sitter-yaml`.
2. **Resolver (`src/resolver/`)**: Groups identical target tasks, queries APIs concurrently with a `UnifiedProvider`, and uses memory/disk caches.
3. **Patcher (`src/patcher/`)**: Applies edits back-to-front based on byte offsets (`mutator.rs`) to prevent offset invalidation, preserves comments, and commits atomically.

---

## 4. Key Rules for Development
- **Do not re-serialize YAML files**. Perform only surgical string slice replacements using the byte offsets from the AST.
- **Maintain comment annotations**. Keep the original version tag as a trailing comment: e.g. `actions/checkout@<sha> # v3`.
- **Use traits for dependency injection**. `RemoteProvider` and `RegistryProvider` are mocked with `mockall` for testing.
- **Isolate tests**. Integration tests must use temporary directories (`tempfile::tempdir()`), intercept HTTP requests with Mockito (`mockito::Server::new_async()`), and run sequentially using the `#[serial_test::serial]` attribute.
- **Update Markdown and Docs**: Always update any relevant markdown documentation (such as `README.md`) and the documents in `docs/` and `docs/llm/` directories whenever modifying codebase structure, subcommands, configuration parameters, or behavior to prevent documentation from going stale.

