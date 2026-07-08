use crate::core::UpdateResult;
use crate::error::PinnerError;
use regex::Regex;
use std::sync::LazyLock;

/// Regex used to identify "version-only" comments that should be replaced during an update.
///
/// If a comment matches this pattern (e.g., `# v1`, `# main`), it is considered a
/// placeholder for the dependency version and is replaced by the new version's tag.
static COMMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^#\s*(v\d[a-zA-Z0-9.\-_]*|main|\d[a-zA-Z0-9.\-_]*)\s*")
        .expect("Failed to compile COMMENT_REGEX")
});

/// Applies an update to the string content of a YAML file.
///
/// This function surgically modifies the source text at the precise byte offsets
/// identified during the scanning phase. It handles:
/// 1. Preservation of existing non-version comments.
/// 2. Appending the old tag as a comment for readability (security best practice).
/// 3. Correct separator usage (`@` for most, `:` for Bitbucket pipes).
///
/// Returns `Ok(Some((old_text, new_text)))` if a change was applied, or `Ok(None)` if
/// the content remains identical.
pub fn apply_update(
    content: &mut String,
    res: &UpdateResult,
) -> Result<Option<(String, String)>, PinnerError> {
    // Determine the end of the line to capture any existing trailing comments.
    let line_end = content[res.task.end..]
        .find('\n')
        .map(|pos| res.task.end + pos)
        .unwrap_or(content.len());

    let suffix = &content[res.task.end..line_end];

    // logic to handle existing comments:
    // If the comment was just the version (e.g., "# v1"), we want to replace it.
    // If it contained more info (e.g., "# v1 # important"), we want to keep the extra info.
    let mut final_suffix = suffix.trim_start().to_string();
    if let Some(parser_comment) = &res.task.comment {
        if let Some(mat) = COMMENT_REGEX.find(parser_comment) {
            // Strip the version part but keep the rest.
            final_suffix = parser_comment[mat.end()..].trim_start().to_string();
        } else {
            final_suffix = parser_comment.clone();
        }
    } else if let Some(mat) = COMMENT_REGEX.find(&final_suffix) {
        final_suffix = final_suffix[mat.end()..].trim_start().to_string();
    }

    // Ensure we don't have a double # at the start if we stripped the first one.
    if final_suffix.starts_with('#') {
        final_suffix = final_suffix[1..].trim_start().to_string();
    }

    // Prepare the new comment showing the symbolic tag (e.g., " # v3").
    let new_comment = if let Some(t) = &res.new_tag {
        let is_sha =
            (t.len() == 40 && t.chars().all(|c| c.is_ascii_hexdigit())) || t.starts_with("sha256:");
        if is_sha {
            // Don't add a comment if the tag is already a SHA or digest.
            "".to_string()
        } else {
            format!(" # {}", t)
        }
    } else {
        "".to_string()
    };

    // Reconstruct the trailing part of the line, merging the new version comment with any existing comments.
    let extra_suffix = if final_suffix.is_empty() {
        "".to_string()
    } else if final_suffix.starts_with('#') {
        format!(" {}", final_suffix)
    } else {
        format!(" # {}", final_suffix)
    };

    let new_val = if res.task.key == "ref" {
        format!("{}{}{}", res.new_sha, new_comment, extra_suffix)
    } else {
        let separator = if res.task.key == "pipe" { ":" } else { "@" };
        format!(
            "{}{}{}{}{}",
            res.task.action, separator, res.new_sha, new_comment, extra_suffix
        )
    };

    // Calculate line starts in the original content to find the preceding line if any.
    let mut line_starts = vec![0];
    for (idx, c) in content.char_indices() {
        if c == '\n' {
            line_starts.push(idx + 1);
        }
    }

    let mut start_range = res.task.start;
    let mut prefix_replacement = "".to_string();

    if res.task.line >= 2 {
        let prev_line_idx = res.task.line - 2;
        if prev_line_idx < line_starts.len() {
            let start = line_starts[prev_line_idx];
            let end = if prev_line_idx + 1 < line_starts.len() {
                line_starts[prev_line_idx + 1]
            } else {
                content.len()
            };
            let prev_line_str = &content[start..end];
            let trimmed = prev_line_str.trim();
            if trimmed.starts_with('#') && COMMENT_REGEX.is_match(trimmed) {
                if let Some(new_t) = &res.new_tag {
                    let is_sha = (new_t.len() == 40
                        && new_t.chars().all(|c| c.is_ascii_hexdigit()))
                        || new_t.starts_with("sha256:");
                    if !is_sha {
                        let indent = prev_line_str.len() - prev_line_str.trim_start().len();
                        let indent_str = &prev_line_str[..indent];
                        let newline_str = if prev_line_str.ends_with('\n') {
                            "\n"
                        } else {
                            ""
                        };

                        start_range = start;
                        prefix_replacement = format!(
                            "{}# {}{}{}",
                            indent_str,
                            new_t,
                            newline_str,
                            &content[end..res.task.start]
                        );
                    }
                }
            }
        }
    }

    let full_new_text = format!("{}{}", prefix_replacement, new_val);
    let full_old_text = content[start_range..line_end].to_string();

    if full_old_text == full_new_text {
        return Ok(None);
    }

    // Surgically replace the range in the original content string.
    content.replace_range(start_range..line_end, &full_new_text);
    Ok(Some((full_old_text, full_new_text)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{DependencyName, DependencyRef, UpdateResult, UpdateTask};
    use std::path::PathBuf;

    #[test]
    fn test_apply_update_basic() {
        let mut content = "uses: actions/checkout@v3".to_string();
        let res = UpdateResult {
            action: DependencyName::from("actions/checkout"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v3".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 6,
                end: 25,
                action: DependencyName::from("actions/checkout"),
                current_tag: Some("v3".to_string()),
                comment: None,
                preceding_comments: None,
                key: "uses".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("hashv3".to_string()),
            new_tag: Some("v3".to_string()),
        };

        let result = apply_update(&mut content, &res).unwrap();
        assert!(result.is_some());
        assert_eq!(content, "uses: actions/checkout@hashv3 # v3");
    }

    #[test]
    fn test_apply_update_preceding_comment() {
        let mut content = "# v1\nuses: actions/checkout@v1".to_string();
        let res = UpdateResult {
            action: DependencyName::from("actions/checkout"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 11,
                end: 30,
                action: DependencyName::from("actions/checkout"),
                current_tag: Some("v1".to_string()),
                comment: None,
                preceding_comments: Some("# v1".to_string()),
                key: "uses".to_string(),
                line: 2,
                column: 7,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("hashv2".to_string()),
            new_tag: Some("v2".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "# v2\nuses: actions/checkout@hashv2 # v2");
    }

    #[test]
    fn test_apply_update_with_existing_comment() {
        let mut content = "uses: o/r@v1 # keep me".to_string();
        let res = UpdateResult {
            action: DependencyName::from("o/r"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 6,
                end: 12,
                action: DependencyName::from("o/r"),
                current_tag: Some("v1".to_string()),
                comment: Some("# keep me".to_string()),
                preceding_comments: None,
                key: "uses".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v2 # keep me");
    }

    #[test]
    fn test_apply_update_comment_regex_replacement() {
        let mut content = "uses: o/r@v1 # v1".to_string();
        let res = UpdateResult {
            action: DependencyName::from("o/r"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 6,
                end: 12,
                action: DependencyName::from("o/r"),
                current_tag: Some("v1".to_string()),
                comment: Some("# v1".to_string()),
                preceding_comments: None,
                key: "uses".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v2");
    }

    #[test]
    fn test_apply_update_no_redundant_sha_comment() {
        let mut content = "image: cimg/base@sha256:oldhash # stable".to_string();
        let res = UpdateResult {
            action: DependencyName::from("cimg/base"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("sha256:oldhash".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 7,
                end: 31,
                action: DependencyName::from("cimg/base"),
                current_tag: Some("sha256:oldhash".to_string()),
                comment: Some("# stable".to_string()),
                preceding_comments: None,
                key: "image".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitHub,
            },
            new_sha: DependencyRef::from("sha256:newhash".to_string()),
            new_tag: Some("sha256:newhash".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        // Since new_tag is a SHA, it shouldn't be added as a comment.
        assert_eq!(content, "image: cimg/base@sha256:newhash # stable");
    }

    #[test]
    fn test_apply_update_gitlab_ref() {
        let mut content = "ref: v1".to_string();
        let res = UpdateResult {
            action: DependencyName::from("proj"),
            path: PathBuf::from("f.yml"),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: PathBuf::from("f.yml"),
                start: 5,
                end: 7,
                action: DependencyName::from("proj"),
                current_tag: Some("v1".to_string()),
                comment: None,
                preceding_comments: None,
                key: "ref".to_string(),
                line: 1,
                column: 1,
                provider: crate::core::CiProvider::GitLab,
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v1".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "ref: hash # v1");
    }

    #[test]
    fn test_apply_update_no_newline_at_end() {
        let mut content = "uses: o/r@v1".to_string(); // No newline
        let res = UpdateResult {
            action: "o/r".into(),
            path: "f.yml".into(),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: "f.yml".into(),
                start: 6,
                end: 12,
                action: "o/r".into(),
                current_tag: Some("v1".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v1".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v1");
    }

    #[test]
    fn test_apply_update_complex_comments() {
        let mut content = "uses: o/r@v1  # v1 # keep # me".to_string();
        let res = UpdateResult {
            action: "o/r".into(),
            path: "f.yml".into(),
            old_tag: Some("v1".to_string()),
            task: UpdateTask {
                path: "f.yml".into(),
                start: 6,
                end: 12,
                action: "o/r".into(),
                current_tag: Some("v1".to_string()),
                comment: Some("# v1 # keep # me".to_string()),
                key: "uses".to_string(),
                ..Default::default()
            },
            new_sha: DependencyRef::from("hash".to_string()),
            new_tag: Some("v2".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "uses: o/r@hash # v2 # keep # me");
    }

    #[test]
    fn test_apply_update_docker_digest() {
        let mut content = "image: alpine:latest".to_string();
        let res = UpdateResult {
            action: "alpine".into(),
            path: "f.yml".into(),
            old_tag: Some("latest".to_string()),
            task: UpdateTask {
                path: "f.yml".into(),
                start: 7,
                end: 20,
                action: "alpine".into(),
                current_tag: Some("latest".to_string()),
                key: "image".to_string(),
                ..Default::default()
            },
            new_sha: DependencyRef::from("sha256:digest".to_string()),
            new_tag: Some("latest".to_string()),
        };

        apply_update(&mut content, &res).unwrap();
        assert_eq!(content, "image: alpine@sha256:digest # latest");
    }
}
