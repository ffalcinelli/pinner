# Contributing to Pinner 🤝

Thank you for your interest in contributing to Pinner! We welcome all contributions, from bug reports and documentation improvements to new features and performance optimizations.

## Getting Started 🚀

1.  **Fork the repository** on GitHub.
2.  **Clone your fork** locally:
    ```bash
    git clone https://github.com/your-username/pinner.git
    cd pinner
    ```
3.  **Install dependencies**: Ensure you have Rust and Cargo installed (1.70+).
4.  **Create a new branch** for your work:
    ```bash
    git checkout -b feature/my-new-feature
    ```

## Development Workflow 👩‍💻

### Building
```bash
cargo build
```

### Testing
We maintain high test coverage. Please ensure all tests pass and add new tests for any changes.
```bash
cargo test
```

### Linting & Formatting
We follow standard Rust idioms and formatting.
```bash
cargo clippy
cargo fmt
```

### Git Hooks
This project uses `cargo-husky` to run checks automatically.
- **Pre-commit**: Runs `cargo fmt`.
- **Pre-push**: Runs `cargo clippy` and `cargo audit`.

## Pull Request Guidelines 📝

1.  **Surgical Changes**: Keep your PRs focused on a single task.
2.  **Update Documentation**: If you add a new feature or flag, update the `README.md` and `CHANGELOG.md`.
3.  **Add Tests**: PRs without tests are unlikely to be merged.
4.  **Descriptive Commit Messages**: Follow standard commit message conventions.

## Architecture 🏗️

- `src/lib.rs`: Core library logic.
- `src/cli.rs`: CLI argument definition using `clap`.
- `src/operations.rs`: Orchestration of pinning and upgrading.
- `src/yaml.rs`: Tree-sitter based YAML parsing.
- `src/github/`: GitHub API integration.

## Questions? ❓

If you have any questions, feel free to open an issue or start a discussion in the repository.

---

Released under the [MIT License](LICENSE).
