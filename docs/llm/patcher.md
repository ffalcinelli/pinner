# Pinner: Patcher & Mutation Layer

The Patcher layer is responsible for surgically editing workflow files to inject resolved commit SHAs/digests, printing formatting diffs, and managing the writing of changes back to disk.

---

## Surgical String Mutation (`mutator.rs`)

To modify files without changing indentation or breaking comments, `pinner` avoids re-serializing the entire parsed AST to YAML. Instead, it applies surgical string operations on the original content string using byte offsets:

1.  **Line End Capture**: Locates the end of the line containing the dependency value (using `.find('\n')`).
2.  **Comment Processing**:
    *   Uses `COMMENT_REGEX` (`r"^#\s*(v\d[a-zA-Z0-9.\-_]*|main|\d[a-zA-Z0-9.\-_]*)\s*"`) to detect if the existing comment contains a mutable version tag (like `# v1` or `# main`).
    *   If matched, the old version portion is stripped, but any additional annotations in the comment (e.g., `# important comment`) are preserved.
3.  **Separator Formatting**:
    *   **GitHub/Registry**: Uses the `@` separator (e.g., `actions/checkout@<sha>`).
    *   **Bitbucket Pipes**: Uses the `:` separator (e.g., `bitbucket-pipelines:pipe:<sha>`).
    *   **GitLab Ref**: Directly overrides the `ref` value (no symbol prefix).
4.  **Tag Annotation**: Appends the original tag version as a comment next to the SHA (e.g., `actions/checkout@<sha> # v3`). If the original tag is already a SHA or digest, the comment annotation is omitted.

---

## The Reverse Offset Preservation Strategy (`disk.rs`)

When multiple dependencies are updated inside a single file, replacing a tag (like `v3`) with a long hash (like `8f4b7f8885f8f35d21a221f7c35e39626e2e5c8e`) changes the file's overall length. This invalidates the byte offsets of any downstream dependencies.

To solve this, `Patcher::calculate_patches` applies updates in **reverse order of their start byte offset** (`std::cmp::Reverse(a.task.start)`):

```
Original File:
Line 10: uses: actions/checkout@v1   (Offset: 200)
Line 25: uses: actions/setup-node@v2 (Offset: 500)

1. Sort offsets in descending order: [500, 200]
2. First update setup-node at offset 500 -> String length changes.
3. Second update checkout at offset 200 -> Offset remains valid because length changes occurred downstream.
```

---

## Diff Formatting & Security Tags (`formatter.rs`)

`pinner` formats updates for the console, JSON output, or Markdown summaries.

### 1. Diffs using the `similar` crate
Generates standard unified Git diffs (`+` and `-` lines).

### 2. Inline Security Status
When generating diffs, `pinner` cross-references the resolved hash/digest against groups defined in the config:
*   **Vetted**: Explicitly approved hashes.
*   **Compromised**: Hashes identified as containing malicious code or known exploits.
*   **Not Checked**: Hashes that are not classified.

If security feedback is enabled, these statuses are appended inline in the printed terminal diff:
*   `[✓ vetted]` (in bold green)
*   `[✗ compromised]` (in bold red)
*   `[? not checked]` (in yellow)

---

## Interactive UI and Confirmation (`ui.rs`)

Disk writing is protected by user-interaction:
*   **Dry Run**: Outputs diffs to the console without writing changes.
*   **Interactive Confirmation**: If `--yes` (`-y`) is not set, a progress bar/interactive prompt displays each patch's diff and asks the user to confirm application (`[y/N]`) before writing to disk.
*   **Atomic Writes**: Changes are written to the target file only after all patch replacements succeed.
