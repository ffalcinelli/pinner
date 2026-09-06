## 2024-05-18 - Clap command aliasing for UX
**Learning:** You can easily add shorthand aliases to CLI commands using `#[command(alias = "shortcut")]` in Clap without breaking backward compatibility for scripts relying on the full command name.
**Action:** When working on CLI subcommands with long names (like `verify` or `export-sbom`), proactively add short, intuitive aliases (like `check` or `sbom`) to improve developer ergonomics and typing speed for everyday users.
