# Pinner: Patcher & Mutation Layer

The Patcher layer is responsible for surgically editing workflow files to inject resolved commit SHAs/digests, printing formatting diffs, and managing the writing of changes back to disk.

---

## Surgical String Mutation (`mutator.rs`)

To modify files without changing indentation or breaking comments, `pinner` avoids re-serializing the entire parsed AST to YAML. Instead, it applies surgical string operations on the original content string using byte offsets:

1.  **Line End Capture**: Locates the end of the line containing the dependency value (using `.find('\n')`). A trailing `\r` (CRLF files) is left outside the replaced range so line endings are preserved.
2.  **Comment Processing**:
    *   Uses the shared `VERSION_COMMENT_REGEX` from `core/update.rs` (`r"^#\s*(v\d[a-zA-Z0-9.\-_+]*|main|\d[a-zA-Z0-9.\-_+]*)\s*(?:#|$)"`) to detect a version-only comment (like `# v1`, `# main` or `# v1 # note`). The token must be the whole comment or be followed by another `#`, so free-form comments such as `# mainly for X` or `# 3 retries` are never treated as versions.
    *   If matched, the old version portion is stripped, but any additional annotations in the comment (e.g., `# important comment`) are preserved.
    *   A version-only comment on the line directly above the dependency is refreshed too. Only the version token is replaced; the rest of that line and its line ending are kept.
3.  **Separator Formatting**:
    *   **GitHub/Registry**: Uses the `@` separator (e.g., `actions/checkout@<sha>`).
    *   **Bitbucket Pipes**: Uses the `:` separator (e.g., `bitbucket-pipelines:pipe:<sha>`).
    *   **GitLab Ref**: Directly overrides the `ref` value (no symbol prefix).
4.  **Tag Annotation**: Appends the original tag version as a comment next to the SHA (e.g., `actions/checkout@<sha> # v3`). If the original tag is already a SHA or digest, the comment annotation is omitted.
5.  **Inline Image Tags**: Image references written as `name:tag@digest` (`UpdateTask::image_tag`) keep that layout (`alpine:3.20@sha256:<new>`); no version comment is added because the tag is already visible.

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

## Verification Reports (`report.rs`)

`Pipeline::verify` classifies every dependency into a `VerifyFinding` with a `VerifyStatus`:
`Unpinned`, `Compromised`, `Vulnerable`, `Unsigned`, `NotVetted` (strict only), `Pinned` or `Vetted`. `VerifyStatus::is_failure(strict)` decides the exit status: `Unsigned` fails only in strict mode. OSV advisory IDs are carried in `VerifyFinding::advisories`. `scan` uses the same OSV assessment and the same configured `compromised` list, so the two commands always classify a reference the same way. With `--check-osv`, commits are queried in OSV and images are checked for a cosign signature. These checks run concurrently, deduplicated by `(action, reference)`, and a lookup error is a warning that leaves the finding unchanged.

`report.rs` renders findings as strings: `render_text` (stderr), `render_github` (`::error`/`::warning` commands with escaped values), `render_markdown` (escaped table cells) and `render_junit` (XML-escaped). JSON output is the serialized `VerificationResult`, printed by `lib.rs`.

---

## Diff Formatting & Security Tags (`formatter.rs`)

`pinner` formats updates and verification results for the console, JSON output, GitHub Actions workflow annotations, standard JUnit XML, or Markdown tables (ideal for GitHub step summaries).

### 1. Diffs using the `similar` crate
Generates standard unified Git diffs (`+` and `-` lines).

### 2. Inline Security Status & Hash Normalization
When generating diffs or verifying dependencies, `pinner` cross-references the resolved hash/digest against groups defined in `.pinner.toml`:
*   **Vetted**: Explicitly approved hashes.
*   **Compromised**: Hashes identified as containing malicious code or known exploits.
*   **Not Checked**: Hashes that are not classified.

Hash matching automatically normalizes prefixes (such as `sha256:` digest prefixes and `docker://` protocol schemes) and checks against bare hashes, action names, and canonical references (`action@hash`).

If security feedback is enabled, these statuses are appended inline in the printed terminal diff:
*   `[✓ vetted]` (in bold green)
*   `[✗ compromised]` (in bold red)
*   `[? not checked]` (in yellow)

---

## Interactive UI and Confirmation (`ui.rs`)

Disk writing is protected by user-interaction:
*   **Dry Run**: Outputs diffs to the console without writing changes.
*   **Interactive Confirmation**: If `--yes` (`-y`) is not set, a progress bar/interactive prompt displays each patch's diff and asks the user to confirm application (`[y/N]`) before writing to disk.
*   **Atomic Writes**: All replacements for a file are computed in memory first. `disk::write_atomic` then writes the result to a temporary file in the same directory and renames it over the original, so an interrupted run never leaves a truncated workflow. File permissions are preserved, and symlinks are followed rather than replaced.
