## 2024-05-18 - Clap command aliasing for UX
**Learning:** You can easily add shorthand aliases to CLI commands using `#[command(alias = "shortcut")]` in Clap without breaking backward compatibility for scripts relying on the full command name.
**Action:** When working on CLI subcommands with long names (like `verify` or `export-sbom`), proactively add short, intuitive aliases (like `check` or `sbom`) to improve developer ergonomics and typing speed for everyday users.

## 2024-05-18 - Stripping error wrapping for top-level path errors
**Learning:** Top-level errors triggered by incorrect user input like an invalid path (e.g. `pinner verify -w does_not_exist`) can produce verbose, wrapped error chains (e.g. `error: Failed to run pinner: Path not found: ...`). This is noisy and unfriendly.
**Action:** When printing `anyhow::Error` at the top level, specifically catch cases like `PathNotFound` (which result from direct invalid input) and unwrap the error to print just the root cause, discarding the intermediate "Failed to run pinner" context. This provides a simpler, cleaner error message.
## 2024-10-04 - Actionable error hints for missing paths
**Learning:** Generic "Path not found" errors are often unhelpful, especially when relying on default fallback paths (like `.github/workflows`). A missing path might indicate the user is in the wrong directory or forgot to specify the `-w` flag.
**Action:** Intercept `PinnerError::PathNotFound` in the main error handler and conditionally append a contextual `"hint:".blue()` message based on whether the failing path is a known default (suggesting the user isn't in a repository root) or a custom one.
